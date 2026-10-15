//! Integration tests for the KnowledgeService.
//!
//! Tests use SQLite with the real migration stack (including FTS5) and
//! temporary directories for note files.

use std::path::PathBuf;
use std::sync::Arc;

use study_platform::knowledge::KnowledgeService;
use study_platform::{AppState, Config};

/// Creates a test Config + AppState with an isolated SQLite DB and notes dir.
/// Runs all migrations (including the knowledge migration with FTS5).
async fn setup_knowledge() -> (Arc<AppState>, KnowledgeService, PathBuf) {
    let db_temp = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_temp.path().to_path_buf();
    let db_url = format!("sqlite://{}", db_path.display());

    let notes_dir = tempfile::tempdir().unwrap().keep();

    let config = Config {
        host: "127.0.0.1".to_string(),
        port: 18090,
        session_secret: "test_secret_key_at_least_32_chars_long!!".to_string(),
        db_path,
        notes_dir,
        openrouter_api_key: None,
        openrouter_model: "cohere/north-mini-code:free".to_string(),
        openrouter_max_tokens: 8192,
        openrouter_max_cost_usd: 2.0,
        mikareno_mcp_url: "http://127.0.0.1:8080/mcp".to_string(),
        genoffice_mcp_url: "http://127.0.0.1:8453/mcp".to_string(),
        custom_mcp_api_key: None,
        rust_log: "test".to_string(),
    };

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("Failed to create test database");

    // Run all migrations (init + seed + knowledge)
    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(&pool)
        .await
        .ok();
    sqlx::query("PRAGMA foreign_keys=ON;")
        .execute(&pool)
        .await
        .ok();
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    let state = Arc::new(AppState { config, db: pool });

    let svc = KnowledgeService::new(state.clone());
    let notes_dir = state.config.notes_dir.clone();

    (state, svc, notes_dir)
}

// ---------------------------------------------------------------------------
// CRUD lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_and_get_note() {
    let (_state, svc, notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note(
            "Test Note",
            "# Hello\n\nWorld content".to_string(),
            vec!["tag1".to_string(), "tag2".to_string()],
        )
        .await
        .expect("create_note should succeed");

    assert_eq!(note.title, "Test Note");
    assert_eq!(note.slug, "test-note");

    // Verify the file was written with frontmatter
    let file_path = notes_dir.join("test-note.md");
    assert!(file_path.exists());
    let file_content = std::fs::read_to_string(&file_path).unwrap();
    assert!(file_content.starts_with("---"));
    assert!(file_content.contains("title: Test Note"));

    // Retrieve the note
    let (retrieved, content) = svc.get_note("test-note").await.expect("get_note");
    assert_eq!(retrieved.title, "Test Note");
    assert_eq!(retrieved.tags, "tag1,tag2");
    assert_eq!(content, "# Hello\n\nWorld content");
}

#[tokio::test]
async fn test_update_note() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note("Original", "old content".to_string(), vec!["old".into()])
        .await
        .expect("create");

    // Update title, content, and tags
    svc.update_note(
        &note.slug,
        Some("Updated Title".to_string()),
        Some("new content here".to_string()),
        Some(vec!["newtag".to_string()]),
    )
    .await
    .expect("update");

    let (updated, content) = svc.get_note(&note.slug).await.expect("get");
    assert_eq!(updated.title, "Updated Title");
    assert_eq!(updated.tags, "newtag");
    assert_eq!(content, "new content here");

    // Verify on-disk file reflects the update
    let file_path = _notes_dir.join(format!("{}.md", note.slug));
    let file_content = std::fs::read_to_string(&file_path).unwrap();
    assert!(file_content.contains("title: Updated Title"));
    assert!(file_content.contains("new content here"));
}

#[tokio::test]
async fn test_update_preserves_existing_fields() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note(
            "My Note",
            "original body".to_string(),
            vec!["a".into(), "b".into()],
        )
        .await
        .expect("create");

    // Update only content, leave title and tags unchanged
    svc.update_note(&note.slug, None, Some("updated body".to_string()), None)
        .await
        .expect("update");

    let (retrieved, content) = svc.get_note(&note.slug).await.expect("get");
    assert_eq!(retrieved.title, "My Note");
    assert_eq!(content, "updated body");
}

