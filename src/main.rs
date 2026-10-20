//! Study Platform — Entry point.
//!
//! Binary entry point. Library logic lives in src/lib.rs.
//! Run with: cargo run (development).
//!
//! CLI commands:
//!   cargo run                  — start the server
//!   cargo run -- --migrate     — run migrations only
//!   cargo run -- --create-admin <username> — create admin user

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use dotenvy::dotenv;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use study_platform::{config::Config, db, handlers, AppState};

#[derive(Parser, Debug)]
#[command(
    name = "study-platform",
    version,
    about = "Personal academic productivity platform"
)]
struct Cli {
    /// Bind address
    #[arg(short, long, default_value = "0.0.0.0:8090")]
    addr: SocketAddr,

    /// Run database migrations and exit
    #[arg(long)]
    migrate: bool,

    /// Create initial admin user
    #[arg(long, value_name = "USERNAME")]
    create_admin: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let cli = Cli::parse();

    // Initialize tracing
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("study_platform=info,tower_http=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Load configuration
    let config = Config::from_env()?;
    info!(
        "Configuration loaded: host={}, port={}, db={}",
        config.host,
        config.port,
        config.db_path.display()
    );

    // Initialize database
    let db = db::init(&config).await?;
    info!("Database initialized: {}", config.db_path.display());

    // Run migrations
    db::run_migrations(&db).await?;
    info!("Database migrations complete");

    // Verify schema
    if let Err(e) = db::verify_schema(&db).await {
        tracing::warn!("Schema verification warning: {}", e);
    }

    // Backup diario (F9): primer backup 60s tras el arranque, luego cada 24h.
    study_platform::backup::spawn_backup_task(
        db.clone(),
        std::path::PathBuf::from(study_platform::backup::DEFAULT_DIR),
    );

    // Create admin user if requested
    if let Some(username) = cli.create_admin {
        db::create_admin(&db, &username).await?;
    }

    if cli.migrate {
        info!("Migrations applied. Exiting.");
        return Ok(());
    }

    // Cliente Mikareno compartido (caché de 180s entre requests).
    let mikareno = study_platform::integrations::mikareno::MikarenoClient::from_config(
        &config.mikareno_mcp_url,
    );

    // Build router with state
    let app_state = Arc::new(AppState {
        config: config.clone(),
        db: db.clone(),
        mikareno: mikareno.clone(),
        cache_mikareno: Arc::new(study_platform::academic::calendar_data::CacheMikareno::default()),
        cache_catalogo: Arc::new(study_platform::ai::modelos::CacheCatalogo::default()),
        start_time: std::time::Instant::now(),
    });

    // Precalienta la caché del MCP en segundo plano: la primera petición real
    // ya no paga el coste de los scrapes en frío.
    if let Some(client) = mikareno.clone() {
        tokio::spawn(async move {
            if client.initialize().await.is_ok() {
                let _ = client.perfil().await;
                // Una sola llamada que trae promedios, horario, calendario y tareas.
                let _ = client
                    .call_tool("get_student_full", serde_json::json!({ "bg": true }))
                    .await;
                info!("Mikareno cache warmed");
            }
        });
    }

    let app = handlers::router(app_state).layer(TraceLayer::new_for_http());

    // Start server
    let listener = TcpListener::bind(&cli.addr).await?;
    info!("Study Platform listening on http://{}", cli.addr);

    axum::serve(listener, app).await?;

    Ok(())
}
