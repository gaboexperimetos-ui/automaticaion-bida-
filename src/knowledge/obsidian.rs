//! Obsidian-compatible import and export for Markdown notes.
//!
//! The *vault* is the notes directory of the app (`Config::notes_dir`): every
//! note is a Markdown file named after its slug (`{slug}.md`) with a YAML
//! frontmatter block, and the notes link to each other with `[[slug]]`
//! wikilinks.
//!
//! - **Import** reads `.md` files from a source directory (read-only — the
//!   original files are never modified) and creates new notes in the vault.
//! - **Export** writes notes as Obsidian documents:
//!   - [`KnowledgeService::export_note`] — one note to a chosen path.
//!   - [`export_vault`] — the whole vault, built from the `.md` files on disk.
//!   - [`KnowledgeService::export_vault_from_db`] — the whole vault, with the
//!     metadata taken from the database and the body from disk.
//!
//! ## Vault format
//!
//! Every exported note is a `.md` file named after the slug of the note and
//! starts with a YAML frontmatter block with exactly these four properties:
//!
//! ```markdown
//! ---
//! title: Ondas y fotones
//! tags: [fisica, ondas]
//! created: 2026-10-19T19:00:00+00:00
//! updated: 2026-10-19T19:00:00+00:00
//! ---
//!
//! Cuerpo de la nota...
//! ```
//!
//! The file is named after the slug on purpose: the app writes its wikilinks
//! as `[[slug]]` and Obsidian resolves a wikilink against the name of the
//! file, so `[[ondas]]` points to `ondas.md`. The export also reports (as a
//! warning, never as an error) the links that Obsidian will not be able to
//! resolve inside the exported vault.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::knowledge::frontmatter::{build_note, parse_note, Frontmatter, ParsedNote};
use crate::knowledge::wikilinks::parse_wikilinks;
use crate::knowledge::KnowledgeService;
use crate::models::Note;

impl KnowledgeService {
    /// Import all `.md` files from an Obsidian/vault source directory.
    ///
    /// Recursively walks `source_dir`, parses each Markdown file's frontmatter
    /// and body, then creates a new note in the user's notes directory.
    ///
    /// Safety guarantees:
    /// - Source files are opened **read-only** and never modified.
    /// - If a note with the same slug already exists, the import skips it
    ///   rather than overwriting.
    ///
    /// Returns the number of notes successfully imported.
    pub async fn import_obsidian(&self, source_dir: &Path) -> Result<usize, String> {
        if !source_dir.is_dir() {
            return Err(format!(
                "Source directory does not exist: {}",
                source_dir.display()
            ));
        }

        let files = find_markdown_files(source_dir);
        if files.is_empty() {
            return Ok(0);
        }

        let mut imported = 0usize;

        for file_path in files {
            // Read-only: we only ever read, never write to source_dir
            let content = match std::fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("Import: failed to read {}: {}", file_path.display(), e);
                    continue;
                }
            };

            let parsed = parse_note(&content);

            // Derive title from frontmatter, H1 heading, or filename
            let title = derive_title(&parsed, &file_path);

            let tags = parsed.frontmatter.tags.clone();
            let body = parsed.content;