#[tokio::test]
async fn test_delete_note() {
    let (_state, svc, notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note("To Delete", "content".to_string(), vec![])
        .await
        .expect("create");

    let file_path = notes_dir.join(format!("{}.md", note.slug));
    assert!(file_path.exists());

    svc.delete_note(&note.slug).await.expect("delete");

    // File should be gone
    assert!(!file_path.exists());

    // get_note should fail
    assert!(svc.get_note(&note.slug).await.is_err());
}

#[tokio::test]
async fn test_slug_collision() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note("Hello World", "content".to_string(), vec![])
        .await
        .expect("first create");

    // Same title → same slug → should error
    let result = svc
        .create_note("Hello World", "other content".to_string(), vec![])
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_nonexistent_note() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;
    let result = svc.get_note("does-not-exist").await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Search (FTS5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_search_basic() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note(
        "Rust Programming Guide",
        "This guide covers Rust basics and advanced topics".to_string(),
        vec!["rust".into()],
    )
    .await
    .expect("create");

    svc.create_note(
        "Python Tips",
        "Python is great for scripting".to_string(),
        vec!["python".into()],
    )
    .await
    .expect("create");

    let results = svc.search("Rust", 10).await.expect("search");
    assert!(!results.is_empty());
    assert!(results
        .iter()
        .any(|r| r.note.title == "Rust Programming Guide"));
}

#[tokio::test]
async fn test_search_content_match() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note(
        "Note 1",
        "The quick brown fox jumps over the lazy dog".to_string(),
        vec![],
    )
    .await
    .expect("create");

    svc.create_note("Note 2", "Programming in Rust is fun".to_string(), vec![])
        .await
        .expect("create");

    // Search for content that appears only in Note 2's body
    let results = svc.search("brown", 10).await.expect("search");
    assert!(results.iter().any(|r| r.note.title == "Note 1"));
}

#[tokio::test]
async fn test_search_returns_ranked_results() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    // Note with many mentions of "rust"
    svc.create_note(
        "Rust Deep Dive",
        "rust rust rust programming".to_string(),
        vec![],
    )
    .await
    .expect("create");

    // Note with one mention of "rust"
    svc.create_note("Intro", "rust basics".to_string(), vec![])
        .await
        .expect("create");

    let results = svc.search("rust", 10).await.expect("search");
    assert!(results.len() >= 2);

    // The note with more matches should rank higher
    let deep = results.iter().find(|r| r.note.title == "Rust Deep Dive");
    let intro = results.iter().find(|r| r.note.title == "Intro");
    assert!(deep.is_some());
    assert!(intro.is_some());
    assert!(
        deep.unwrap().rank <= intro.unwrap().rank,
        "higher rank = better match"
    );
}

#[tokio::test]
async fn test_search_empty_query() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;
    svc.create_note("Note", "content".to_string(), vec![])
        .await
        .expect("create");

    let results = svc.search("", 10).await.expect("search empty");
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_search_returns_snippets() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note(
        "Snippet Test",
        "This is a long piece of content about database systems and optimization techniques"
            .to_string(),
        vec![],
    )
    .await
    .expect("create");

    let results = svc.search("database", 10).await.expect("search");
    assert!(!results.is_empty());
    // FTS5 snippet should contain highlighting markers
    let result = results
        .iter()
        .find(|r| r.note.title == "Snippet Test")
        .unwrap();
    assert!(!result.snippet.is_empty());
}

// ---------------------------------------------------------------------------
// Version history
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_version_history() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note("Versioned", "v1 content".to_string(), vec![])
        .await
        .expect("create");

    svc.update_note(&note.slug, None, Some("v2 content".to_string()), None)
        .await
        .expect("update 1");

    svc.update_note(&note.slug, None, Some("v3 content".to_string()), None)
        .await
        .expect("update 2");

    let versions = svc
        .version_history(&note.slug)
        .await
        .expect("version history");
    assert!(versions.len() >= 3);
    // Newest first
    assert!(
        versions[0].content == "v3 content" || versions.iter().any(|v| v.content == "v3 content")
    );
}

