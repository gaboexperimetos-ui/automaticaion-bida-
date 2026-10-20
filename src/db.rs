//! Database initialization, migrations, and admin management.
//! Uses sqlx with SQLite (lib-sqlite3-sys compiles from source — no system dependency).
//! Documentation: ADR-002 (SQLite for simplicity), ADR-010 (musl target for portability).

use std::path::PathBuf;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tracing::{error, info, warn};

use crate::config::Config;

pub type DbPool = SqlitePool;

/// Cache de paginas por conexion, en KiB (negativo = KiB en vez de paginas).
///
/// -2000 = 2 MB. Es el default de SQLite desde la 3.12, pero se fija de
/// forma explicita: con 4 conexiones el tope de cache es 8 MB y queda
/// documentado (no heredado) en vez de depender del build de SQLite.
const PAGE_CACHE_KIB: i64 = -2000;

/// Espera maxima por un lock de SQLite, en ms.
///
/// Sin `busy_timeout` sqlx falla al instante con "database is locked" en
/// cuanto otra conexion tiene el lock de escritura; 5 s es el valor que
/// usa SQLite en la practica.
const BUSY_TIMEOUT_MS: u64 = 5_000;

/// Umbral de auto-checkpoint del WAL, en paginas (4 KiB cada una) => ~4 MB.
/// Mantiene acotado el tamano del fichero `-wal` (y su consumo de disco).
const WAL_AUTOCHECKPOINT_PAGES: u32 = 1_000;

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

    // Pool size: hard cap on RAM (one page cache per connection) and on
    // write contention in the WAL. Configurable via DB_MAX_CONNECTIONS.
    let mut max_connections = config.db_max_connections.max(1);
    if db_path.as_os_str() == ":memory:" {
        // An in-memory DB lives inside its connection: a second connection
        // would be a second, different database.
        max_connections = 1;
    }

    // Use SqliteConnectOptions directly to avoid URL authority parsing issues
    // (sqlite://./path interprets "." as host, not path).
    // Every pragma here is applied to EACH new connection of the pool
    // (sqlx runs them at establish time), which is the only correct way:
    // cache_size/temp_store/busy_timeout are per-connection settings.
    let connect_options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        // Wait up to 5 s for a lock instead of failing right away.
        .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
        // Enforce FKs on every connection (the pragma is per-connection).
        .foreign_keys(true)
        // RAM: cap the page cache at 2 MB per connection.
        .pragma("cache_size", PAGE_CACHE_KIB.to_string())
        // RAM: no mmap (mapping the file inflates RSS).
        .pragma("mmap_size", "0")
        // RAM: temp tables/sorters on disk instead of the heap.
        .pragma("temp_store", "FILE")
        // WAL: checkpoint every ~4 MB so the -wal file does not grow.
        .pragma("wal_autocheckpoint", WAL_AUTOCHECKPOINT_PAGES.to_string());

    let pool = SqlitePoolOptions::new()
        .max_connections(max_connections)
        .min_connections(1)
        // Do not fail instantly when the pool is saturated.
        .acquire_timeout(Duration::from_secs(10))
        // Release idle connections (and their page cache).
        .idle_timeout(Duration::from_secs(300))
        // Recycle connections from time to time.
        .max_lifetime(Some(Duration::from_secs(1_800)))
        .connect_with(connect_options)
        .await?;

    info!(
        "Database pool initialized: {} (max_connections={}, busy_timeout={}ms)",
        db_path.display(),
        max_connections,
        BUSY_TIMEOUT_MS
    );
    Ok(pool)
}

