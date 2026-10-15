//! Data models for the Study Platform.
//! All models derive Serialize (for JSON API responses) and FromRow (for SQLite).

use serde::Serialize;

mod note;
mod user;

pub use note::{CreateNoteRequest, Note, NoteWithContent, UpdateNoteRequest};
pub use user::{RegisterRequest, SetPasswordRequest, User};

/// A session for an authenticated user.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub user_id: i64,
    pub created_at: String,
    pub expires_at: String,
    #[serde(skip)]
    pub user_agent: Option<String>,
}

impl Session {
    /// Check if the session is still valid (not expired).
    pub fn is_valid(&self) -> bool {
        let now = chrono::Utc::now().to_rfc3339();
        self.expires_at > now
    }
}

/// Pagination metadata for list responses.
#[derive(Debug, Clone, Serialize)]
pub struct Pagination {
    pub page: u64,
    pub per_page: u64,
    pub total: u64,
}

impl Pagination {
    pub fn new(page: u64, per_page: u64, total: u64) -> Self {
        Self {
            page,
            per_page,
            total,
        }
    }

    pub fn offset(&self) -> u64 {
        (self.page.saturating_sub(1)) * self.per_page
    }
}

/// Health check response.
#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
    pub version: &'static str,
    pub database: bool,
    pub mcp_mikareno: bool,
    pub mcp_genoffice: bool,
}

/// Public configuration response (no secrets).
#[derive(Debug, Clone, Serialize)]
pub struct PublicConfig {
    pub openrouter_configured: bool,
    pub openrouter_model: String,
    pub mikareno_mcp_url: String,
    pub genoffice_mcp_url: String,
}