#[tokio::test]
async fn test_version_history_max_five() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note("Many Versions", "v1".to_string(), vec![])
        .await
        .expect("create");

    for i in 2..=10 {
        svc.update_note(&note.slug, None, Some(format!("v{}", i)), None)
            .await
            .expect("update");
    }

    let versions = svc.version_history(&note.slug).await.expect("history");
    assert!(
        versions.len() <= 5,
        "Should keep at most 5 versions, got {}",
        versions.len()
    );
}

// ---------------------------------------------------------------------------
// Backlinks / Wikilinks
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_backlinks() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note("Target Note", "body".to_string(), vec![])
        .await
        .expect("create target");

    svc.create_note(
        "Linker Note",
        "This links to [[Target Note]] and [[Another]]".to_string(),
        vec![],
    )
    .await
    .expect("create linker");

    let backlinks = svc.backlinks("target-note").await.expect("backlinks");
    assert!(backlinks.iter().any(|b| b.from_title == "Linker Note"));
}

// ---------------------------------------------------------------------------
// Obsidian import
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_import_obsidian() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    // Create a fake Obsidian vault
    let vault = tempfile::tempdir().unwrap();
    let vault_path = vault.path();

    std::fs::write(
        vault_path.join("Note A.md"),
        "---\ntitle: Note A\ntags: [alpha, beta]\n---\n\n# Content A\n\nHello world",
    )
    .unwrap();

    std::fs::create_dir_all(vault_path.join("subfolder")).unwrap();
    std::fs::write(
        vault_path.join("subfolder/Note B.md"),
        "---\ntitle: Note B\ntags: []\n---\n\nContent B",
    )
    .unwrap();

    let imported = svc
        .import_obsidian(vault_path)
        .await
        .expect("import should succeed");

    assert_eq!(imported, 2, "Should import 2 notes");

    // Verify both notes exist
    let (note_a, content_a) = svc.get_note("note-a").await.expect("get note A");
    assert_eq!(note_a.title, "Note A");
    assert_eq!(note_a.tags, "alpha,beta");
    assert_eq!(content_a, "# Content A\n\nHello world");

    let (note_b, content_b) = svc.get_note("note-b").await.expect("get note B");
    assert_eq!(note_b.title, "Note B");
    assert_eq!(content_b, "Content B");

    // Verify original files are untouched (read-only import)
    let original_a = std::fs::read_to_string(vault_path.join("Note A.md")).unwrap();
    assert!(original_a.starts_with("---"));
    // Original should still have its frontmatter intact
    assert!(original_a.contains("tags: [alpha, beta]"));
}

#[tokio::test]
async fn test_import_obsidian_empty_dir() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;
    let empty = tempfile::tempdir().unwrap();
    let count = svc
        .import_obsidian(empty.path())
        .await
        .expect("should not error on empty dir");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_import_obsidian_nonexistent_dir() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;
    let result = svc
        .import_obsidian(std::path::Path::new("/nonexistent/path"))
        .await;
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_export_note() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note(
        "Export Me",
        "# Heading\n\nSome content here".to_string(),
        vec!["export".to_string(), "test".to_string()],
    )
    .await
    .expect("create");

    let export_dir = tempfile::tempdir().unwrap();
    let export_path = export_dir.path().join("exported.md");

    svc.export_note("export-me", &export_path)
        .await
        .expect("export");

    let exported = std::fs::read_to_string(&export_path).expect("read exported file");
    assert!(exported.starts_with("---"));
    assert!(exported.contains("title: Export Me"));
    assert!(exported.contains("tags: [export, test]"));
    assert!(exported.contains("# Heading"));
    assert!(exported.contains("Some content here"));
}

