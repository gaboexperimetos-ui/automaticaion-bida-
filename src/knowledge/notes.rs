//! Note CRUD service — create, read, update, delete Markdown notes.
//!
//! Notes are persisted as:
//! - Content: Markdown file with YAML frontmatter on disk (`{slug}.md`)
//! - Metadata: row in the `notes` SQL table
//! - Version history: content snapshots in `note_versions` (last 5)
//! - Search index: row in `notes_fts` (FTS5, synced here — no triggers)

use sqlx::Row;

use crate::knowledge::frontmatter::{build_note, parse_note};
use crate::knowledge::KnowledgeService;
use crate::models::Note;

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
            r#"INSERT INTO notes (title, slug, filename, tags, word_count, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(title)
        .bind(&slug)
        .bind(&filename)
        .bind(&tags_str)
        .bind(word_count)
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
    /// Any `None` argument leaves that field unchanged. Saves a content
    /// version snapshot before writing (last 5 retained).
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
        let new_content = content.unwrap_or_else(|| current_body);
        let new_word_count = count_words(&new_content);

        // Save a content version (snapshot of current state)
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

        // Prune to last 5 versions
        sqlx::query(
            r#"DELETE FROM note_versions
               WHERE note_id = ? AND id NOT IN (
                   SELECT id FROM note_versions
                   WHERE note_id = ?
                   ORDER BY id DESC
                   LIMIT 5
               )"#,
        )
        .bind(note.id)
        .bind(note.id)
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("DB error pruning versions: {}", e))?;

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

        // Update DB metadata
        sqlx::query(
            r#"UPDATE notes
               SET title = ?, tags = ?, word_count = ?, updated_at = ?
               WHERE slug = ?"#,
        )
        .bind(&new_title)
        .bind(&new_tags_str)
        .bind(new_word_count)
        .bind(&now)
        .bind(slug)
        .execute(&self.state.db)
        .await
        .map_err(|e| format!("DB error updating note: {}", e))?;

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
