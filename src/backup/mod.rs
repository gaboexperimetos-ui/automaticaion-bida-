//! F9 — Backups y restauración de la base de datos SQLite.
//!
//! Un backup es una copia del fichero de la base (`app-YYYYmmdd-HHMMSS.db`)
//! acompañada de un sidecar `app-YYYYmmdd-HHMMSS.db.sha256`.
//!
//! - El sidecar **no** es un hash criptográfico: guarda el resultado del
//!   `PRAGMA integrity_check` de la base en el instante de la copia. Es un
//!   check de integridad (detecta bases corruptas), no una firma.
//! - Antes de copiar se ejecuta `PRAGMA wal_checkpoint(TRUNCATE)`: el WAL se
//!   vuelca al fichero principal y se trunca, de modo que el fichero copiado
//!   es un snapshot completo y consistente.
//!
//! RESTAURACIÓN
//! -------------
//! La restauración **no** sustituye el fichero de la base en vivo ni cambia
//! el modo de journal. Hacerlo exigía `PRAGMA journal_mode=DELETE`, que
//! necesita un lock EXCLUSIVO que `busy_timeout` no puede esperar: con la app
//! en marcha el endpoint devolvía `database is locked (code 5)` en
//! producción, sin llegar a copiar nada.
//!
//! Ahora el restore:
//!  1. Valida el backup (`PRAGMA integrity_check`) con una conexión propia y
//!     `busy_timeout=5000`.
//!  2. Vuelca el WAL de la base viva (`PRAGMA wal_checkpoint(TRUNCATE)`) a
//!     través del pool, sin abrir conexiones nuevas.
//!  3. Copia el backup a una copia de trabajo `{base}.restore-{ts}`.
//!  4. Abre una conexión propia a la base viva (también con
//!     `busy_timeout=5000`), adjunta la copia de trabajo
//!     (`ATTACH ... AS restauracion`) y, dentro de una transacción, copia el
//!     CONTENIDO (datos) del backup sobre la base viva.
//!  5. Mantiene el esquema de la app en marcha: los objetos que sólo existen
//!     en el backup se crean y el libro de migraciones
//!     (`_sqlx_migrations`) nunca se restaura.
//!
//! Si la base está ocupada (SQLITE_BUSY) el error es honesto: "server en
//! uso: reintenta tras detener la app". Nunca se simula un restore que no ha
//! ocurrido.
//!
//! El pool NUNCA se cierra en este módulo: cerrar el pool mataría la app.

/// Directorio por defecto de los backups, relativo a la raíz de la app.
///
/// El orquestador (`main.rs`) usará esta constante como directorio de la
/// tarea periódica y de las operaciones manuales (listar/restaurar).
pub const DEFAULT_DIR: &str = "data/backups";
/// Número de backups que se conservan por defecto.
pub const DEFAULT_KEEP: usize = 5;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Local};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Row, Sqlite, SqlitePool};
use thiserror::Error;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Prefijo de los ficheros de backup generados por este módulo.
const BACKUP_PREFIX: &str = "app-";
/// Sufijo de los ficheros de backup.
const BACKUP_SUFFIX: &str = ".db";
/// Sufijo del sidecar que acompaña a cada backup.
const SIDECAR_SUFFIX: &str = ".sha256";
/// Backups conservados por la tarea periódica (mismo valor que
/// [`DEFAULT_KEEP`], que es la constante pública).
const DEFAULT_RETENTION: usize = DEFAULT_KEEP;
/// Marca temporal dentro del nombre del backup.
const NAME_TIME_FORMAT: &str = "%Y%m%d-%H%M%S";
/// Formato de `BackupInfo::created` (hora local).
const CREATED_FORMAT: &str = "%Y-%m-%d %H:%M:%S";
/// Espera máxima ante bloqueos (SQLITE_BUSY) de las conexiones que abre este
/// módulo: la de validación y la de operación.
const BUSY_TIMEOUT_MS: u64 = 5_000;
/// Sufijo de la copia de trabajo que se adjunta (ATTACH) al restaurar.
const RESTORE_SUFFIX: &str = ".restore-";
/// Tablas que nunca se copian del backup.
///
/// `_sqlx_migrations` es el libro de migraciones: describe el ESQUEMA VIVO,
/// así que restaurarlo haría que sqlx volviera a aplicar migraciones ya
/// aplicadas sobre una base cuyo contenido es anterior.
const TABLAS_NO_RESTAURABLES: [&str; 1] = ["_sqlx_migrations"];
/// Sufijos de las tablas sombra de una tabla virtual FTS5: no se copian (las
/// gestiona la propia tabla virtual, que mantiene su índice).
const SOMBRAS_FTS: [&str; 5] = ["data", "idx", "content", "docsize", "config"];

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("sqlite: {0}")]
    Sqlite(String),
    #[error("io: {0}")]
    Io(String),
    #[error("path inseguro")]
    Unsafe,
    #[error("integrity")]
    Integrity,
}

