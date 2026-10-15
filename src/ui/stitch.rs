//! Shared Stitch "Synthetic Cognition" UI primitives.
//!
//! The canonical Tailwind token set + base CSS + `<head>` live in
//! `academic::templates::head` (the single source of truth for the design
//! system). This module re-exports `head` and `html_escape` so every handler
//! — `auth`, `ai`, `office`, `notes` — renders with identical tokens instead
//! of duplicating CSS blocks.

pub use crate::academic::templates::{head, html_escape};

/// Convenience: build a full Stitch document around a body.
///
/// `head` only emits `<head>`; callers open `<body>` themselves so each page
/// can keep its own body classes/layout while sharing the head.
pub fn page(title: &str, body: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html class=\"dark\" lang=\"es\">\n{}<body class=\"bg-surface text-on-surface font-body-md antialiased min-h-screen\">\n{}\n</body>\n</html>",
        head(title),
        body
    )
}
