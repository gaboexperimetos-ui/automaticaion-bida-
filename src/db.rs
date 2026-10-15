//! Database initialization, migrations, and admin management.
//! Uses sqlx with SQLite (lib-sqlite3-sys compiles from source — no system dependency).
//! Documentation: ADR-002 (SQLite for simplicity), ADR-010 (musl target for portability).

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use tracing::{error, info};

use crate::config::Config;

pub type DbPool = SqlitePool;

/// Initialize the database: create parent directories, open pool, run migrations.
pub async fn init(config: &Config) -> Result<DbPool, Box<dyn std::error::Error>> {
    let db_path = &config.db_path;

    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        if !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                error!("Failed to create DB directory {}: {}", parent.display(), e);
                Box::new(e) as Box<dyn std::error::Error>
            })?;
            info!("Created DB directory: {}", parent.display());
        }
    }

    // Create empty DB file if it doesn't exist (sqlx defaults create_if_missing=false
    // when parsing plain file paths — must pre-create the file)
    if !db_path.exists() {
        std::fs::File::create(db_path)?;
    }

    // Use SqliteConnectOptions directly to avoid URL authority parsing issues
    // (sqlite://./path interprets "." as host, not path)
    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .connect(db_path.to_str().unwrap_or("./data/app.db"))
        .await?;

    info!("Database pool initialized: {}", db_path.display());
    Ok(pool)
}

/// Run embedded migrations (from migrations/ directory, checked into source).
pub async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    info!("Running database migrations...");
    sqlx::query("PRAGMA journal_mode=WAL;")
        .execute(pool)
        .await?;
    sqlx::query("PRAGMA foreign_keys=ON;").execute(pool).await?;

    // Embedded migrations (compiled from migrations/*.sql at build time)
    sqlx::migrate!("./migrations").run(pool).await?;

    info!("All migrations applied successfully");
    Ok(())
}

/// Create an admin user (for first-run setup).
/// Returns the user ID. Password must be set separately before use.
pub async fn create_admin(
    pool: &DbPool,
    username: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    use password_hash::PasswordHasher;
    use rand::distributions::Alphanumeric;
    use rand::Rng;

    // Generate a random temporary password
    let temp_pass: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(32)
        .map(char::from)
        .collect();

    let salt = argon2::password_hash::SaltString::generate(rand::thread_rng());
    let argon2 = argon2::Argon2::default();
    let password_hash = argon2
        .hash_password(temp_pass.as_bytes(), &salt)
        .map_err(|e| {
            error!("Failed to hash password: {}", e);
            std::io::Error::other(e.to_string())
        })?;

    let password_hash_str = password_hash.to_string();

    let result = sqlx::query(
        r#"
        INSERT INTO users (username, password_hash)
        VALUES (?, ?)
        ON CONFLICT(username) DO UPDATE SET username = EXCLUDED.username
        RETURNING id
        "#,
    )
    .bind(username)
    .bind(&password_hash_str)
    .execute(pool)
    .await?;

    let id = result.last_insert_rowid();

    info!(
        "Admin user '{}' created (ID: {}). Set password via CLI.",
        username, id
    );

    eprintln!("\n=== Admin user created ===");
    eprintln!("Username: {}", username);
    eprintln!("Set password via: study-platform set-password {}", username);
    eprintln!("===========================\n");

    Ok(id)
}

/// Verify database schema integrity.
pub async fn verify_schema(pool: &DbPool) -> Result<(), String> {
    let tables: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| format!("Database error: {}", e))?;

    let expected = ["users", "sessions", "notes", "app_config"];
    for table in &expected {
        if !tables.iter().any(|t| t == table) {
            error!("Missing table: {}", table);
            return Err(format!("Missing table: {}", table));
        }
    }

    info!("Database schema verified: {} tables present", tables.len());
    Ok(())
}

/// Get the count of notes (for health checks).
pub async fn get_note_count(pool: &DbPool) -> Result<i64, sqlx::Error> {
    let count: i64 = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notes")
        .fetch_one(pool)
        .await?;
    Ok(count)
}