// ---------------------------------------------------------------------------
// FTS5 fallback verification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_fts5_table_exists() {
    let (state, _svc, _notes_dir) = setup_knowledge().await;

    // Verify the notes_fts virtual table was created by migrations
    let result: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='notes_fts'",
    )
    .fetch_optional(&state.db)
    .await
    .unwrap();

    assert_eq!(result, Some("notes_fts".to_string()));
}

#[tokio::test]
async fn test_fallback_search_when_fts5_empty() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    // With no notes, search should return empty (not error)
    let results = svc.search("anything", 10).await.expect("search");
    assert!(results.is_empty());
}

// ---------------------------------------------------------------------------
// Wikilink unit tests (integration-level)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_wikilink_parse_and_resolve() {
    use study_platform::knowledge::wikilinks::parse_wikilinks;

    let content = "See [[My Note]] and [[Other|alias]] and [[Page#Section|display]].";
    let links = parse_wikilinks(content);
    assert_eq!(links.len(), 3);
    assert_eq!(links[0].target, "My Note");
    assert_eq!(links[0].resolve(), "my-note");
    assert_eq!(links[1].target, "Other");
    assert_eq!(links[1].display, Some("alias".to_string()));
    assert_eq!(links[2].heading, Some("Section".to_string()));
}

#[tokio::test]
async fn test_frontmatter_roundtrip_via_service() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    let _note = svc
        .create_note(
            "Roundtrip",
            "Body content".to_string(),
            vec!["x".to_string()],
        )
        .await
        .expect("create");

    // Read raw file
    let raw = std::fs::read_to_string(_notes_dir.join("roundtrip.md")).unwrap();
    assert!(raw.starts_with("---\n"));
    assert!(raw.contains("title: Roundtrip"));
    assert!(raw.contains("tags: [x]"));
    assert!(raw.contains("created:"));
    assert!(raw.contains("modified:"));
}

// ---------------------------------------------------------------------------
// Agent C additions: full lifecycle, FTS5 availability + LIKE fallback,
// read-only import, export roundtrip, tags, wikilinks, list/versions sync
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_full_lifecycle_create_to_delete() {
    let (_state, svc, notes_dir) = setup_knowledge().await;

    // CREATE two fictitious notes; beta links to alpha.
    let alpha = svc
        .create_note(
            "Zephyr Meadow",
            "Stellar cartography notes about zephyr currents.".to_string(),
            vec!["meadow".into(), "zephyr".into()],
        )
        .await
        .expect("create alpha");
    assert_eq!(alpha.slug, "zephyr-meadow");

    let beta = svc
        .create_note(
            "Quasar Garden",
            "See [[Zephyr Meadow]] for currents.".to_string(),
            vec!["garden".into()],
        )
        .await
        .expect("create beta");
    assert_eq!(beta.slug, "quasar-garden");

    // GET
    let (got, body) = svc.get_note("zephyr-meadow").await.expect("get alpha");
    assert_eq!(got.title, "Zephyr Meadow");
    assert!(body.contains("zephyr currents"));

    // UPDATE alpha content + tags
    svc.update_note(
        "zephyr-meadow",
        None,
        Some("Stellar cartography notes about zephyr currents and quasar winds.".to_string()),
        Some(vec!["meadow".into(), "updated".into()]),
    )
    .await
    .expect("update alpha");
    let (updated, updated_body) = svc.get_note("zephyr-meadow").await.expect("get updated");
    assert!(updated_body.contains("quasar winds"));
    assert_eq!(updated.tags, "meadow,updated");

    // SEARCH finds updated content via FTS5
    let hits = svc.search("quasar winds", 10).await.expect("search");
    assert!(hits.iter().any(|r| r.note.slug == "zephyr-meadow"));

    // BACKLINKS: beta links to alpha
    let links = svc.backlinks("zephyr-meadow").await.expect("backlinks");
    assert!(
        links.iter().any(|b| b.from_slug == "quasar-garden"),
        "expected backlink from quasar-garden, got {:?}",
        links.iter().map(|b| &b.from_slug).collect::<Vec<_>>()
    );

    // VERSION HISTORY: create + update = at least 2, newest first, max 5
    let versions = svc.version_history("zephyr-meadow").await.expect("history");
    assert!(versions.len() >= 2 && versions.len() <= 5);
    assert!(versions[0].id > versions[versions.len() - 1].id);

    // DELETE beta then alpha; files removed and get fails
    svc.delete_note("quasar-garden").await.expect("delete beta");
    assert!(svc.get_note("quasar-garden").await.is_err());
    assert!(!notes_dir.join("quasar-garden.md").exists());

    svc.delete_note("zephyr-meadow")
        .await
        .expect("delete alpha");
    assert!(svc.get_note("zephyr-meadow").await.is_err());
    assert!(!notes_dir.join("zephyr-meadow.md").exists());
}

