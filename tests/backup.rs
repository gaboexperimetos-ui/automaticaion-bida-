//! SQLite backup/restore tests (Mission 11, Agent F).
//!
//! SQLx only (`VACUUM INTO`) — no `sqlite3` CLI, no paid calls, no network.
//! Every database lives under a per-test temp dir (under the OS temp dir);
//! the real application database (`data/app.db`) is never touched.
//! All rows are fictitious probe data created by the tests themselves.

use sqlx::sqlite::SqlitePoolOptions;

/// Guard: a scratch path must live under the temp dir and must never resolve
/// to the real application database.
fn assert_scratch_only(path: &std::path::Path) {
    let real_db = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("app.db");
    assert_ne!(path, &real_db, "must never touch the real app database");
    let tmp = std::env::temp_dir();
    assert!(
        path.starts_with(&tmp) || path.starts_with("/tmp"),
        "scratch DB must live under the temp dir, got {}",
        path.display()
    );
    // Even via symlinks / `..` segments the scratch file must not be the
    // real database.
    let scratch_canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let real_canon = real_db.canonicalize().unwrap_or(real_db);
    assert_ne!(
        scratch_canon, real_canon,
        "scratch DB must not resolve to the real app database"
    );
}

#[tokio::test]
async fn sqlite_backup_paths_never_touch_real_data() {
    let work = tempfile::tempdir().unwrap();
    assert_scratch_only(&work.path().join("scratch.db"));
    assert_scratch_only(&work.path().join("snapshot.db"));
    assert_scratch_only(&work.path().join("restored.db"));
}

#[tokio::test]
async fn sqlite_backup_roundtrip_preserves_snapshot_and_integrity() {
    // 1. Scratch DB in an isolated temp dir (fictitious probe data only).
    let work = tempfile::tempdir().unwrap();
    let db_path = work.path().join("scratch.db");
    assert_scratch_only(&db_path);
    std::fs::write(&db_path, b"").unwrap();
    let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&db_url)
        .await
        .expect("Failed to create scratch database");

    sqlx::query(
        "CREATE TABLE backup_probe (
            id    INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL
        )",
    )
    .execute(&pool)
    .await
    .unwrap();
    for title in [
        "fictitious probe alpha",
        "fictitious probe beta",
        "fictitious probe gamma",
    ] {
        sqlx::query("INSERT INTO backup_probe (title) VALUES (?)")
            .bind(title)
            .execute(&pool)
            .await
            .unwrap();
    }
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_probe")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, 3);

    // 2. Snapshot via SQL only (`VACUUM INTO`) — no sqlite3 CLI.
    let backup_path = work.path().join("snapshot.db");
    assert_scratch_only(&backup_path);
    let escaped = backup_path.display().to_string().replace('\'', "''");
    sqlx::query(&format!("VACUUM INTO '{escaped}'"))
        .execute(&pool)
        .await
        .expect("VACUUM INTO must succeed");
    assert!(
        backup_path.exists(),
        "snapshot file must exist after VACUUM INTO"
    );

    // 3. Mutate the original AFTER the snapshot: the backup must not see this.
    for title in ["fictitious probe delta", "fictitious probe epsilon"] {
        sqlx::query("INSERT INTO backup_probe (title) VALUES (?)")
            .bind(title)
            .execute(&pool)
            .await
            .unwrap();
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_probe")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        after, 5,
        "original must have 5 rows after post-backup writes"
    );
    pool.close().await;

    // 4. Restore: copy the snapshot aside (never overwrite the live file in
    //    this test) and open the copy as the restored DB.
    let restore_path = work.path().join("restored.db");
    assert_scratch_only(&restore_path);
    std::fs::copy(&backup_path, &restore_path).unwrap();
    let restore_url = format!("sqlite://{}?mode=rw", restore_path.display());
    let restored = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&restore_url)
        .await
        .expect("Failed to open restored database");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_probe")
        .fetch_one(&restored)
        .await
        .unwrap();
    assert_eq!(
        count, 3,
        "restored snapshot must contain the 3 pre-backup rows only"
    );
    let titles: Vec<String> = sqlx::query_scalar("SELECT title FROM backup_probe ORDER BY id")
        .fetch_all(&restored)
        .await
        .unwrap();
    assert_eq!(
        titles,
        vec![
            "fictitious probe alpha".to_string(),
            "fictitious probe beta".to_string(),
            "fictitious probe gamma".to_string(),
        ]
    );

    // 5. integrity_check must be clean on the restored copy.
    let rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&restored)
        .await
        .unwrap();
    assert!(!rows.is_empty(), "integrity_check must return rows");
    for row in &rows {
        assert_eq!(
            row.to_lowercase(),
            "ok",
            "integrity_check must pass: {rows:?}"
        );
    }
    restored.close().await;
}
