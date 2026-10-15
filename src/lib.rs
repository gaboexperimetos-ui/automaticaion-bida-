//! Study Platform — Academic Productivity Web Platform
//!
//! A personal academic productivity platform with:
//! - MCP client integration (Mikareno — read-only, future phase)
//! - AI assistant via OpenRouter (application LLM)
//! - Markdown notes (Obsidian-compatible, FTS5 search)
//! - Web office suite (optional proxy to GenOffice MCP)
//!
//! Built with Rust + Axum + SQLite.
//! Development model: poolside/laguna-s-2.1:free
//! Production build: musl static target for Ubuntu compatibility (ADR-010)

pub mod config;
pub mod db;
pub mod handlers;
pub mod middleware;
pub mod models;
pub mod ui;

// Mission 7 modules (orquestator adds these declarations)
#[allow(unused)]
pub mod academic;
#[allow(unused)]
pub mod ai;
#[allow(unused)]
pub mod integrations;
#[allow(unused)]
pub mod knowledge;
#[allow(unused)]
pub mod mcp_server;
#[allow(unused)]
pub mod office;

pub use config::Config;

/// Shared application state (cloned per-request via Arc).
#[derive(Clone)]
pub struct AppState {
    pub config: Config,
    pub db: sqlx::SqlitePool,
}