/// Run embedded migrations (from migrations/ directory, checked into source).
pub async fn run_migrations(pool: &DbPool) -> Result<(), sqlx::Error> {
    info!("Running database migrations...");
    // WAL is a persistent property of the DB file. Switching it needs an
    // exclusive lock that `busy_timeout` cannot wait for, so only ask for it
    // when the DB is not already in WAL (the normal case on restarts): that
    // avoids "database is locked" when several connections are already up.
    let modo: Option<String> = sqlx::query("PRAGMA journal_mode;")
        .fetch_one(pool)
        .await?
        .try_get("journal_mode")
        .ok();
    if !modo
        .as_deref()
        .map(|m| m.eq_ignore_ascii_case("wal"))
        .unwrap_or(false)
    {
        sqlx::query("PRAGMA journal_mode=WAL;")
            .execute(pool)
            .await?;
    }
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

/// Crea un backup de la base de datos.
///
/// Ya no hay dos sistemas de backup: esta función **delega** en el sistema
/// canónico (`crate::backup`), el mismo que usan el handler de administración
/// y la tarea periódica. Eso significa:
///
/// - volcado del WAL (`PRAGMA wal_checkpoint(TRUNCATE)`) antes de copiar,
/// - copia del fichero de la base a `data/backups/app-YYYYmmdd-HHMMSS.db`
///   (no un `.gz` en `./backups/`),
/// - sidecar `….db.sha256` con el `PRAGMA integrity_check` del instante.
///
/// Se conserva la firma legacy `(ruta, bytes)`: los bytes son los de la copia.
/// Los errores se propagan tal cual (nunca un backup simulado).
pub async fn backup(pool: &DbPool) -> Result<(PathBuf, u64), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(crate::backup::DEFAULT_DIR);
    let info = crate::backup::backup_now(pool, &dir)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    Ok((PathBuf::from(info.path), info.bytes))
}

/// Resumen del estado de la base de datos (panel de administración / handoff).
///
/// - `bytes`: tamaño del fichero de la base en disco (sin contar el WAL).
/// - `tables`: número de tablas reales (excluye las internas de SQLite,
///   `sqlite_%`; las tablas sombra de FTS5 sí se cuentan porque existen).
/// - `notes`: filas de la tabla `notes`.
/// - `conversations`: hilos distintos de `chat_history` (`COUNT(DISTINCT
///   subject)`) — la misma definición que usa la página de handoff.
///
/// Nunca falla: cada campo degrada a 0 y deja un aviso en el log si su
/// fuente no está disponible (tabla ausente, base en memoria, etc.).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DbInfo {
    /// Tamaño del fichero de la base en bytes.
    pub bytes: u64,
    /// Número de tablas de la base.
    pub tables: i64,
    /// Número de notas.
    pub notes: i64,
    /// Número de conversaciones (hilos distintos de `chat_history`).
    pub conversations: i64,
}

/// Ruta real del fichero de la base según SQLite.
///
/// `PRAGMA database_list` devuelve una fila por base adjunta; la columna
/// `file` es la ruta en disco de la principal. `None` si la base está en
/// memoria (`:memory:`) o si la ruta está vacía.
async fn db_file_path(pool: &DbPool) -> Option<PathBuf> {
    let fila = sqlx::query("PRAGMA database_list")
        .fetch_one(pool)
        .await
        .ok()?;
    let fichero: Option<String> = fila.try_get("file").ok().flatten();
    let fichero = fichero?;
    if fichero.is_empty() {
        return None;
    }
    Some(PathBuf::from(fichero))
}

/// Recoge el estado de la base de datos para el panel / handoff.
///
/// Es la única fuente de estos números: el panel no debe consultar la BD por
/// su cuenta (evita duplicar consultas y divergir en los conteos).
pub async fn db_info(pool: &DbPool) -> DbInfo {
    // Tamaño del fichero de la base (no incluye el WAL, que es volátil).
    let bytes = match db_file_path(pool).await {
        Some(ruta) => match tokio::fs::metadata(&ruta).await {
            Ok(meta) => meta.len(),
            Err(e) => {
                warn!(
                    "db_info: no se pudo leer el tamaño de {}: {}",
                    ruta.display(),
                    e
                );
                0
            }
        },
        None => {
            warn!("db_info: la base no está en disco (¿:memory:?): tamaño desconocido");
            0
        }
    };

    // Tablas reales: sin las internas de SQLite (`sqlite_%`).
    let tables: i64 = match sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_one(pool)
    .await
    {
        Ok(n) => n,
        Err(e) => {
            warn!("db_info: no se pudo contar las tablas: {}", e);
            0
        }
    };

    // Notas.
    let notes: i64 = match sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM notes")
        .fetch_one(pool)
        .await
    {
        Ok(n) => n,
        Err(e) => {
            warn!("db_info: no se pudieron contar las notas: {}", e);
            0
        }
    };

    // Conversaciones: hilos distintos (por `subject`) de `chat_history`.
    let conversations: i64 =
        match sqlx::query_scalar::<_, i64>("SELECT COUNT(DISTINCT subject) FROM chat_history")
            .fetch_one(pool)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                warn!("db_info: no se pudieron contar las conversaciones: {}", e);
                0
            }
        };

    DbInfo {
        bytes,
        tables,
        notes,
        conversations,
    }
}
