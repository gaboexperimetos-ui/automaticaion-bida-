//! MCP client for Mikareno (read-only academic data)
//!
//! A read-only MCP client that speaks the MCP-over-SSE protocol spoken by the
//! Mikareno student server (`http://127.0.0.1:8080/mcp`). It handles
//! `initialize` (session-id harvesting), `tools/list`, `tools/call`, TTL-backed
//! response caching, retries with backoff, 30s request timeouts, and exposes
//! typed adapters for `calendario`, `horario`, `perfil`, `tareas` and `notas`.
//!
//! Agent A implementation. Agent B (dashboard) consumes the public API below.

mod adapters;
mod cache;
mod client;
mod models;

pub use client::{MikarenoClient, MikarenoError, DEFAULT_CACHE_TTL_SECS};
pub use models::{
    CalendarioEvent, Horario, HorarioDia, HorarioSlot, Nota, Perfil, Tarea, ToolInfo,
};