/// Metadatos de un backup.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupInfo {
    /// Nombre del fichero (p. ej. `app-20260131-153011.db`).
    pub name: String,
    /// Ruta completa del fichero de la base respaldada.
    pub path: String,
    /// Tamaño en bytes de la copia.
    pub bytes: u64,
    /// Momento de la copia, hora local (`%Y-%m-%d %H:%M:%S`).
    pub created: String,
}

/// Envuelve un error de SQLite.
fn sqlite_err(e: sqlx::Error) -> BackupError {
    BackupError::Sqlite(e.to_string())
}

/// Envuelve un error de E/S.
fn io_err(e: std::io::Error) -> BackupError {
    BackupError::Io(e.to_string())
}

/// Error honesto de "la base está en uso" (SQLITE_BUSY).
///
/// Se devuelve tal cual al llamante: preferimos un error claro a un restore
/// a medias que parezca haber funcionado.
fn error_en_uso() -> BackupError {
    BackupError::Sqlite(
        "server en uso: reintenta tras detener la app (la base está bloqueada por otra conexión)"
            .to_string(),
    )
}

/// ¿El error es un bloqueo de SQLite (SQLITE_BUSY, código 5)?
fn es_bloqueo(e: &sqlx::Error) -> bool {
    match e {
        sqlx::Error::Database(db) => db
            .code()
            .and_then(|c| c.parse::<i32>().ok())
            .map(|codigo| codigo & 0xff == 5)
            .unwrap_or(false),
        _ => false,
    }
}

/// Envuelve un error de SQLite del restore: los bloqueos se reportan con el
/// mensaje honesto de "server en uso"; el resto se propaga tal cual.
fn sqlite_err_restore(e: sqlx::Error) -> BackupError {
    if es_bloqueo(&e) {
        warn!("restore: la base está en uso ({e})");
        return error_en_uso();
    }
    BackupError::Sqlite(e.to_string())
}

/// Nombre de backup admitido: `app-<algo>.db`, sin separadores ni `..`.
fn is_safe_backup_name(name: &str) -> bool {
    name.len() > BACKUP_PREFIX.len() + BACKUP_SUFFIX.len()
        && name.starts_with(BACKUP_PREFIX)
        && name.ends_with(BACKUP_SUFFIX)
        && !name.contains("..")
        && !name.contains('/')
        && !name.contains('\\')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Ruta real del fichero de la base, según SQLite.
///
/// `PRAGMA database_list` devuelve una fila por base adjunta; la columna
/// `file` es la ruta en disco de la principal. Si la base está en memoria
/// (`:memory:`) no hay fichero: es un error.
async fn db_file_path(pool: &SqlitePool) -> Result<PathBuf, BackupError> {
    let fila = sqlx::query("PRAGMA database_list")
        .fetch_one(pool)
        .await
        .map_err(sqlite_err)?;

    let fichero = match fila.try_get::<Option<String>, _>("file") {
        Ok(Some(f)) => f,
        Ok(None) => {
            return Err(BackupError::Sqlite(
                "PRAGMA database_list: la base no está en disco (¿:memory:?)".to_string(),
            ))
        }
        Err(e) => {
            return Err(BackupError::Sqlite(format!(
                "PRAGMA database_list: no se pudo leer la columna 'file': {e}"
            )))
        }
    };

    if fichero.is_empty() {
        return Err(BackupError::Sqlite(
            "PRAGMA database_list: ruta de la base vacía".to_string(),
        ));
    }

    Ok(PathBuf::from(fichero))
}

/// Conexión de solo lectura para validar el backup.
///
/// Se abre aparte (no toca la base en vivo) y espera hasta `BUSY_TIMEOUT_MS`
/// ante un bloqueo en lugar de fallar al instante.
async fn conexion_validacion(path: &Path) -> Result<SqliteConnection, BackupError> {
    let opciones = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .read_only(true)
        .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS));
    let mut conexion = SqliteConnection::connect_with(&opciones)
        .await
        .map_err(sqlite_err)?;
    // `busy_timeout` ya lo aplica sqlx al conectar; se deja explícito porque
    // el restore depende de él.
    sqlx::query(&format!("PRAGMA busy_timeout={BUSY_TIMEOUT_MS}"))
        .execute(&mut conexion)
        .await
        .map_err(sqlite_err)?;
    Ok(conexion)
}