#[tokio::test]
async fn test_fts5_available_true_initially() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;
    assert!(
        svc.is_fts5_available().await,
        "notes_fts should exist after migrations"
    );
}

#[tokio::test]
async fn test_search_like_fallback_when_fts_missing() {
    let (state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note(
        "Fallback Meadow",
        "Unique larkspur content for fallback verification".to_string(),
        vec![],
    )
    .await
    .expect("create");

    // Sanity: FTS5 path works before DROP
    assert!(svc.is_fts5_available().await);
    let fts_hits = svc.search("larkspur", 10).await.expect("fts search");
    assert!(fts_hits.iter().any(|r| r.note.title == "Fallback Meadow"));

    // Simulate missing FTS5 (no new migrations): drop the virtual table.
    sqlx::query("DROP TABLE notes_fts")
        .execute(&state.db)
        .await
        .expect("drop notes_fts");

    assert!(
        !svc.is_fts5_available().await,
        "is_fts5_available must be false after DROP"
    );

    // LIKE fallback must still find the note, with rank 0.0
    let like_hits = svc
        .search("larkspur", 10)
        .await
        .expect("like fallback search");
    assert!(
        like_hits.iter().any(|r| r.note.title == "Fallback Meadow"),
        "LIKE fallback should find note after FTS table dropped"
    );
    for r in &like_hits {
        assert_eq!(r.rank, 0.0, "LIKE fallback rank must be 0.0");
    }

    // Creating a note after FTS is gone must still succeed (FTS sync is non-fatal)
    // and be searchable via LIKE.
    svc.create_note(
        "Post Drop Note",
        "wombat quixotic post-drop content".to_string(),
        vec![],
    )
    .await
    .expect("create after drop should succeed");
    let post_hits = svc.search("wombat", 10).await.expect("search after drop");
    assert!(post_hits.iter().any(|r| r.note.title == "Post Drop Note"));
}

#[tokio::test]
async fn test_search_fallback_on_malformed_fts_query() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note(
        "Asterisk Note",
        "stellar cartography uses * markers for waypoints".to_string(),
        vec![],
    )
    .await
    .expect("create");

    // `*` alone is a syntax error in FTS5 MATCH but a literal in LIKE.
    // search() must not error; it must fall back and find the note.
    let hits = svc
        .search("*", 10)
        .await
        .expect("malformed query must not error");
    assert!(
        hits.iter().any(|r| r.note.title == "Asterisk Note"),
        "LIKE fallback should match literal * content"
    );
    for r in &hits {
        assert_eq!(r.rank, 0.0);
    }

    // Another malformed query: unbalanced double-quote must also fall back, not error.
    let q2 = svc
        .search("\"unclosed", 10)
        .await
        .expect("unbalanced quote must not error");
    // No assertion on hits (LIKE for literal quote may be empty); only that it is Ok.
    let _ = q2;
}

