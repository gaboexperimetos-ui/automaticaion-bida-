//! Session handoff reader — serves the project handoff markdown as a browsable page.
//!
//! This is a temporary route for the current session's work documentation.
//! It compiles the markdown (pulldown-cmark) into HTML at build time, so the
//! document travels inside the binary and does not depend on files at runtime.

use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse},
};
use pulldown_cmark::{html, Parser};
use std::sync::Arc;

use crate::AppState;

/// Compile-time content of the session handoff document.
const HANDOFF_CONTENT: &str = include_str!("../../STUDY_PLATFORM_HANDOFF.md");

/// Serve the handoff as a styled markdown page at GET /handoff.
pub async fn get_handoff(State(_state): State<Arc<AppState>>) -> impl IntoResponse {
    let md = HANDOFF_CONTENT;
    let parser = Parser::new(md);

    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);

    let rendered = format!(
        r#"<!DOCTYPE html>
<html class="dark" lang="es">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Study Platform — Handoff de sesión</title>
<link href="https://fonts.googleapis.com/css2?family=Manrope:wght@400..900&family=Fira+Code:wght@400;500&display=swap" rel="stylesheet">
<style>
body {{ background: #101419; color: #e0e2ea; font-family: Manrope, sans-serif; margin: 0; padding: 2rem; line-height: 1.6; }}
.container {{ max-width: 960px; margin: 0 auto; }}
h1, h2, h3 {{ color: #5eead4; margin-top: 1.5rem; }}
h1 {{ border-bottom: 1px solid #3c4946; padding-bottom: 0.5rem; }}
code {{ background: #1c2025; padding: 2px 6px; border-radius: 4px; font-family: 'Fira Code', monospace; font-size: 12px; }}
pre {{ background: #1c2025; padding: 1rem; border-radius: 8px; overflow-x: auto; }}
pre code {{ background: none; padding: 0; }}
blockquote {{ border-left: 3px solid #5eead4; margin-left: 0; padding-left: 1rem; color: #bbcac5; }}
table {{ border-collapse: collapse; width: 100%; margin: 1rem 0; }}
th, td {{ border: 1px solid #3c4946; padding: 8px 12px; text-align: left; }}
th {{ background: #1c2025; color: #5eead4; }}
a {{ color: #9ed753; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
</style>
</head>
<body>
<div class="container">{html}</div>
</body>
</html>"#,
        html = html_out
    );

    (StatusCode::OK, Html(rendered))
}
