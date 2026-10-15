use serde::{Deserialize, Serialize};

/// A note in the second brain.
/// Stored as Markdown file on disk, metadata in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Note {
    pub id: i64,
    pub title: String,
    pub slug: String,
    pub filename: String,
    pub tags: String,
    pub word_count: i64,
    pub summary: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl Note {
    /// Generate a slug from a title (lowercase, hyphens for spaces, no special chars).
    pub fn slugify(title: &str) -> String {
        title
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else if c.is_whitespace() {
                    '-'
                } else {
                    '\0'
                }
            })
            .filter(|c| *c != '\0')
            .collect::<String>()
            .trim_matches('-')
            .to_string()
    }

    /// Convert tags string to Vec<String>
    pub fn tags_vec(&self) -> Vec<String> {
        self.tags
            .split(',')
            .filter(|t| !t.is_empty())
            .map(|t| t.trim().to_string())
            .collect()
    }

    /// Format tags as display string
    pub fn tags_display(&self) -> String {
        self.tags
            .split(',')
            .filter(|t| !t.is_empty())
            .map(|t| format!("tag-{}", t.trim().to_lowercase().replace(' ', "-")))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Request to create a new note.
#[derive(Debug, Deserialize)]
pub struct CreateNoteRequest {
    pub title: String,
    pub content: Option<String>,
    pub tags: Option<String>,
}

/// Request to update an existing note.
#[derive(Debug, Deserialize)]
pub struct UpdateNoteRequest {
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<String>,
}

/// Response for a note with content.
#[derive(Debug, Serialize)]
pub struct NoteWithContent {
    #[serde(flatten)]
    pub note: Note,
    pub content: Option<String>,
}
