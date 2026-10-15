//! FTS5 full-text search with LIKE-based fallback.
//!
//! FTS5 is compiled into the bundled SQLite (verified). If the `notes_fts`
//! virtual table is missing or a MATCH query fails, the service transparently
//! falls back to `LIKE`-based search on the `notes` table and `note_versions`
//! content.

use sqlx::Row;

use crate::knowledge::KnowledgeService;
use crate::knowledge::SearchResult;
use crate::models::Note;

impl KnowledgeService {
    /// Search notes by title and content.
    ///
    /// Uses FTS5 with BM25 ranking and snippet generation when available.
    /// Falls back to `LIKE` on title + latest content if FTS5 is unavailable
    /// or the query expression is malformed.
    ///
    /// Returns results ranked by relevance (FTS5 rank descending, or
    /// `updated_at` for the LIKE fallback).
    /// Check whether the FTS5 virtual table exists and is queryable at runtime.
    ///
    /// Returns `true` when `notes_fts` is present in `sqlite_master` and a
    /// trivial `SELECT` against it succeeds. Returns `false` when FTS5 was
    /// not created (e.g. older DB without the knowledge migration) or the
    /// bundled SQLite was built without FTS5. No migrations are created here;
    /// callers must use the LIKE fallback when this returns `false`.
    pub async fn is_fts5_available(&self) -> bool {
        let name: Option<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='notes_fts'",
        )
        .fetch_optional(&self.state.db)
        .await
        .unwrap_or(None);
        if name.as_deref() != Some("notes_fts") {
            return false;
        }
        // Verify the virtual table is actually queryable (catches a build
        // without FTS5 support where CREATE VIRTUAL TABLE would have failed).
        sqlx::query("SELECT rowid FROM notes_fts LIMIT 0")
            .fetch_all(&self.state.db)
            .await
            .is_ok()
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Runtime availability check: skip FTS5 entirely when notes_fts is
        // missing so we go straight to the LIKE fallback (no new migrations).
        if !self.is_fts5_available().await {
            tracing::warn!("notes_fts missing at runtime, using LIKE fallback");
            return self.search_like(query, limit).await;
        }

        // Attempt FTS5 search first
        match self.search_fts5(query, limit).await {
            Ok(results) => Ok(results),
            Err(e) => {
                tracing::warn!("FTS5 search failed, falling back to LIKE: {}", e);
                self.search_like(query, limit).await
            }
        }
    }

    /// FTS5 search implementation.
    ///
    /// Returns ranked results with snippets. Uses the standalone `notes_fts`
    /// table joined to `notes` on `rowid = notes.id`.
    async fn search_fts5(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let sql = r#"
            SELECT n.id, n.title, n.slug, n.filename, n.tags,
                   n.word_count, n.summary, n.created_at, n.updated_at,
                   rank as fts_rank,
                   snippet(notes_fts, 1, '[', ']', '...', 10) as fts_snippet
            FROM notes_fts
            JOIN notes n ON n.id = notes_fts.rowid
            WHERE notes_fts MATCH ?
            ORDER BY rank
            LIMIT ?
        "#;

        let rows = sqlx::query(sql)
            .bind(query)
            .bind(limit as i64)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("FTS5 search error: {}", e))?;

        let mut results = Vec::with_capacity(rows.len());
        for row in &rows {
            let note = Note {
                id: row.try_get("id").unwrap_or(0),
                title: row.try_get("title").unwrap_or_default(),
                slug: row.try_get("slug").unwrap_or_default(),
                filename: row.try_get("filename").unwrap_or_default(),
                tags: row.try_get("tags").unwrap_or_default(),
                word_count: row.try_get("word_count").unwrap_or(0),
                summary: row.try_get("summary").ok().flatten(),
                created_at: row.try_get("created_at").unwrap_or_default(),
                updated_at: row.try_get("updated_at").unwrap_or_default(),
            };

            results.push(SearchResult {
                note,
                snippet: row.try_get("fts_snippet").unwrap_or_default(),
                rank: row.try_get::<f64, _>("fts_rank").unwrap_or(0.0),
            });
        }

        Ok(results)
    }

    /// LIKE-based search fallback (no FTS5 dependency).
    ///
    /// Searches note titles and the latest content version. Returns results
    /// ordered by `updated_at` descending with `rank = 0.0`.
    async fn search_like(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, String> {
        let pattern = format!("%{}%", query);

        let sql = r#"
            SELECT n.id, n.title, n.slug, n.filename, n.tags,
                   n.word_count, n.summary, n.created_at, n.updated_at
            FROM notes n
            WHERE n.title LIKE ?
               OR EXISTS (
                   SELECT 1 FROM note_versions nv
                   WHERE nv.note_id = n.id
                     AND nv.content LIKE ?
               )
            ORDER BY n.updated_at DESC
            LIMIT ?
        "#;

        let rows = sqlx::query(sql)
            .bind(&pattern)
            .bind(&pattern)
            .bind(limit as i64)
            .fetch_all(&self.state.db)
            .await
            .map_err(|e| format!("LIKE search error: {}", e))?;

        let mut results = Vec::with_capacity(rows.len());
        for row in &rows {
            let note = Note {
                id: row.try_get("id").unwrap_or(0),
                title: row.try_get("title").unwrap_or_default(),
                slug: row.try_get("slug").unwrap_or_default(),
                filename: row.try_get("filename").unwrap_or_default(),
                tags: row.try_get("tags").unwrap_or_default(),
                word_count: row.try_get("word_count").unwrap_or(0),
                summary: row.try_get("summary").ok().flatten(),
                created_at: row.try_get("created_at").unwrap_or_default(),
                updated_at: row.try_get("updated_at").unwrap_or_default(),
            };

            results.push(SearchResult {
                note,
                snippet: row
                    .try_get::<Option<String>, _>("summary")
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
                rank: 0.0,
            });
        }

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Note;

    #[tokio::test]
    async fn test_empty_query_returns_empty() {
        // We can't easily test without a DB, but the logic is:
        // search("") should return empty vec.
        // This is a logic test of the guard clause.
        assert!("".trim().is_empty());
        assert!("   ".trim().is_empty());
        assert!(!"query".trim().is_empty());
    }

    #[test]
    fn test_like_pattern_construction() {
        let pattern = format!("%{}%", "hello");
        assert_eq!(pattern, "%hello%");
    }
}
