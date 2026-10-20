//! Note CRUD service — create, read, update, delete Markdown notes.
//!
//! Notes are persisted as:
//! - Content: Markdown file with YAML frontmatter on disk (`{slug}.md`)
//! - Metadata: row in the `notes` SQL table
//! - Version history: content snapshots in `note_versions` (last 5 distinct
//!   states; the newest one is the body the note currently has)
//! - Search index: row in `notes_fts` (FTS5, synced here — no triggers)

use sqlx::Row;

use crate::knowledge::frontmatter::{build_note, parse_note, summarize};
use crate::knowledge::KnowledgeService;
use crate::models::Note;

/// Longitud máxima (en caracteres) del resumen que se guarda en
/// `notes.summary`.
///
/// Coincide con el `SNIPPET_CHARS` del buscador (`handlers::search`): el
/// resumen es el fallback del snippet cuando el `.md` no está en disco, así
/// que debe poder recortarse al mismo tamaño sin perder información.
const SUMMARY_CHARS: usize = 240;

impl KnowledgeService {
    /// Create a new Markdown note.
    ///
    /// Writes an Obsidian-compatible `.md` file (YAML frontmatter + body),
    /// inserts metadata into the `notes` table, stores an initial content
    /// version in `note_versions`, and updates the FTS5 index.
    pub async fn create_note(
        &self,
        title: &str,
        content: String,
        tags: Vec<String>,
    ) -> Result<Note, String> {
        let slug = Note::slugify(title);
        let filename = format!("{}.md", &slug);
        let file_path = self.state.config.notes_dir.join(&filename);
        let now = chrono::Utc::now().to_rfc3339();

        // Check for slug collision
        let exists: Option<Note> = sqlx::query_as("SELECT * FROM notes WHERE slug = ?")
            .bind(&slug)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("DB error checking slug: {}", e))?;

        if exists.is_some() {
            return Err(format!("Note with slug '{}' already exists", slug));
        }

        // Ensure notes directory exists
        if let Err(e) = std::fs::create_dir_all(&self.state.config.notes_dir) {
            return Err(format!("Failed to create notes directory: {}", e));
        }

        let word_count = count_words(&content);
        let tags_str = tags.join(",");
        // Resumen del cuerpo: lo usa el buscador como fallback del snippet
        // cuando el `.md` no está en disco.
        let summary = summarize(&content, SUMMARY_CHARS);

        // Build the Markdown file with frontmatter
        let fm = crate::knowledge::frontmatter::Frontmatter {
            title: title.to_string(),
            tags: tags.clone(),
            created: Some(now.clone()),
            modified: Some(now.clone()),
            ..Default::default()
        };
        let markdown = build_note(&fm, &content);

        // Write file to disk
        if let Err(e) = std::fs::write(&file_path, &markdown) {
            return Err(format!("Failed to write note file: {}", e));
        }

        // Insert metadata into DB
        let result = sqlx::query(
            r#"INSERT INTO notes (title, slug, filename, tags, word_count, summary, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(title)
        .bind(&slug)
        .bind(&filename)
        .bind(&tags_str)
        .bind(word_count)
        .bind(&summary)
        .bind(&now)
        .bind(&now)
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("DB error inserting note: {}", e))?;

        let note_id = result.last_insert_rowid();

