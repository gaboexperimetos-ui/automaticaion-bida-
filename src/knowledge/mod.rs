//! Second brain: Markdown notes, Obsidian compatibility, and FTS5 search.
//!
//! ## Architecture
//!
//! Notes are stored as Markdown files on disk (Obsidian-compatible) with
//! metadata in SQLite. FTS5 provides full-text search on title + content.
//!
//! - [`notes`] — CRUD service (create, read, update, delete)
//! - [`search`] — FTS5 search with LIKE-based fallback
//! - [`obsidian`] — safe import from Obsidian directories, export to Markdown
//! - [`wikilinks`] — `[[Note Name]]` link parsing and resolution
//! - [`frontmatter`] — YAML frontmatter parse/serialize (minimal, no extra deps)
//!
//! ## FTS5 availability
//!
//! `libsqlite3-sys` is built with the `bundled` feature (verified in build
//! fingerprints) which compiles SQLite with `-DSQLITE_ENABLE_FTS5`. The search
//! method attempts FTS5 first and transparently falls back to LIKE-based search
//! if the FTS5 table is missing or the query fails.
//!
//! ## Root cause of earlier "SQL logic error"
//!
//! The init migration (`20240101000000_init.sql`) defines `notes` with no
//! `content` column — content lives on disk as `.md` files. The earlier FTS5
//! attempt used triggers (`notes_ai`, `notes_ad`) that referenced
//! `new.content` / `old.content` — columns that do not exist — producing
//! `SQL logic error 1 (SQLITE_ERROR): no such column: content`.
//!
//! Fix: standalone FTS5 virtual table (no external content, no triggers).
//! The `KnowledgeService` syncs the index from application code on every CRUD
//! operation.

pub mod frontmatter;
pub mod markdown;
pub mod notes;
mod obsidian;
pub mod search;
pub mod wikilinks;

use std::path::Path;
use std::sync::Arc;

use serde::Serialize;

use crate::{models::Note, AppState};

/// A search result with FTS5 ranking and content snippet.
#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub note: Note,
    pub snippet: String,
    pub rank: f64,
}

/// Knowledge management service for the second brain.
///
/// Provides full CRUD for Markdown notes, Obsidian-compatible format
/// (YAML frontmatter, wikilinks), FTS5 search, and safe Obsidian import.
pub struct KnowledgeService {
    pub state: Arc<AppState>,
}

impl KnowledgeService {
    /// Create a new `KnowledgeService` bound to the given application state.
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}
