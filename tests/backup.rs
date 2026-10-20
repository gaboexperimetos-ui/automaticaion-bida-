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

// ---------------------------------------------------------------------------
// Integration tests for the `backup` module itself: restore roundtrip,
// retention and the ordering of `list_backups`.
// ---------------------------------------------------------------------------

/// Writes a file with an explicit modification time, so the ordering used by
/// `list_backups` (mtime, descending) is deterministic and does not depend on
/// the order in which the files happen to be created.
fn write_with_mtime(path: &std::path::Path, contents: &[u8], mtime: std::time::SystemTime) {
    std::fs::write(path, contents).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(mtime))
        .unwrap();
}

#[tokio::test]
async fn restore_reinstates_the_latest_backup_over_the_same_pool() {
    use std::time::Duration;

    use study_platform::backup;

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

    // 2. First backup (3 rows).
    let backups = work.path().join("backups");
    let first = backup::backup_now(&pool, &backups).await.unwrap();
    assert!(first.bytes > 0, "the backup copy must not be empty");
    assert!(backups.join(&first.name).exists());

    // The backup name has second resolution: wait so the second backup gets a
    // different name and the directory really holds two backups.
    tokio::time::sleep(Duration::from_millis(1100)).await;

    // 3. Two more rows (5) and a second, newer backup.
    for title in ["fictitious probe delta", "fictitious probe epsilon"] {
        sqlx::query("INSERT INTO backup_probe (title) VALUES (?)")
            .bind(title)
            .execute(&pool)
            .await
            .unwrap();
    }
    let latest = backup::backup_now(&pool, &backups).await.unwrap();
    assert_ne!(
        first.name, latest.name,
        "the two backups must have different names"
    );

    let listed = backup::list_backups(&backups).await;
    assert_eq!(
        listed.len(),
        2,
        "the directory must hold exactly two backups"
    );
    assert_eq!(listed[0].name, latest.name, "newest backup first");
    assert_eq!(listed[1].name, first.name);

    // 4. The live database drifts away from the backups (7 rows).
    for title in ["fictitious probe zeta", "fictitious probe eta"] {
        sqlx::query("INSERT INTO backup_probe (title) VALUES (?)")
            .bind(title)
            .execute(&pool)
            .await
            .unwrap();
    }
    let live: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_probe")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(live, 7, "the live database must differ from the backup");

    // 5. Restore the newest backup over the database of the SAME pool.
    let restored = backup::restore(&pool, &backups, &listed[0].name)
        .await
        .unwrap();
    assert_eq!(restored.name, latest.name);
    assert!(restored.bytes > 0);

    pool.close().await;

    // 6. Verify the file on disk with a fresh pool: the old connection may
    //    still hold pages cached from before the restore.
    let check = SqlitePoolOptions::new()
        .max_connections(1)
        .connect(&format!("sqlite://{}?mode=rwc", db_path.display()))
        .await
        .expect("Failed to reopen the restored database");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM backup_probe")
        .fetch_one(&check)
        .await
        .unwrap();
    assert_eq!(
        count, 5,
        "after the restore the database must match the backup (5 rows)"
    );
    let titles: Vec<String> = sqlx::query_scalar("SELECT title FROM backup_probe ORDER BY rowid")
        .fetch_all(&check)
        .await
        .unwrap();
    assert_eq!(
        titles,
        vec![
            "fictitious probe alpha".to_string(),
            "fictitious probe beta".to_string(),
            "fictitious probe gamma".to_string(),
            "fictitious probe delta".to_string(),
            "fictitious probe epsilon".to_string(),
        ]
    );
    let max_rowid: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(rowid), 0) FROM backup_probe")
        .fetch_one(&check)
        .await
        .unwrap();
    assert_eq!(max_rowid, 5, "the rowid must survive the restore");
    let rows: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_all(&check)
        .await
        .unwrap();
    assert!(!rows.is_empty(), "integrity_check must return rows");
    for row in &rows {
        assert_eq!(row.to_lowercase(), "ok", "integrity_check: {rows:?}");
    }
    check.close().await;
}

#[tokio::test]
async fn retention_keeps_only_the_newest_of_three_backups() {
    use std::time::{Duration, SystemTime};

    use study_platform::backup;

    let work = tempfile::tempdir().unwrap();
    let backups = work.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();

    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let oldest = "app-20260101-000000.db";
    let middle = "app-20260101-000100.db";
    let newest = "app-20260101-000200.db";

    // Dummy (non-SQLite) payloads: retention only manipulates files.
    write_with_mtime(&backups.join(oldest), b"oldest", base);
    write_with_mtime(
        &backups.join(middle),
        b"middle",
        base + Duration::from_secs(3600),
    );
    write_with_mtime(
        &backups.join(newest),
        b"newest",
        base + Duration::from_secs(7200),
    );
    // The sidecar must be dragged along by the retention.
    write_with_mtime(&backups.join(format!("{oldest}.sha256")), b"ok\n", base);

    let listed = backup::list_backups(&backups).await;
    assert_eq!(listed.len(), 3, "the sidecar is not a backup");
    assert_eq!(listed[0].name, newest);
    assert_eq!(listed[1].name, middle);
    assert_eq!(listed[2].name, oldest);

    let deleted = backup::enforce_retention(&backups, 1).await.unwrap();
    assert_eq!(
        deleted,
        vec![middle.to_string(), oldest.to_string()],
        "the oldest backups must be the deleted ones"
    );
    assert!(!backups.join(oldest).exists());
    assert!(!backups.join(middle).exists());
    assert!(!backups.join(format!("{oldest}.sha256")).exists());
    assert!(backups.join(newest).exists());

    let remaining = backup::list_backups(&backups).await;
    assert_eq!(remaining.len(), 1, "only one backup must survive");
    assert_eq!(remaining[0].name, newest);
}

#[tokio::test]
async fn list_backups_orders_by_mtime_descending_and_skips_sidecars() {
    use std::time::{Duration, SystemTime};

    use study_platform::backup;

    let work = tempfile::tempdir().unwrap();
    let backups = work.path().join("backups");
    std::fs::create_dir_all(&backups).unwrap();

    let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // Names and mtimes disagree on purpose: the order must follow the mtime.
    let oldest = "app-20260103-000000.db";
    let middle = "app-20260101-000000.db";
    let newest = "app-20260102-000000.db";

    write_with_mtime(&backups.join(oldest), b"a", base);
    write_with_mtime(
        &backups.join(middle),
        b"bb",
        base + Duration::from_secs(3600),
    );
    write_with_mtime(
        &backups.join(newest),
        b"ccc",
        base + Duration::from_secs(7200),
    );
    // Neither the sidecar nor an unrelated file is a backup.
    write_with_mtime(
        &backups.join(format!("{newest}.sha256")),
        b"ok\n",
        base + Duration::from_secs(7200),
    );
    write_with_mtime(
        &backups.join("notas.txt"),
        b"x",
        base + Duration::from_secs(7200),
    );

    let listed = backup::list_backups(&backups).await;
    let names: Vec<&str> = listed.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(
        names,
        vec![newest, middle, oldest],
        "backups must be listed from newest to oldest by mtime"
    );
    assert_eq!(listed[0].bytes, 3, "the size of the copy must be reported");
    assert!(
        !listed[0].created.is_empty(),
        "the creation date must be set"
    );
}