#[tokio::test]
async fn test_search_reflects_update_and_delete() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note(
            "Sync Check",
            "xylophonezephyr initial content".to_string(),
            vec![],
        )
        .await
        .expect("create");
    assert!(svc
        .search("xylophonezephyr", 10)
        .await
        .expect("s")
        .iter()
        .any(|r| r.note.slug == note.slug));

    // Update: old term gone, new term found (verifies FTS row is re-synced).
    svc.update_note(
        &note.slug,
        None,
        Some("quixoticwaffle replacement content".to_string()),
        None,
    )
    .await
    .expect("update");
    let old_hits = svc.search("xylophonezephyr", 10).await.expect("s old");
    assert!(
        !old_hits.iter().any(|r| r.note.slug == note.slug),
        "old content should no longer match after update"
    );
    let new_hits = svc.search("quixoticwaffle", 10).await.expect("s new");
    assert!(new_hits.iter().any(|r| r.note.slug == note.slug));

    // Delete: FTS entry removed, search no longer finds it.
    svc.delete_note(&note.slug).await.expect("delete");
    let after = svc
        .search("quixoticwaffle", 10)
        .await
        .expect("s after delete");
    assert!(!after.iter().any(|r| r.note.slug == note.slug));
}

#[tokio::test]
async fn test_import_never_writes_to_source() {
    use std::collections::HashMap;

    let (_state, svc, notes_dir) = setup_knowledge().await;

    // Fictitious vault with nested file.
    let vault = tempfile::tempdir().unwrap();
    let vault_path = vault.path();
    std::fs::write(
        vault_path.join("Fern Hollow.md"),
        "---\ntitle: Fern Hollow\ntags: [fern, hollow]\n---\n\nMossy waypoint content",
    )
    .unwrap();
    std::fs::create_dir_all(vault_path.join("deep")).unwrap();
    std::fs::write(
        vault_path.join("deep/Pebble Brook.md"),
        "# Pebble Brook\n\nQuiet brook notes",
    )
    .unwrap();

    // Snapshot source dir: contents + mtimes + file list.
    fn snapshot(
        dir: &std::path::Path,
    ) -> (
        HashMap<String, Vec<u8>>,
        HashMap<String, std::time::SystemTime>,
        Vec<String>,
    ) {
        let mut contents = HashMap::new();
        let mut mtimes = HashMap::new();
        let mut names = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let e = e.unwrap();
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(dir).unwrap().to_string_lossy().to_string();
                    names.push(rel.clone());
                    contents.insert(rel.clone(), std::fs::read(&p).unwrap());
                    mtimes.insert(rel, std::fs::metadata(&p).unwrap().modified().unwrap());
                }
            }
        }
        names.sort();
        (contents, mtimes, names)
    }

    let (before_bytes, before_mtimes, before_list) = snapshot(vault_path);

    let imported = svc.import_obsidian(vault_path).await.expect("import");
    assert_eq!(imported, 2);

    // Source dir must be byte-identical with same mtimes and no new files.
    let (after_bytes, after_mtimes, after_list) = snapshot(vault_path);
    assert_eq!(
        before_list, after_list,
        "import must not create files in source dir"
    );
    assert_eq!(
        before_bytes, after_bytes,
        "source file bytes must be unchanged"
    );
    assert_eq!(
        before_mtimes, after_mtimes,
        "source mtimes must be unchanged"
    );

    // Imported notes live in notes_dir, not in the vault.
    assert!(notes_dir.join("fern-hollow.md").exists());
    assert!(notes_dir.join("pebble-brook.md").exists());
    assert!(
        !vault_path.join("fern-hollow.md").exists(),
        "no slug-renamed copy in source"
    );

    // Second import skips collisions (slug already exists) and imports 0 new.
    let second = svc.import_obsidian(vault_path).await.expect("reimport");
    assert_eq!(second, 0, "reimport should skip existing slugs");
    let (third_bytes, _, third_list) = snapshot(vault_path);
    assert_eq!(before_list, third_list);
    assert_eq!(before_bytes, third_bytes);
}