        // Store initial content version
        sqlx::query(
            r#"INSERT INTO note_versions (note_id, content, word_count, created_at)
               VALUES (?, ?, ?, ?)"#,
        )
        .bind(note_id)
        .bind(&content)
        .bind(word_count)
        .bind(&now)
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("DB error storing version: {}", e))?;

        // Update FTS5 index (non-fatal — search falls back to LIKE if this fails)
        let _ = sqlx::query(r#"INSERT INTO notes_fts(rowid, title, content) VALUES (?, ?, ?)"#)
            .bind(note_id)
            .bind(title)
            .bind(&content)
            .execute(&self.state.db)
            .await
            .map_err(|e| {
                tracing::warn!("FTS5 insert failed (will use LIKE fallback): {}", e);
                format!("FTS5 error: {}", e)
            });

        // Return the created note
        let note: Note = sqlx::query_as("SELECT * FROM notes WHERE id = ?")
            .bind(note_id)
            .fetch_one(&self.state.db)
            .await
            .map_err(|e| format!("DB error fetching created note: {}", e))?;

        Ok(note)
    }

    /// Retrieve a note by slug.
    ///
    /// Returns `(Note, content)` where `content` is the Markdown body (without
    /// frontmatter). Metadata comes from the DB; content comes from the disk
    /// file.
    pub async fn get_note(&self, slug: &str) -> Result<(Note, String), String> {
        let note: Note = sqlx::query_as("SELECT * FROM notes WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("DB error: {}", e))?
            .ok_or_else(|| format!("Note not found: {}", slug))?;

        let file_path = self.state.config.notes_dir.join(format!("{}.md", slug));
        let file_content = std::fs::read_to_string(&file_path)
            .map_err(|e| format!("Failed to read note file {}: {}", file_path.display(), e))?;

        let parsed = parse_note(&file_content);

        Ok((note, parsed.content))
    }

    /// Update an existing note.
    ///
    /// Any `None` argument leaves that field unchanged.
    ///
    /// The version history is real: the content that the edit replaces is
    /// archived in `note_versions` *before* the new body is written, and the
    /// new body is archived as the current state once it is on disk. The last
    /// 5 distinct contents are retained per note.
    pub async fn update_note(
        &self,
        slug: &str,
        title: Option<String>,
        content: Option<String>,
        tags: Option<Vec<String>>,
    ) -> Result<(), String> {
        let note: Note = sqlx::query_as("SELECT * FROM notes WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("DB error: {}", e))?
            .ok_or_else(|| format!("Note not found: {}", slug))?;

        let now = chrono::Utc::now().to_rfc3339();
        let file_path = self.state.config.notes_dir.join(format!("{}.md", slug));

        // Determine final values
        let new_title = title.unwrap_or_else(|| note.title.clone());
        let new_tags = tags.unwrap_or_else(|| note.tags_vec());
        let new_tags_str = new_tags.join(",");

        // Read current file content to determine body
        let current_body = {
            let file_content = std::fs::read_to_string(&file_path).unwrap_or_default();
            let parsed = parse_note(&file_content);
            // If the file had frontmatter with its own title, use the body;
            // otherwise the whole file is the body.
            if parsed.frontmatter.title.is_empty() && parsed.content.is_empty() {
                file_content
            } else {
                parsed.content
            }
        };

        // Determine the new content body
        let new_content = content.unwrap_or_else(|| current_body.clone());
        let new_word_count = count_words(&new_content);
        let content_changed = new_content != current_body;

        // Archive the content that is being REPLACED (the state that really
        // exists on disk right now) before writing the new one. This is the
        // real history: if the write below fails, the history keeps the true
        // state instead of a phantom one that was never persisted.
        if content_changed {
            sqlx::query(
                r#"INSERT INTO note_versions (note_id, content, word_count, created_at)
                   VALUES (?, ?, ?, ?)"#,
            )
            .bind(note.id)
            .bind(&current_body)
            .bind(count_words(&current_body))
            .bind(&now)
            .execute(&self.state.db)
            .await
            .map_err(|e| format!("DB error storing version: {}", e))?;
        }

        // Write updated file with frontmatter
        let fm = crate::knowledge::frontmatter::Frontmatter {
            title: new_title.clone(),
            tags: new_tags.clone(),
            created: Some(note.created_at.clone()),
            modified: Some(now.clone()),
            ..Default::default()
        };
        let markdown = build_note(&fm, &new_content);

        if let Err(e) = std::fs::write(&file_path, &markdown) {
            return Err(format!("Failed to write note file: {}", e));
        }

        // Update DB metadata (the summary is kept in sync with the body so
        // the search can fall back to it when the file is not on disk).
        let new_summary = summarize(&new_content, SUMMARY_CHARS);
        sqlx::query(
            r#"UPDATE notes
               SET title = ?, tags = ?, word_count = ?, summary = ?, updated_at = ?
               WHERE slug = ?"#,
        )
        .bind(&new_title)
        .bind(&new_tags_str)
        .bind(new_word_count)
        .bind(&new_summary)
        .bind(&now)
        .bind(slug)
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("DB error updating note: {}", e))?;

        // Archive the NEW content as the current state, now that it is
        // persisted on disk. The newest row of `note_versions` must hold the
        // body the note currently has: the `notes_search_fts` triggers
        // (migración 20240101000006) and the LIKE fallback of the search read
        // the latest version to know the current body of a note.
        if content_changed {
            sqlx::query(
                r#"INSERT INTO note_versions (note_id, content, word_count, created_at)
                   VALUES (?, ?, ?, ?)"#,
            )
            .bind(note.id)
            .bind(&new_content)
            .bind(new_word_count)
            .bind(&now)
            .execute(&self.state.db)
            .await
            .map_err(|e| format!("DB error storing version: {}", e))?;
        }

        // Prune to the last 5 distinct contents. An archived state and its
        // "current" copy count as one, so the history keeps 5 real states
        // instead of losing half of them to duplicates.
        sqlx::query(
            r#"DELETE FROM note_versions
               WHERE note_id = ?
                 AND id NOT IN (
                     SELECT id FROM (
                         SELECT MAX(id) AS id
                           FROM note_versions
                          WHERE note_id = ?
                          GROUP BY content
                          ORDER BY MAX(id) DESC
                          LIMIT 5
                     ) AS keep
                 )"#,
        )
        .bind(note.id)
        .bind(note.id)
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("DB error pruning versions: {}", e))?;

        // Sync FTS5 index (non-fatal)
        let _ = sqlx::query("DELETE FROM notes_fts WHERE rowid = ?")
            .bind(note.id)
            .execute(&self.state.db)
            .await;

        let _ = sqlx::query(r#"INSERT INTO notes_fts(rowid, title, content) VALUES (?, ?, ?)"#)
            .bind(note.id)
            .bind(&new_title)
            .bind(&new_content)
            .execute(&self.state.db)
            .await;

        Ok(())
    }

    /// Delete a note by slug.
    ///
    /// Removes the FTS5 entry, the disk file, and the DB row (note_versions
    /// are cascade-deleted by the foreign key).
    pub async fn delete_note(&self, slug: &str) -> Result<(), String> {
        let note: Note = sqlx::query_as("SELECT * FROM notes WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("DB error: {}", e))?
            .ok_or_else(|| format!("Note not found: {}", slug))?;

        // Remove FTS5 entry (non-fatal)
        let _ = sqlx::query("DELETE FROM notes_fts WHERE rowid = ?")
            .bind(note.id)
            .execute(&self.state.db)
            .await;

        // Remove disk file
        let file_path = self.state.config.notes_dir.join(format!("{}.md", slug));
        if file_path.exists() {
            if let Err(e) = std::fs::remove_file(&file_path) {
                return Err(format!("Failed to delete note file: {}", e));
            }
        }

        // Delete DB row (triggers cascade to note_versions)
        let result = sqlx::query("DELETE FROM notes WHERE id = ?")
            .bind(note.id)
            .execute(&self.state.db)
            .await
            .map_err(|e| format!("DB error deleting note: {}", e))?;

        if result.rows_affected() == 0 {
            return Err("Note was not deleted".to_string());
        }

        Ok(())
    }

    /// List notes as owned `Note` values, ordered by most recently updated.
    pub async fn list_notes(&self, limit: usize) -> Result<Vec<Note>, String> {
        sqlx::query_as::<_, Note>("SELECT * FROM notes ORDER BY updated_at DESC LIMIT ?")
            .bind(limit as i64)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("DB error: {}", e))
    }

    /// Retrieve content version history for a note (newest first, max 5).
    pub async fn version_history(&self, slug: &str) -> Result<Vec<VersionEntry>, String> {
        let note: Note = sqlx::query_as("SELECT * FROM notes WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.state.db)
            .await
            .map_err(|e| format!("DB error: {}", e))?
            .ok_or_else(|| format!("Note not found: {}", slug))?;

        let rows = sqlx::query(
            r#"SELECT id, content, word_count, created_at
               FROM note_versions
               WHERE note_id = ?
               ORDER BY id DESC
               LIMIT 5"#,
        )
        .bind(note.id)
        .fetch_all(&self.state.db)
        .await
        .map_err(|e| format!("DB error: {}", e))?;

        let mut versions = Vec::new();
        for row in rows {
            versions.push(VersionEntry {
                id: row.try_get("id").map_err(|e| format!("DB error: {}", e))?,
                content: row
                    .try_get("content")
                    .map_err(|e| format!("DB error: {}", e))?,
                word_count: row
                    .try_get("word_count")
                    .map_err(|e| format!("DB error: {}", e))?,
                created_at: row
                    .try_get("created_at")
                    .map_err(|e| format!("DB error: {}", e))?,
            });
        }

        Ok(versions)
    }

    /// Resolve which existing notes are linked from a note's content.
    pub async fn backlinks(&self, slug: &str) -> Result<Vec<Backlink>, String> {
        let all_notes: Vec<Note> = self.list_notes(1000).await?;
        let mut backlinks = Vec::new();

        for note in all_notes {
            let file_path = self
                .state
                .config
                .notes_dir
                .join(format!("{}.md", note.slug));
            if let Ok(content) = std::fs::read_to_string(&file_path) {
                for link in crate::knowledge::wikilinks::parse_wikilinks(&content) {
                    // Exclude self-links by comparing the *source* note slug,
                    // not the raw link target text (which differs in case/spacing
                    // from the slug, e.g. "Target Note" vs "target-note").
                    if link.resolve() == slug && note.slug != slug {
                        backlinks.push(Backlink {
                            from_slug: note.slug.clone(),
                            from_title: note.title.clone(),
                            display: link.display.clone().unwrap_or_else(|| link.target.clone()),
                            heading: link.heading.clone(),
                        });
                    }
                }
            }
        }

        Ok(backlinks)
    }
}

/// A content version snapshot from `note_versions`.
#[derive(Debug, Clone)]
pub struct VersionEntry {
    pub id: i64,
    pub content: String,
    pub word_count: i64,
    pub created_at: String,
}

/// A backlink entry — a note that links to another note.
#[derive(Debug, Clone)]
pub struct Backlink {
    pub from_slug: String,
    pub from_title: String,
    pub display: String,
    pub heading: Option<String>,
}

/// Count whitespace-separated words in a string.
fn count_words(content: &str) -> i64 {
    content.split_whitespace().count() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppState, Config};
    use std::path::PathBuf;
    use std::sync::Arc;

    /// `KnowledgeService` sobre una BD SQLite temporal con las migraciones
    /// aplicadas (FTS5 incluido) y un directorio de notas aislado.
    async fn setup() -> (KnowledgeService, PathBuf, tempfile::NamedTempFile) {
        let db = tempfile::NamedTempFile::new().expect("db temporal");
        let db_url = format!("sqlite://{}", db.path().display());
        let notes_dir = tempfile::tempdir().expect("directorio temporal").keep();

        let config = Config {
            host: "127.0.0.1".to_string(),
            port: 18091,
            session_secret: "test_secret_key_at_least_32_chars_long!!".to_string(),
            db_path: db.path().to_path_buf(),
            notes_dir: notes_dir.clone(),
            openrouter_api_key: None,
            openrouter_model: "test/model".to_string(),
            openrouter_max_tokens: 1024,
            openrouter_max_cost_usd: 1.0,
            mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
            genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
            custom_mcp_api_key: None,
            registration_token: None,
            rust_log: "test".to_string(),
            auth_disabled: true,
            db_max_connections: 4,
            mcp_timeout_ms: 5_000,
        };

        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&db_url)
            .await
            .expect("conectar a la BD de test");
        sqlx::migrate!().run(&pool).await.expect("migraciones");

        let state = Arc::new(AppState {
            config,
            db: pool,
            mikareno: None,
            start_time: std::time::Instant::now(),
        });

        // El `NamedTempFile` se devuelve para que NO se borre al salir de
        // `setup()`: SQLite abre la BD por ruta y, si el archivo desaparece,
        // la escritura falla con "attempt to write a readonly database".
        (KnowledgeService::new(state), notes_dir, db)
    }

    /// Editar una nota archiva el contenido ANTERIOR y deja el nuevo como
    /// estado actual; el `summary` de `notes` se mantiene al día.
    #[tokio::test]
    async fn test_update_archiva_el_contenido_anterior() {
        let (svc, notes_dir, _db) = setup().await;

        let note = svc
            .create_note("Historial", "contenido v1".to_string(), vec![])
            .await
            .expect("create_note");

        svc.update_note(&note.slug, None, Some("contenido v2".to_string()), None)
            .await
            .expect("update_note");

        // El historial contiene el estado anterior (v1) y el actual (v2),
        // y el más reciente es el cuerpo que la nota tiene ahora.
        let versions = svc
            .version_history(&note.slug)
            .await
            .expect("version_history");
        let contents: Vec<&str> = versions.iter().map(|v| v.content.as_str()).collect();
        assert!(
            contents.contains(&"contenido v1"),
            "el historial debe guardar el contenido anterior: {:?}",
            contents
        );
        assert_eq!(
            contents.first().copied(),
            Some("contenido v2"),
            "el historial debe guardar el contenido actual: {:?}",
            contents
        );

        // El `summary` de `notes` refleja el cuerpo nuevo.
        let summary: Option<String> =
            sqlx::query_scalar("SELECT summary FROM notes WHERE slug = ?")
                .bind(&note.slug)
                .fetch_one(&svc.state.db)
                .await
                .expect("summary");
        assert_eq!(summary.as_deref(), Some("contenido v2"));

        // El archivo en disco tiene el cuerpo nuevo.
        let raw = std::fs::read_to_string(notes_dir.join(format!("{}.md", note.slug)))
            .expect("nota en disco");
        assert!(raw.contains("contenido v2"), "archivo: {}", raw);
    }
}