            match self.create_note(&title, body, tags).await {
                Ok(_) => {
                    imported += 1;
                    tracing::info!("Import: created '{}' from {}", title, file_path.display());
                }
                Err(e) => {
                    // Expected for slug collisions — skip and continue
                    tracing::warn!(
                        "Import: skipping {} ({}: {})",
                        file_path.display(),
                        if e.contains("already exists") {
                            "collision"
                        } else {
                            "error"
                        },
                        e
                    );
                    continue;
                }
            }
        }

        Ok(imported)
    }

    /// Export a single note to a Markdown file with YAML frontmatter.
    ///
    /// Reads the note's metadata from the DB and its body content from disk,
    /// then writes an Obsidian-compatible `.md` file to `export_path`.
    pub async fn export_note(&self, slug: &str, export_path: &Path) -> Result<(), String> {
        let (note, content) = self.get_note(slug).await?;

        let fm = Frontmatter {
            title: note.title.clone(),
            tags: note.tags_vec(),
            created: Some(note.created_at.clone()),
            modified: Some(note.updated_at.clone()),
            ..Default::default()
        };

        let markdown = build_note(&fm, &content);

        // Ensure the parent directory exists
        if let Some(parent) = export_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Err(format!(
                    "Failed to create export directory {}: {}",
                    parent.display(),
                    e
                ));
            }
        }

        std::fs::write(export_path, &markdown)
            .map_err(|e| format!("Failed to write export file: {}", e))?;

        Ok(())
    }

    /// Export the whole vault of `.md` files of `notes_dir` into `out_dir`.
    ///
    /// Thin wrapper over [`export_vault`]: the `knowledge::obsidian` module is
    /// private, so this associated function is the way to reach the vault
    /// export from the rest of the crate
    /// (`KnowledgeService::export_vault(notes_dir, out_dir)`).
    pub fn export_vault(notes_dir: &Path, out_dir: &Path) -> Result<usize, String> {
        crate::knowledge::obsidian::export_vault(notes_dir, out_dir)
    }

    /// Export the whole vault using the database as the source of truth.
    ///
    /// The title, the tags and the timestamps come from the `notes` table and
    /// the body from the `.md` file of each note (the body only exists on
    /// disk). A note whose file is missing is skipped with a warning: the
    /// export never aborts because of a single note.
    ///
    /// Returns the number of notes written into the vault.
    pub async fn export_vault_from_db(&self, out_dir: &Path) -> Result<usize, String> {
        std::fs::create_dir_all(out_dir).map_err(|e| {
            format!(
                "Failed to create vault directory {}: {}",
                out_dir.display(),
                e
            )
        })?;

        let notes: Vec<Note> =
            sqlx::query_as::<_, Note>("SELECT * FROM notes ORDER BY updated_at DESC")
                .fetch_all(&self.state.db)
                .await
                .map_err(|e| format!("DB error listing notes: {}", e))?;

        let mut exported: Vec<ExportedNote> = Vec::new();

        for note in notes {
            // The body of a note only lives on disk. The file is named after
            // the slug, which never carries path separators.
            let path = self.state.config.notes_dir.join(format!("{}.md", note.slug));
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(
                        "Export: cannot read the body of '{}': {}",
                        note.slug,
                        e
                    );
                    continue;
                }
            };

            let vault_note = VaultNote {
                slug: note.slug.clone(),
                title: note.title.clone(),
                tags: normalize_tags(&note.tags_vec()),
                created: Some(note.created_at.clone()),
                updated: Some(note.updated_at.clone()),
                body: parse_note(&content).content,
            };

            let target = out_dir.join(format!("{}.md", vault_note.slug));
            match std::fs::write(&target, vault_note.render()) {
                Ok(()) => {
                    tracing::info!(
                        "Export: '{}' -> {}",
                        vault_note.title,
                        target.display()
                    );
                    exported.push(ExportedNote {
                        slug: vault_note.slug,
                        title: vault_note.title,
                        body: vault_note.body,
                    });
                }
                Err(e) => {
                    tracing::warn!("Export: failed to write {}: {}", target.display(), e);
                }
            }
        }

        report_link_issues(&exported);
        Ok(exported.len())
    }
}

/// Export the whole vault of `.md` files of `notes_dir` into `out_dir`.
///
/// Every `.md` file of `notes_dir` becomes a note of the vault: the name of
/// the file becomes the slug of the note (`{slug}.md`), the frontmatter is
/// rebuilt with the four properties of the vault format (`title`, `tags`,
/// `created`, `updated`) and the body is written without frontmatter.
///
/// The metadata comes from the frontmatter of the file (the app writes it
/// from the database) and, when it is missing, from the modification time of
/// the file: no data is invented. A note that cannot be read or written is
/// reported with a warning and does not abort the export.
///
/// Returns the number of notes exported.
pub fn export_vault(notes_dir: &Path, out_dir: &Path) -> Result<usize, String> {
    if !notes_dir.is_dir() {
        return Err(format!(
            "Notes directory does not exist: {}",
            notes_dir.display()
        ));
    }

    std::fs::create_dir_all(out_dir).map_err(|e| {
        format!(
            "Failed to create vault directory {}: {}",
            out_dir.display(),
            e
        )
    })?;

    let files = find_markdown_files(notes_dir);
    let mut used: HashSet<String> = HashSet::new();
    let mut exported: Vec<ExportedNote> = Vec::new();

    for file in files {
        let content = match std::fs::read_to_string(&file) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Export: failed to read {}: {}", file.display(), e);
                continue;
            }
        };

        let parsed = parse_note(&content);
        let note = vault_note_from_file(&file, &parsed);
        let slug = unique_slug(&note.slug, &mut used);
        let note = VaultNote {
            slug: slug.clone(),
            ..note
        };

        // The folder structure of the source is kept: two notes with the same
        // name in different folders do not collide in the vault.
        let relative = file.strip_prefix(notes_dir).unwrap_or(&file);
        let target = match relative.parent().filter(|p| !p.as_os_str().is_empty()) {
            Some(dir) => out_dir.join(dir).join(format!("{}.md", slug)),
            None => out_dir.join(format!("{}.md", slug)),
        };

        if let Some(parent) = target.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("Export: cannot create {}: {}", parent.display(), e);
                continue;
            }
        }

        match std::fs::write(&target, note.render()) {
            Ok(()) => {
                tracing::info!("Export: '{}' -> {}", note.title, target.display());
                exported.push(ExportedNote {
                    slug,
                    title: note.title,
                    body: note.body,
                });
            }
            Err(e) => {
                tracing::warn!("Export: failed to write {}: {}", target.display(), e);
            }
        }
    }

    report_link_issues(&exported);
    Ok(exported.len())
}