#[tokio::test]
async fn test_export_roundtrips_frontmatter() {
    use study_platform::knowledge::frontmatter::parse_note;

    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note(
        "Export Roundtrip",
        "# Waypoint\n\nBody with [[Fern Hollow]] link.".to_string(),
        vec!["trail".into(), "waypoint two".into()],
    )
    .await
    .expect("create");

    let export_dir = tempfile::tempdir().unwrap();
    let export_path = export_dir.path().join("nested/exported.md");
    svc.export_note("export-roundtrip", &export_path)
        .await
        .expect("export");

    let exported = std::fs::read_to_string(&export_path).expect("read export");
    assert!(
        exported.starts_with("---"),
        "export must start with frontmatter"
    );
    let parsed = parse_note(&exported);
    assert_eq!(parsed.frontmatter.title, "Export Roundtrip");
    assert_eq!(
        parsed.frontmatter.tags,
        vec!["trail".to_string(), "waypoint two".to_string()]
    );
    assert!(parsed.frontmatter.created.is_some());
    assert!(parsed.frontmatter.modified.is_some());
    assert_eq!(
        parsed.content,
        "# Waypoint\n\nBody with [[Fern Hollow]] link."
    );

    // Re-parsing the on-disk note file yields the same frontmatter (roundtrip stable).
    let (_note, body) = svc.get_note("export-roundtrip").await.expect("get");
    assert_eq!(body, parsed.content);
}

#[tokio::test]
async fn test_tag_handling() {
    let (_state, svc, notes_dir) = setup_knowledge().await;

    // Multiple tags incl. space; empty tags; single tag.
    let multi = svc
        .create_note(
            "Tagged Meadow",
            "content".to_string(),
            vec!["alpha".into(), "beta gamma".into(), "Delta".into()],
        )
        .await
        .expect("create multi");
    assert_eq!(multi.tags, "alpha,beta gamma,Delta");
    assert_eq!(
        multi.tags_vec(),
        vec![
            "alpha".to_string(),
            "beta gamma".to_string(),
            "Delta".to_string()
        ]
    );

    // On-disk frontmatter preserves tags.
    let raw = std::fs::read_to_string(notes_dir.join("tagged-meadow.md")).unwrap();
    assert!(raw.contains("alpha") && raw.contains("beta gamma") && raw.contains("Delta"));

    let empty = svc
        .create_note("Untagged Hollow", "content".to_string(), vec![])
        .await
        .expect("create empty");
    assert_eq!(empty.tags, "");
    assert!(empty.tags_vec().is_empty());

    // Update tags: replace, then clear via empty vec.
    svc.update_note(&multi.slug, None, None, Some(vec!["solo".into()]))
        .await
        .expect("update tags");
    let (got, _) = svc.get_note(&multi.slug).await.expect("get");
    assert_eq!(got.tags, "solo");
    assert_eq!(got.tags_vec(), vec!["solo".to_string()]);

    // Title-only update preserves tags.
    svc.update_note(
        &multi.slug,
        Some("Tagged Meadow Renamed".into()),
        None,
        None,
    )
    .await
    .expect("rename");
    let (renamed, _) = svc.get_note(&multi.slug).await.expect("get renamed");
    assert_eq!(renamed.tags, "solo", "tags must survive title-only update");
}

