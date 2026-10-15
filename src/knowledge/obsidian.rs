//! Obsidian-compatible import and export for Markdown notes.
//!
//! Import reads `.md` files from a source directory (read-only — original files
//! are never modified) and creates new notes in the user's notes directory.
//! Export writes a single note to a specified path as Markdown with YAML
//! frontmatter.

use std::path::{Path, PathBuf};

use crate::knowledge::frontmatter::{build_note, parse_note, Frontmatter};
use crate::knowledge::KnowledgeService;

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
            let title = if !parsed.frontmatter.title.is_empty() {
                parsed.frontmatter.title.clone()
            } else {
                content
                    .lines()
                    .find(|l| {
                        let t = l.trim_start();
                        t.starts_with("# ") && !t.starts_with("##")
                    })
                    .map(|l| l.trim_start()[2..].trim().to_string())
                    .unwrap_or_else(|| {
                        file_path
                            .file_stem()
                            .map(|s| s.to_string_lossy().replace('_', " "))
                            .unwrap_or_else(|| "Imported Note".to_string())
                    })
            };

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
}