/// A note ready to be written into an Obsidian vault.
struct VaultNote {
    /// Slug of the note: the name of the file in the vault (without `.md`)
    /// and the target of the `[[slug]]` wikilinks.
    slug: String,
    /// Human readable title (frontmatter `title`).
    title: String,
    /// Normalized tags (frontmatter `tags`).
    tags: Vec<String>,
    /// Creation timestamp (frontmatter `created`).
    created: Option<String>,
    /// Last modification timestamp (frontmatter `updated`).
    updated: Option<String>,
    /// Body of the note, without frontmatter.
    body: String,
}

impl VaultNote {
    /// Render the note as an Obsidian document: YAML frontmatter + body.
    ///
    /// The frontmatter holds exactly the four properties of the vault format:
    /// `title`, `tags`, `created` and `updated`.
    fn render(&self) -> String {
        let mut other: HashMap<String, String> = HashMap::new();
        if let Some(updated) = &self.updated {
            other.insert("updated".to_string(), updated.clone());
        }

        let fm = Frontmatter {
            title: self.title.clone(),
            tags: self.tags.clone(),
            created: self.created.clone(),
            modified: None,
            other,
        };

        build_note(&fm, &self.body)
    }
}

/// A note already written in the vault, kept to check its wikilinks.
struct ExportedNote {
    slug: String,
    title: String,
    body: String,
}

/// Build the vault entry of a note stored as a `.md` file.
///
/// The metadata comes from the frontmatter of the file (the app writes it
/// from the database) and, when it is missing, from the modification time of
/// the file. If neither is available the property is left out: no data is
/// invented.
fn vault_note_from_file(path: &Path, parsed: &ParsedNote) -> VaultNote {
    let stamp = file_timestamp(path);

    VaultNote {
        slug: slug_from_stem(path),
        title: derive_title(parsed, path),
        tags: normalize_tags(&parsed.frontmatter.tags),
        created: parsed.frontmatter.created.clone().or_else(|| stamp.clone()),
        updated: parsed
            .frontmatter
            .modified
            .clone()
            .or_else(|| parsed.frontmatter.other.get("updated").cloned())
            .or(stamp),
        body: parsed.content.clone(),
    }
}

/// Title of a note: the title of the frontmatter, the first H1 of the body
/// or, as a last resort, the name of the file with the underscores as spaces.
fn derive_title(parsed: &ParsedNote, path: &Path) -> String {
    if !parsed.frontmatter.title.is_empty() {
        return parsed.frontmatter.title.clone();
    }

    parsed
        .content
        .lines()
        .find_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.starts_with("# ") && !trimmed.starts_with("##") {
                Some(trimmed[2..].trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().replace('_', " "))
                .unwrap_or_else(|| "Imported Note".to_string())
        })
}

/// Slug of a note from the name of its file, with the same convention the app
/// uses to name the notes of the vault (`{slug}.md`).
fn slug_from_stem(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let slug = Note::slugify(&stem);
    if slug.is_empty() {
        "note".to_string()
    } else {
        slug
    }
}

/// Make a slug unique inside the vault: two notes with the same name (in
/// different folders) would be written to the same file, so the second one
/// gets a numeric suffix instead of overwriting the first.
fn unique_slug(base: &str, used: &mut HashSet<String>) -> String {
    let mut candidate = base.to_string();
    let mut suffix = 1usize;
    while used.contains(&candidate) {
        suffix += 1;
        candidate = format!("{}-{}", base, suffix);
    }
    used.insert(candidate.clone());
    if candidate != base {
        tracing::warn!(
            "Export: duplicate note name '{}', exported as '{}'",
            base,
            candidate
        );
    }
    candidate
}

