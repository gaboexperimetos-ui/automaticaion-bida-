//! Application configuration — loaded from environment variables.
//! Secrets are never hardcoded. All sensitive values come from .env.

use std::env;
use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

/// Default for `DB_MAX_CONNECTIONS`: 4 connections.
///
/// Cada conexion de SQLite lleva su propia cache de paginas, asi que el
/// tamano del pool es un multiplicador directo de la RAM del proceso.
fn default_db_max_connections() -> u32 {
    4
}

/// Default for `MCP_TIMEOUT_MS`: 5000 ms (5 s) por llamada MCP.
fn default_mcp_timeout_ms() -> u64 {
    5_000
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub session_secret: String,
    pub db_path: PathBuf,
    pub notes_dir: PathBuf,
    pub openrouter_api_key: Option<String>,
    pub openrouter_model: String,
    pub openrouter_max_tokens: u32,
    pub openrouter_max_cost_usd: f64,
    pub mikareno_mcp_url: String,
    pub genoffice_mcp_url: String,
    pub custom_mcp_api_key: Option<String>,
    pub rust_log: String,
    /// Cuando es `true`, la app no exige sesion (modelo "sin login",
    /// single-user local). Activable con `AUTH_DISABLED=true` en el entorno.
    /// Por defecto `false`: produccion conserva el login con contrasena.
    pub auth_disabled: bool,
    /// Token exigido para completar el registro publico de cuentas.
    /// Se lee de la variable de entorno `REGISTRATION_TOKEN`. Si esa variable
    /// no esta definida (o su valor queda vacio tras `trim()`), el registro
    /// publico de cuentas permanece deshabilitado.
    #[serde(default)]
    pub registration_token: Option<String>,
    /// Numero maximo de conexiones del pool de SQLite.
    /// Se lee de `DB_MAX_CONNECTIONS` (por defecto 4). Es un tope DURO: cada
    /// conexion tiene su propia cache de paginas y su propia posicion en el
    /// WAL, asi que mas conexiones = mas RAM y mas contencion (y por tanto
    /// mas "database is locked"). El valor se corrige a un minimo de 1.
    #[serde(default = "default_db_max_connections")]
    pub db_max_connections: u32,
    /// Presupuesto de tiempo (ms) para UNA llamada MCP.
    /// Se lee de `MCP_TIMEOUT_MS` (por defecto 5000). Los handlers deben
    /// usar este valor en vez de fijar timeouts a pelo: asi el tiempo maximo
    /// de un MCP se ajusta sin recompilar.
    #[serde(default = "default_mcp_timeout_ms")]
    pub mcp_timeout_ms: u64,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    Missing(String),
    #[error("Invalid value for {0}: {1}")]
    Invalid(String, String),
    #[error("Invalid integer for {0}: {1}")]
    InvalidInt(String, String),
    #[error("Invalid float for {0}: {1}")]
    InvalidFloat(String, String),
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        // Helper: get required env var
        let get = |key: &str| -> Result<String, ConfigError> {
            env::var(key).map_err(|_| ConfigError::Missing(key.to_string()))
        };

        // Helper: get optional env var with default
        let get_or = |key: &str, default: &str| -> String {
            env::var(key).unwrap_or_else(|_| default.to_string())
        };

        let host = get_or("HOST", "127.0.0.1");

        let port: u16 = get_or("PORT", "8090").parse::<u16>().map_err(|_| {
            ConfigError::Invalid(
                "PORT".to_string(),
                "must be a valid port number".to_string(),
            )
        })?;

        let session_secret = get("SESSION_SECRET")?;
        if session_secret.len() < 32 {
            return Err(ConfigError::Invalid(
                "SESSION_SECRET".to_string(),
                "must be at least 32 characters".to_string(),
            ));
        }

        let db_path = PathBuf::from(get_or("DB_PATH", "./data/app.db"));
        let notes_dir = PathBuf::from(get_or("NOTES_DIR", "./notes"));

        // OpenRouter (optional — app works without it, just no AI features)
        // Una clave vacía (OPENROUTER_API_KEY=) cuenta como "sin clave":
        // evita anunciar `openrouter_configured: true` con un string vacío.
        let openrouter_api_key = env::var("OPENROUTER_API_KEY")
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty());

        let openrouter_model = get_or("OPENROUTER_MODEL", "cohere/north-mini-code:free");

        let openrouter_max_tokens: u32 = get_or("OPENROUTER_MAX_TOKENS", "8192")
            .parse::<u32>()
            .map_err(|e: std::num::ParseIntError| {
                ConfigError::InvalidInt("OPENROUTER_MAX_TOKENS".to_string(), e.to_string())
            })?;

        let openrouter_max_cost_usd: f64 = get_or("OPENROUTER_MAX_COST_USD", "2.0")
            .parse::<f64>()
            .map_err(|e: std::num::ParseFloatError| {
                ConfigError::InvalidFloat("OPENROUTER_MAX_COST_USD".to_string(), e.to_string())
            })?;

        // MCP endpoints
        let mikareno_mcp_url = get_or("MIKARENO_MCP_URL", "http://127.0.0.1:8080/mcp");
        let genoffice_mcp_url = get_or("GENOFFICE_MCP_URL", "http://127.0.0.1:8453/mcp");

        // Custom MCP API key (optional)
        let custom_mcp_api_key = env::var("CUSTOM_MCP_API_KEY").ok();

        let rust_log = get_or("RUST_LOG", "study_platform=info,tower_http=info");

        // "Sin login" (single-user local). Off by default so production keeps
        // password auth; enable explicitly with AUTH_DISABLED=true.
        let auth_disabled = matches!(
            get_or("AUTH_DISABLED", "false").to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );

        // Token del registro publico de cuentas. Ausente (o vacio) =>
        // el registro publico queda deshabilitado.
        let registration_token = env::var("REGISTRATION_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty());

        // Pool de SQLite: tope duro de conexiones (RAM + contencion del WAL).
        let db_max_connections: u32 = get_or("DB_MAX_CONNECTIONS", "4")
            .parse::<u32>()
            .map_err(|e: std::num::ParseIntError| {
                ConfigError::InvalidInt("DB_MAX_CONNECTIONS".to_string(), e.to_string())
            })?
            .max(1); // un pool de 0 conexiones no es un pool

        // Presupuesto por llamada MCP. Los handlers deben leerlo de aqui.
        let mcp_timeout_ms: u64 = get_or("MCP_TIMEOUT_MS", "5000").parse::<u64>().map_err(
            |e: std::num::ParseIntError| {
                ConfigError::InvalidInt("MCP_TIMEOUT_MS".to_string(), e.to_string())
            },
        )?;

        Ok(Config {
            host,
            port,
            session_secret,
            db_path,
            notes_dir,
            openrouter_api_key,
            openrouter_model,
            openrouter_max_tokens,
            openrouter_max_cost_usd,
            mikareno_mcp_url,
            genoffice_mcp_url,
            custom_mcp_api_key,
            rust_log,
            auth_disabled,
            registration_token,
            db_max_connections,
            mcp_timeout_ms,
        })
    }

    /// Presupuesto de tiempo de una llamada MCP, como `Duration`.
    ///
    /// Atajo para los handlers: `state.config.mcp_timeout()` en vez de
    /// `Duration::from_millis(state.config.mcp_timeout_ms)`.
    pub fn mcp_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.mcp_timeout_ms)
    }

    /// Public-facing configuration (no secrets). Safe to return via API.
    pub fn public(&self) -> PublicConfig {
        PublicConfig {
            openrouter_configured: self.openrouter_api_key.is_some(),
            openrouter_model: self.openrouter_model.clone(),
            mikareno_mcp_url: self.mikareno_mcp_url.clone(),
            genoffice_mcp_url: self.genoffice_mcp_url.clone(),
            custom_mcp_enabled: self.custom_mcp_api_key.is_some(),
            auth_disabled: self.auth_disabled,
        }
    }
}

/// Non-sensitive configuration — safe to expose via API.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PublicConfig {
    pub openrouter_configured: bool,
    pub openrouter_model: String,
    pub mikareno_mcp_url: String,
    pub genoffice_mcp_url: String,
    pub custom_mcp_enabled: bool,
    #[serde(default)]
    pub auth_disabled: bool,
}