/// Ejecuta `PRAGMA integrity_check` sobre un fichero de base de datos.
///
/// Abre el fichero aparte (no toca la base en vivo) y devuelve el resultado
/// del check: `"ok"` si el fichero está sano.
async fn integrity_check_file(path: &Path) -> Result<String, BackupError> {
    let mut conexion = conexion_validacion(path).await?;
    let estado: String = sqlx::query_scalar::<Sqlite, String>("PRAGMA integrity_check")
        .fetch_one(&mut conexion)
        .await
        .map_err(sqlite_err)?;
    Ok(estado)
}

/// Vuelca el WAL de la base viva (`PRAGMA wal_checkpoint(TRUNCATE)`) usando el
/// pool recibido: no se abre ninguna conexión nueva.
///
/// Un `busy` aquí sólo significa que hay lectores activos: la restauración
/// escribe en el WAL y no depende del checkpoint, así que no es fatal.
async fn checkpoint_via_pool(pool: &SqlitePool) {
    match sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(pool)
        .await
    {
        Ok(fila) => {
            let busy: i64 = fila.try_get("busy").unwrap_or(0);
            if busy != 0 {
                warn!("restore: el checkpoint del WAL quedó ocupado (hay lectores activos)");
            }
        }
        Err(e) => warn!("restore: no se pudo volcar el WAL: {e}"),
    }
}

/// Crea un backup de la base en `dir`.
///
/// Hace el volcado del WAL (`PRAGMA wal_checkpoint(TRUNCATE)`), copia el
/// fichero de la base a `dir/app-YYYYmmdd-HHMMSS.db` y escribe el sidecar
/// `….db.sha256` con el resultado del `PRAGMA integrity_check`.
///
/// Devuelve los metadatos de la copia (los bytes son los de la copia, no los
/// del original).
pub async fn backup_now(pool: &SqlitePool, dir: &Path) -> Result<BackupInfo, BackupError> {
    tokio::fs::create_dir_all(dir).await.map_err(io_err)?;

    // Volcar el WAL al fichero principal: sin este paso la copia del fichero
    // podría no contener las últimas transacciones confirmadas.
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(pool)
        .await
        .map_err(sqlite_err)?;

    // El fichero real de la base (no una ruta supuesta).
    let origen = db_file_path(pool).await?;

    let ahora = Local::now();
    let name = format!("{BACKUP_PREFIX}{}.db", ahora.format(NAME_TIME_FORMAT));
    let destino = dir.join(&name);

    if tokio::fs::try_exists(&destino).await.unwrap_or(false) {
        warn!("el backup {name} ya existía: se sobrescribe");
    }

    // Copia a fichero; los bytes devueltos son los de la copia.
    let bytes = tokio::fs::copy(&origen, &destino).await.map_err(io_err)?;
    if bytes == 0 {
        let _ = tokio::fs::remove_file(&destino).await;
        return Err(BackupError::Io(
            "la copia del backup quedó vacía".to_string(),
        ));
    }

    // Sidecar: el resultado del check de integridad de la base en este
    // instante (no es un hash criptográfico).
    let check: String = sqlx::query_scalar::<Sqlite, String>("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
        .map_err(sqlite_err)?;
    tokio::fs::write(
        dir.join(format!("{name}{SIDECAR_SUFFIX}")),
        format!("{check}\n"),
    )
    .await
    .map_err(io_err)?;

    Ok(BackupInfo {
        name,
        path: destino.to_string_lossy().to_string(),
        bytes,
        created: ahora.format(CREATED_FORMAT).to_string(),
    })
}

/// Columna de una tabla: lo mínimo necesario para copiar su contenido.
struct Columna {
    nombre: String,
    tipo: String,
    pk: i64,
}

/// ¿El DDL crea una tabla virtual (FTS5)?
fn es_virtual_fts(ddl: &str) -> bool {
    ddl.trim_start()
        .to_ascii_lowercase()
        .starts_with("create virtual table")
}

/// ¿Alguna columna es el alias del rowid (`INTEGER PRIMARY KEY`)?
fn alias_de_rowid(columnas: &[Columna]) -> bool {
    columnas
        .iter()
        .any(|c| c.pk == 1 && c.tipo.eq_ignore_ascii_case("integer"))
}

/// Columnas de una tabla (nombre, tipo y posición en la clave primaria).
async fn columnas(
    conexion: &mut SqliteConnection,
    esquema: &str,
    tabla: &str,
) -> Result<Vec<Columna>, BackupError> {
    let sql = format!(
        "PRAGMA {esquema}.table_info('{}')",
        tabla.replace('\'', "''")
    );
    let filas = sqlx::query(&sql)
        .fetch_all(&mut *conexion)
        .await
        .map_err(sqlite_err)?;
    let mut salida = Vec::with_capacity(filas.len());
    for fila in filas {
        let nombre: String = fila.try_get("name").map_err(sqlite_err)?;
        let tipo: Option<String> = fila.try_get("type").map_err(sqlite_err)?;
        let pk: i64 = fila.try_get("pk").unwrap_or(0);
        salida.push(Columna {
            nombre,
            tipo: tipo.unwrap_or_default(),
            pk,
        });
    }
    Ok(salida)
}

/// Tablas WITHOUT ROWID de un esquema (no tienen rowid copiable).
async fn tablas_sin_rowid(
    conexion: &mut SqliteConnection,
    esquema: &str,
) -> Result<HashSet<String>, BackupError> {
    let sql = format!(
        "SELECT name FROM pragma_table_list WHERE schema = '{}' AND wr = 1",
        esquema.replace('\'', "''")
    );
    match sqlx::query(&sql).fetch_all(&mut *conexion).await {
        Ok(filas) => Ok(filas
            .into_iter()
            .filter_map(|f| f.try_get::<String, _>("name").ok())
            .collect()),
        Err(e) => {
            warn!("restore: no se pudo listar las tablas sin rowid ({e})");
            Ok(HashSet::new())
        }
    }
}

/// Ruta de la copia de trabajo: `{base}.restore-{ts}`, en el mismo directorio
/// que la base (así, mismo sistema de ficheros).
fn copia_de_trabajo(destino: &Path) -> PathBuf {
    let marca = Local::now().format(NAME_TIME_FORMAT);
    let nombre = format!(
        "{}{RESTORE_SUFFIX}{marca}",
        destino
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "app.db".to_string())
    );
    let padre = destino.parent().unwrap_or_else(|| Path::new("."));
    padre.join(nombre)
}