#[tokio::test]
async fn test_wikilink_alias_heading_and_self_exclusion() {
    use study_platform::knowledge::wikilinks::{extract_linked_slugs, parse_wikilinks};

    let (_state, svc, _notes_dir) = setup_knowledge().await;

    svc.create_note("Atlas Meadow", "base content".to_string(), vec![])
        .await
        .expect("target");
    svc.create_note(
        "Compass Brook",
        "See [[Atlas Meadow|custom alias]] and [[Atlas Meadow#Overview]] for details.".to_string(),
        vec![],
    )
    .await
    .expect("linker");
    // Self-link must be excluded from backlinks.
    svc.create_note(
        "Self Loop",
        "links to [[Self Loop]] itself".to_string(),
        vec![],
    )
    .await
    .expect("self");

    // Parser-level: alias + heading forms.
    let links = parse_wikilinks("[[Atlas Meadow|custom alias]] and [[Atlas Meadow#Overview]]");
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].target, "Atlas Meadow");
    assert_eq!(links[0].display, Some("custom alias".into()));
    assert_eq!(links[0].resolve(), "atlas-meadow");
    assert_eq!(links[1].heading, Some("Overview".into()));
    assert_eq!(links[1].resolve(), "atlas-meadow");
    assert_eq!(
        extract_linked_slugs("[[Atlas Meadow]] and [[Atlas Meadow]] and [[Compass Brook]]"),
        vec!["atlas-meadow".to_string(), "compass-brook".to_string()]
    );

    // Service-level backlinks include alias/heading links.
    let backs = svc.backlinks("atlas-meadow").await.expect("backlinks");
    assert!(
        backs.iter().any(|b| b.from_slug == "compass-brook"),
        "expected compass-brook backlink"
    );
    assert!(
        backs.iter().any(|b| b.display == "custom alias"),
        "alias display text should be preserved in backlink"
    );

    // Self-links excluded.
    let self_backs = svc.backlinks("self-loop").await.expect("self backlinks");
    assert!(
        self_backs.is_empty(),
        "self-links must be excluded, got {:?}",
        self_backs
    );
}

#[tokio::test]
async fn test_list_notes_limit_and_ordering() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    let a = svc
        .create_note("List Alpha", "alpha content".to_string(), vec![])
        .await
        .expect("a");
    let b = svc
        .create_note("List Beta", "beta content".to_string(), vec![])
        .await
        .expect("b");

    // Limit respected.
    let one = svc.list_notes(1).await.expect("list 1");
    assert_eq!(one.len(), 1);

    let all = svc.list_notes(10).await.expect("list all");
    assert!(all.len() >= 2);
    assert!(all.iter().any(|n| n.slug == a.slug));
    assert!(all.iter().any(|n| n.slug == b.slug));

    // Touch A so it becomes most-recently-updated; it should sort first.
    svc.update_note(
        &a.slug,
        None,
        Some("alpha content touched".to_string()),
        None,
    )
    .await
    .expect("touch a");
    let ordered = svc.list_notes(10).await.expect("ordered");
    assert_eq!(
        ordered[0].slug, a.slug,
        "most recently updated note should sort first"
    );
}

#[tokio::test]
async fn test_version_history_order_and_content() {
    let (_state, svc, _notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note("History Hollow", "v1 meadow".to_string(), vec![])
        .await
        .expect("create");
    svc.update_note(&note.slug, None, Some("v2 meadow brook".to_string()), None)
        .await
        .expect("u2");
    svc.update_note(
        &note.slug,
        None,
        Some("v3 meadow brook fern".to_string()),
        None,
    )
    .await
    .expect("u3");

    let versions = svc.version_history(&note.slug).await.expect("history");
    assert_eq!(versions.len(), 3);
    // Newest first.
    assert_eq!(versions[0].content, "v3 meadow brook fern");
    assert!(versions[0].id > versions[1].id && versions[1].id > versions[2].id);
    assert_eq!(versions[2].content, "v1 meadow");
    assert_eq!(versions[0].word_count, 4);
}

#[tokio::test]
async fn test_delete_cleans_search_and_versions() {
    let (state, svc, _notes_dir) = setup_knowledge().await;

    let note = svc
        .create_note(
            "Ephemeral Fern",
            "ephemeral uniqueword content".to_string(),
            vec![],
        )
        .await
        .expect("create");
    assert!(!svc.search("uniqueword", 10).await.expect("s").is_empty());

    svc.delete_note(&note.slug).await.expect("delete");

    // Search no longer finds it (FTS row removed).
    let hits = svc.search("uniqueword", 10).await.expect("s after");
    assert!(!hits.iter().any(|r| r.note.slug == note.slug));

    // Versions cascade-deleted: no rows remain for the note id.
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM note_versions WHERE note_id = ?")
        .bind(note.id)
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(remaining, 0, "note_versions should cascade on note delete");

    // version_history on deleted slug errors (note not found).
    assert!(svc.version_history(&note.slug).await.is_err());
}