/// Normalize a list of tags: split the CSV form, trim, drop the empty ones,
/// lowercase and remove the duplicates (keeping the order of appearance).
fn normalize_tags(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in raw {
        for piece in entry.split(',') {
            let tag = piece
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .trim()
                .to_lowercase();
            if tag.is_empty() {
                continue;
            }
            if !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    out
}

/// Modification time of a file as an RFC 3339 timestamp.
fn file_timestamp(path: &Path) -> Option<String> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    let stamp: chrono::DateTime<chrono::Utc> = modified.into();
    Some(stamp.to_rfc3339())
}

/// Check the wikilinks of the exported notes and report the ones that
/// Obsidian will not be able to resolve inside the vault.
///
/// The vault names its files after the slug (`[[slug]]` → `slug.md`), so a
/// link written with the title of a note is reported as well: it works inside
/// the app, but not in Obsidian.
fn report_link_issues(exported: &[ExportedNote]) {
    if exported.is_empty() {
        return;
    }

    let mut names: HashSet<String> = HashSet::with_capacity(exported.len());
    let mut titles: HashMap<String, String> = HashMap::with_capacity(exported.len());
    for note in exported {
        names.insert(note.slug.to_lowercase());
        if !note.title.is_empty() {
            titles.insert(note.title.to_lowercase(), note.slug.clone());
        }
    }

    for note in exported {
        for link in parse_wikilinks(&note.body) {
            let target = link.target.trim().to_lowercase();
            if target.is_empty() || names.contains(&target) {
                continue;
            }
            match titles.get(&target) {
                Some(slug) => tracing::warn!(
                    "Export: '{}' links to the title of '{}' ([[{}]]); in the vault the file is '{}.md'",
                    note.slug,
                    slug,
                    link.target,
                    slug
                ),
                None => tracing::warn!(
                    "Export: '{}' links to [[{}]], which is not part of the vault",
                    note.slug,
                    link.target
                ),
            }
        }
    }
}

/// Recursively find all `.md` files in a directory.
///
/// The traversal is shallow-safe: it skips directories that cannot be read
/// rather than failing. Source files are only read, never written.
fn find_markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Cannot read directory {}: {}", dir.display(), e);
            return files;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(find_markdown_files(&path));
        } else if path.extension().map_or(false, |ext| ext == "md") {
            files.push(path);
        }
    }

    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_markdown_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();

        // Create some files
        std::fs::write(dir.join("a.md"), "# A").unwrap();
        std::fs::write(dir.join("b.md"), "# B").unwrap();
        std::fs::write(dir.join("c.txt"), "not markdown").unwrap();

        let mut files = find_markdown_files(dir);
        files.sort();
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|p| p.ends_with("a.md")));
        assert!(files.iter().any(|p| p.ends_with("b.md")));
    }

    #[test]
    fn test_find_markdown_files_recursive() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();

        std::fs::write(dir.join("top.md"), "# Top").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/nested.md"), "# Nested").unwrap();

        let files = find_markdown_files(dir);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_find_markdown_files_empty_dir() {
        let temp = tempfile::tempdir().unwrap();
        let files = find_markdown_files(temp.path());
        assert!(files.is_empty());
    }

    /// La exportación de la bóveda escribe un `.md` por nota, con el
    /// frontmatter del formato Obsidian (`title`, `tags`, `created`,
    /// `updated`) y el cuerpo sin frontmatter.
    #[test]
    fn test_export_vault_escribe_frontmatter() {
        let temp = tempfile::tempdir().unwrap();
        let notes_dir = temp.path().join("notes");
        let out_dir = temp.path().join("vault");
        std::fs::create_dir_all(&notes_dir).unwrap();

        std::fs::write(
            notes_dir.join("fisica.md"),
            "---\ntitle: Física Cuántica\ntags: [Ondas, fotones]\ncreated: 2026-01-01T00:00:00Z\nmodified: 2026-01-02T00:00:00Z\n---\n\n# Teoría\n\nVer [[materia]].",
        )
        .unwrap();
        std::fs::write(
            notes_dir.join("materia.md"),
            "---\ntitle: Materia\ntags: [química]\n---\n\nContenido de la materia.",
        )
        .unwrap();

        let exported = export_vault(&notes_dir, &out_dir).expect("export vault");
        assert_eq!(exported, 2, "las dos notas de la bóveda se exportan");

        let fisica = std::fs::read_to_string(out_dir.join("fisica.md")).expect("nota exportada");
        assert!(fisica.starts_with("---\n"), "frontmatter: {}", fisica);
        assert!(fisica.contains("title: Física Cuántica"), "{}", fisica);
        assert!(fisica.contains("tags: [ondas, fotones]"), "{}", fisica);
        assert!(fisica.contains("created: 2026-01-01T00:00:00Z"), "{}", fisica);
        assert!(fisica.contains("updated: 2026-01-02T00:00:00Z"), "{}", fisica);
        assert!(fisica.contains("Ver [[materia]]."), "cuerpo: {}", fisica);

        let materia = std::fs::read_to_string(out_dir.join("materia.md")).expect("nota exportada");
        assert!(materia.contains("title: Materia"), "{}", materia);
        assert!(materia.contains("tags: [química]"), "{}", materia);
        // Sin timestamps en el fichero: se usa la fecha de modificación.
        assert!(materia.contains("created:"), "{}", materia);
        assert!(materia.contains("updated:"), "{}", materia);
        assert!(materia.contains("Contenido de la materia."), "{}", materia);
    }
}