/// Pasa la copia de trabajo a modo DELETE (journal de rollback): al adjuntarla
/// no deja ficheros `-wal`/`-shm` junto a la base viva.
async fn preparar_copia(copia: &Path) -> Result<(), BackupError> {
    let opciones = SqliteConnectOptions::new()
        .filename(copia)
        .create_if_missing(false)
        .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS));
    let mut conexion = SqliteConnection::connect_with(&opciones)
        .await
        .map_err(sqlite_err)?;
    let modo: String = sqlx::query_scalar("PRAGMA journal_mode=DELETE")
        .fetch_one(&mut conexion)
        .await
        .map_err(sqlite_err)?;
    if !modo.eq_ignore_ascii_case("delete") {
        warn!("restore: la copia de trabajo sigue en modo {modo}");
    }
    Ok(())
}

/// Deshace la transacción abierta (best effort).
async fn rollback(conexion: &mut SqliteConnection) {
    if let Err(e) = sqlx::query("ROLLBACK").execute(&mut *conexion).await {
        warn!("restore: no se pudo deshacer la transacción: {e}");
    }
}

/// Restaura el CONTENIDO del backup (adjuntado como `restauracion`) sobre la
/// base viva, dentro de una única transacción.
///
/// La conexión es propia (no el pool) y lleva `busy_timeout`: si la base está
/// ocupada se espera y, si sigue ocupada, se devuelve el error honesto de
/// "server en uso" en lugar de fingir un restore.
async fn restaurar_contenido(destino: &Path, copia: &Path) -> Result<(), BackupError> {
    let opciones = SqliteConnectOptions::new()
        .filename(destino)
        .create_if_missing(false)
        .busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
        .foreign_keys(false);
    let mut conexion = SqliteConnection::connect_with(&opciones)
        .await
        .map_err(sqlite_err)?;
    sqlx::query(&format!("PRAGMA busy_timeout={BUSY_TIMEOUT_MS}"))
        .execute(&mut conexion)
        .await
        .map_err(sqlite_err)?;
    // Las claves foráneas se desactivan durante la copia: el contenido del
    // backup es coherente consigo mismo, no con el orden de borrado.
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut conexion)
        .await
        .map_err(sqlite_err)?;

    // Lock de escritura (espera hasta `busy_timeout`).
    let begun = sqlx::query("BEGIN IMMEDIATE").execute(&mut conexion).await;
    if let Err(e) = begun {
        return Err(sqlite_err_restore(e));
    }

    // La copia de trabajo se adjunta DENTRO de la transacción.
    let ruta = copia.to_string_lossy().replace('\'', "''");
    let adjuntada = sqlx::query(&format!("ATTACH DATABASE '{ruta}' AS restauracion"))
        .execute(&mut conexion)
        .await;
    if let Err(e) = adjuntada {
        rollback(&mut conexion).await;
        return Err(sqlite_err_restore(e));
    }

    let copiado = copiar_contenido(&mut conexion).await;
    match copiado {
        Ok(()) => {
            let confirmado = sqlx::query("COMMIT").execute(&mut conexion).await;
            match confirmado {
                Ok(_) => {
                    let _ = sqlx::query("DETACH DATABASE restauracion")
                        .execute(&mut conexion)
                        .await;
                    Ok(())
                }
                Err(e) => {
                    rollback(&mut conexion).await;
                    Err(sqlite_err_restore(e))
                }
            }
        }
        Err(e) => {
            rollback(&mut conexion).await;
            Err(e)
        }
    }
}

/// Copia el contenido de `restauracion` sobre `main` (transacción abierta).
///
/// Se copian los datos de todas las tablas, los objetos que sólo existen en
/// el backup se crean y el libro de migraciones no se toca. Las tablas
/// virtuales FTS5 se copian por su interfaz propia (filas y rowid) para que su
/// índice y sus tablas sombra queden coherentes.
async fn copiar_contenido(conexion: &mut SqliteConnection) -> Result<(), BackupError> {
    let filas = sqlx::query(
        "SELECT name, COALESCE(sql, '') AS ddl FROM restauracion.sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .fetch_all(&mut *conexion)
    .await
    .map_err(sqlite_err)?;

    let mut objetos: Vec<(String, String)> = Vec::with_capacity(filas.len());
    for fila in filas {
        let nombre: String = fila.try_get("name").map_err(sqlite_err)?;
        let ddl: String = fila.try_get("ddl").map_err(sqlite_err)?;
        objetos.push((nombre, ddl));
    }

    // Tablas virtuales FTS5 y sus tablas sombra.
    let fts: HashSet<String> = objetos
        .iter()
        .filter(|(_, ddl)| es_virtual_fts(ddl))
        .map(|(nombre, _)| nombre.clone())
        .collect();
    let sombras: HashSet<String> = fts
        .iter()
        .flat_map(|f| SOMBRAS_FTS.iter().map(move |s| format!("{f}_{s}")))
        .collect();

    let sin_rowid = tablas_sin_rowid(conexion, "restauracion").await?;

    for (nombre, ddl) in &objetos {
        if TABLAS_NO_RESTAURABLES.contains(&nombre.as_str()) {
            info!("restore: {nombre} no se restaura (libro de migraciones)");
            continue;
        }
        if sombras.contains(nombre) {
            continue;
        }
        copiar_tabla(
            conexion,
            "restauracion",
            "main",
            nombre,
            Some(ddl.as_str()),
            !sin_rowid.contains(nombre),
        )
        .await?;
    }
    Ok(())
}

/// Copia las filas de una tabla de `origen` sobre `destino`.
///
/// - `ddl`: definición de la tabla; si no existe en `destino`, se crea.
/// - `con_rowid`: la tabla tiene rowid propio (no es alias de una columna).
async fn copiar_tabla(
    conexion: &mut SqliteConnection,
    origen: &str,
    destino: &str,
    tabla: &str,
    ddl: Option<&str>,
    con_rowid: bool,
) -> Result<(), BackupError> {
    let columnas_origen = columnas(conexion, origen, tabla).await?;
    if columnas_origen.is_empty() {
        return Ok(());
    }

    let mut columnas_destino = columnas(conexion, destino, tabla).await?;
    if columnas_destino.is_empty() {
        // La tabla no existe en la base viva: se crea con el DDL del backup.
        let Some(ddl) = ddl else { return Ok(()) };
        if let Err(e) = sqlx::query(ddl).execute(&mut *conexion).await {
            warn!("restore: no se pudo crear la tabla {tabla} ({e}); se omite su contenido");
            return Ok(());
        }
        columnas_destino = columnas(conexion, destino, tabla).await?;
        if columnas_destino.is_empty() {
            return Ok(());
        }
    }

    // Columnas que existen en ambos lados: el resto del backup no se puede
    // volcar sobre el esquema vivo.
    let comunes: Vec<String> = columnas_origen
        .iter()
        .filter(|c| columnas_destino.iter().any(|d| d.nombre == c.nombre))
        .map(|c| c.nombre.clone())
        .collect();
    if comunes.is_empty() {
        return Err(BackupError::Sqlite(format!(
            "restore: la tabla {tabla} del backup no tiene columnas en común con el esquema actual"
        )));
    }
    let omitidas: Vec<String> = columnas_origen
        .iter()
        .filter(|c| !comunes.contains(&c.nombre))
        .map(|c| c.nombre.clone())
        .collect();
    if !omitidas.is_empty() {
        warn!(
            "restore: la tabla {tabla} pierde las columnas {} (no existen en el esquema actual)",
            omitidas.join(", ")
        );
    }

    // El rowid se copia sólo si no es alias de una columna INTEGER PRIMARY KEY.
    let mut lista = comunes.clone();
    if con_rowid && !alias_de_rowid(&columnas_origen) {
        lista.insert(0, "rowid".to_string());
    }
    let seleccion = lista
        .iter()
        .map(|c| format!("\"{}\"", c.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(", ");
    let nombre = tabla.replace('"', "\"\"");

    // Vaciar el destino (en una tabla virtual FTS5 el DELETE recorre sus
    // filas y mantiene el índice coherente).
    sqlx::query(&format!("DELETE FROM {destino}.\"{nombre}\""))
        .execute(&mut *conexion)
        .await
        .map_err(sqlite_err)?;

    // Copiar las filas.
    sqlx::query(&format!(
        "INSERT INTO {destino}.\"{nombre}\" ({seleccion}) \
         SELECT {seleccion} FROM {origen}.\"{nombre}\""
    ))
    .execute(&mut *conexion)
    .await
    .map_err(sqlite_err)?;

    Ok(())
}

/// Restaura la base en vivo desde el backup `name` de `dir`.
///
/// 1. `name` debe ser un nombre válido (`app-*.db`, sin `..` ni separadores).
/// 2. El backup debe pasar `PRAGMA integrity_check` **antes** de tocar nada.
/// 3. Se vuelca el WAL de la base viva (`PRAGMA wal_checkpoint(TRUNCATE)`) a
///    través del pool.
/// 4. Se copia el backup a una copia de trabajo `{base}.restore-{ts}` y se
///    adjunta (`ATTACH ... AS restauracion`) a una conexión propia de la base
///    viva, con `busy_timeout=5000`.
/// 5. Dentro de una transacción se copia el CONTENIDO del backup sobre la
///    base viva: los datos de todas las tablas; los objetos que sólo existen
///    en el backup se crean y `_sqlx_migrations` no se toca.
///
/// El esquema es el de la app en marcha: por eso NO se desactiva el WAL
/// (exigiría un lock EXCLUSIVO que `busy_timeout` no puede esperar y
/// devolvía `database is locked` en producción) y por eso el fichero de la
/// base en vivo no se sustituye.
///
/// Si la base está ocupada (SQLITE_BUSY) se devuelve un error honesto
/// ("server en uso: reintenta tras detener la app"): nunca se simula un
/// restore que no ha ocurrido. El pool no se cierra nunca.
pub async fn restore(pool: &SqlitePool, dir: &Path, name: &str) -> Result<BackupInfo, BackupError> {
    if !is_safe_backup_name(name) {
        return Err(BackupError::Unsafe);
    }
    let origen = dir.join(name);

    // 1. El backup tiene que estar sano ANTES de tocar la base en vivo.
    let check = integrity_check_file(&origen).await?;
    if !check.eq_ignore_ascii_case("ok") {
        return Err(BackupError::Integrity);
    }

    // 2. Destino: el fichero real de la base en vivo.
    let destino = db_file_path(pool).await?;

    // 3. Volcar el WAL de la base viva (vía el pool, sin conexiones nuevas).
    checkpoint_via_pool(pool).await;

    // 4. Copia de trabajo: es la que se adjunta (ATTACH) al restaurar. Se
    //    trabaja sobre una copia para no tocar el backup original.
    let copia = copia_de_trabajo(&destino);
    if let Err(e) = tokio::fs::copy(&origen, &copia).await {
        let _ = tokio::fs::remove_file(&copia).await;
        return Err(io_err(e));
    }
    let bytes = tokio::fs::metadata(&origen)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if let Err(e) = preparar_copia(&copia).await {
        let _ = tokio::fs::remove_file(&copia).await;
        return Err(e);
    }

    // 5. Restaurar el contenido (transacción sobre la base viva).
    let resultado = restaurar_contenido(&destino, &copia).await;

    // 6. La copia de trabajo se borra siempre (haya éxito o error).
    let _ = tokio::fs::remove_file(&copia).await;

    resultado?;

    // 7. Lo escrito queda en el WAL: se vuelca (best effort).
    checkpoint_via_pool(pool).await;

    let creado = tokio::fs::metadata(&origen)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| {
            DateTime::<Local>::from(t)
                .format(CREATED_FORMAT)
                .to_string()
        })
        .unwrap_or_else(|| Local::now().format(CREATED_FORMAT).to_string());

    Ok(BackupInfo {
        name: name.to_string(),
        path: origen.to_string_lossy().to_string(),
        bytes,
        created: creado,
    })
}

/// Lista los backups de `dir`, del más reciente al más antiguo.
///
/// Filtra los ficheros `*.db` (los sidecars `.sha256` quedan fuera) y los
/// ordena por fecha de modificación, descendente.
pub async fn list_backups(dir: &Path) -> Vec<BackupInfo> {
    let mut lector = match tokio::fs::read_dir(dir).await {
        Ok(lector) => lector,
        Err(_) => return Vec::new(),
    };

    let mut backups: Vec<(SystemTime, BackupInfo)> = Vec::new();
    while let Ok(Some(entrada)) = lector.next_entry().await {
        let name = entrada.file_name().to_string_lossy().to_string();
        if !name.ends_with(BACKUP_SUFFIX) {
            continue; // sidecars .sha256 y cualquier otro fichero
        }
        let path = dir.join(&name);
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modificado = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let creado = DateTime::<Local>::from(modificado);
        backups.push((
            modificado,
            BackupInfo {
                name,
                path: path.to_string_lossy().to_string(),
                bytes: meta.len(),
                created: creado.format(CREATED_FORMAT).to_string(),
            },
        ));
    }

    // Del más nuevo al más viejo.
    backups.sort_by(|a, b| b.0.cmp(&a.0));
    backups.into_iter().map(|(_, info)| info).collect()
}

/// Aplica la retención: conserva los `keep` backups más recientes y borra el
/// resto (cada copia arrastra también su sidecar `.sha256`).
///
/// `keep = 0` borra todos los backups del directorio. Devuelve los nombres de
/// los ficheros eliminados.
pub async fn enforce_retention(dir: &Path, keep: usize) -> Result<Vec<String>, BackupError> {
    let mut eliminados = Vec::new();
    // `list_backups` entrega del más nuevo al más viejo: los que quedan tras
    // `keep` son los más antiguos.
    for viejo in list_backups(dir).await.iter().skip(keep) {
        tokio::fs::remove_file(&viejo.path).await.map_err(io_err)?;
        // El sidecar puede no existir: se ignora su fallo.
        let _ = tokio::fs::remove_file(dir.join(format!("{}{SIDECAR_SUFFIX}", viejo.name))).await;
        eliminados.push(viejo.name.clone());
    }
    Ok(eliminados)
}

/// Tarea periódica: backup diario (el primero a los 60 s del arranque) y
/// retención de los `DEFAULT_RETENTION` (`DEFAULT_KEEP`) más recientes.
///
/// El directorio lo decide el orquestador: el arranque de la app (`main.rs`)
/// usará [`DEFAULT_DIR`] (`data/backups`) como `dir`; este módulo no decide
/// la ruta por su cuenta.
///
/// Los errores se trazan con `tracing`; la tarea nunca hace panic.
pub fn spawn_backup_task(pool: SqlitePool, dir: PathBuf) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Deja pasar el arranque (migraciones, primeras peticiones).
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut intervalo = tokio::time::interval(Duration::from_secs(86400));
        loop {
            intervalo.tick().await;
            match backup_now(&pool, &dir).await {
                Ok(copia) => info!("backup creado: {} ({} bytes)", copia.name, copia.bytes),
                Err(e) => warn!("backup falló: {e}"),
            }
            match enforce_retention(&dir, DEFAULT_RETENTION).await {
                Ok(borrados) if !borrados.is_empty() => info!(
                    "retención: {} backup(s) eliminado(s): {}",
                    borrados.len(),
                    borrados.join(", ")
                ),
                Ok(_) => {}
                Err(e) => warn!("retención falló: {e}"),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    /// Directorio base de los tests (uno por proceso).
    fn dir_base() -> PathBuf {
        std::env::temp_dir().join(format!("backup-test-{}", std::process::id()))
    }

    /// Directorio de un test: la base viva (`app.db`) y los backups
    /// (`backups/`) van en directorios distintos, para que la base no cuente
    /// como un backup. Cada test tiene el suyo: `cargo test` corre los tests
    /// en paralelo dentro del mismo proceso (mismo PID).
    fn dir_test(nombre: &str) -> PathBuf {
        dir_base().join(nombre)
    }

    /// Pool sobre un fichero real (`mode=rwc`): la base tiene que estar en
    /// disco para que el backup pueda copiarse.
    async fn pool_de_prueba(dir: &Path) -> SqlitePool {
        let url = format!("sqlite:{}?mode=rwc", dir.join("app.db").display());
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS demo (id INTEGER PRIMARY KEY, texto TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    /// Pool de varias conexiones sobre un fichero real, en modo WAL: es la
    /// situación de producción (el pool de la app tiene varias conexiones y
    /// la base vive en WAL).
    async fn pool_wal_de_varias_conexiones(dir: &Path) -> SqlitePool {
        let url = format!("sqlite:{}?mode=rwc", dir.join("app.db").display());
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .unwrap();
        let modo: String = sqlx::query_scalar("PRAGMA journal_mode=WAL")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            modo.to_ascii_lowercase(),
            "wal",
            "la base de prueba debe quedarse en WAL (como en producción)"
        );
        sqlx::query("CREATE TABLE IF NOT EXISTS demo (id INTEGER PRIMARY KEY, texto TEXT)")
            .execute(&pool)
            .await
            .unwrap();
        pool
    }

    /// Regresión: el restore con el pool ACTIVO y sobre el mismo fichero ya
    /// no falla con `database is locked (code 5)`.
    ///
    /// Antes, `restore` desactivaba el WAL (`PRAGMA journal_mode=DELETE`),
    /// que exige un lock EXCLUSIVO que `busy_timeout` no puede esperar: con
    /// el pool vivo (varias conexiones en WAL) devolvía siempre
    /// `database is locked`, sin copiar nada.
    #[tokio::test]
    async fn restore_con_el_pool_activo_no_falla_con_database_locked() {
        let dir = dir_test("restore-pool-activo");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let backups = dir.join("backups");
        let pool = pool_wal_de_varias_conexiones(&dir).await;

        sqlx::query("INSERT INTO demo (id, texto) VALUES (1, 'uno'), (2, 'dos')")
            .execute(&pool)
            .await
            .unwrap();

        // Backup con 2 filas.
        let copia = backup_now(&pool, &backups).await.unwrap();
        assert!(copia.bytes > 0);

        // La base viva se desvía del backup.
        sqlx::query("INSERT INTO demo (id, texto) VALUES (3, 'tres')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM demo WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();

        // Otra conexión del pool mantiene una lectura abierta: es lo que
        // bloqueaba la base en producción.
        let mut lector = pool.acquire().await.unwrap();
        let mut tx = lector.begin().await.unwrap();
        let abiertas: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM demo")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        assert_eq!(abiertas, 2, "la base viva tiene 2 filas antes de restaurar");

        // El restore, con el pool ACTIVO y sobre el mismo fichero.
        let restaurado = restore(&pool, &backups, &copia.name).await;
        let nombre = match &restaurado {
            Ok(info) => info.name.clone(),
            Err(e) => panic!("restore falló con el pool activo: {e:?}"),
        };
        assert_eq!(nombre, copia.name);

        tx.rollback().await.unwrap();
        drop(lector);

        // El contenido vuelve a ser el del backup.
        let filas: Vec<(i64, String)> = sqlx::query_as("SELECT id, texto FROM demo ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            filas,
            vec![(1, "uno".to_string()), (2, "dos".to_string())],
            "tras el restore la base debe coincidir con el backup"
        );

        // La copia de trabajo no se queda en el disco.
        let mut entradas = tokio::fs::read_dir(&dir).await.unwrap();
        let mut trabajo = Vec::new();
        while let Some(entrada) = entradas.next_entry().await.unwrap() {
            let ruta = entrada.path();
            if ruta.to_string_lossy().contains(RESTORE_SUFFIX) {
                trabajo.push(ruta);
            }
        }
        assert!(trabajo.is_empty(), "quedó la copia de trabajo: {trabajo:?}");

        pool.close().await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn backup_crea_el_fichero_y_list_lo_ve() {
        let dir = dir_test("backup-crea");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let backups = dir.join("backups");
        let pool = pool_de_prueba(&dir).await;

        let copia = backup_now(&pool, &backups).await.unwrap();
        assert!(
            copia.name.starts_with(BACKUP_PREFIX),
            "nombre: {}",
            copia.name
        );
        assert!(
            copia.name.ends_with(BACKUP_SUFFIX),
            "nombre: {}",
            copia.name
        );
        assert!(copia.bytes > 0);
        assert!(
            tokio::fs::try_exists(backups.join(&copia.name))
                .await
                .unwrap(),
            "no se creó {}",
            copia.path
        );
        assert!(
            tokio::fs::try_exists(backups.join(format!("{}{SIDECAR_SUFFIX}", copia.name)))
                .await
                .unwrap(),
            "no se creó el sidecar de {}",
            copia.name
        );

        let lista = list_backups(&backups).await;
        assert_eq!(lista.len(), 1);
        assert_eq!(lista[0].name, copia.name);
        assert_eq!(lista[0].path, copia.path);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn retention_conserva_solo_el_mas_reciente() {
        let dir = dir_test("retention");
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let backups = dir.join("backups");
        tokio::fs::create_dir_all(&backups).await.unwrap();
        let _pool = pool_de_prueba(&dir).await;

        let viejo = format!("{BACKUP_PREFIX}20260101-100000.db");
        let nuevo = format!("{BACKUP_PREFIX}20260102-100000.db");
        tokio::fs::write(backups.join(&viejo), b"viejo")
            .await
            .unwrap();
        // Separación en el tiempo: el orden por mtime debe ser inequívoco.
        tokio::time::sleep(Duration::from_millis(20)).await;
        tokio::fs::write(backups.join(&nuevo), b"nuevo")
            .await
            .unwrap();

        let eliminados = enforce_retention(&backups, 1).await.unwrap();
        assert_eq!(eliminados, vec![viejo.clone()]);
        assert!(!tokio::fs::try_exists(backups.join(&viejo)).await.unwrap());

        let lista = list_backups(&backups).await;
        assert_eq!(lista.len(), 1);
        assert_eq!(lista[0].name, nuevo);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
