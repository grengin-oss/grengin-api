use anyhow::Result;
use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

mod db_manager;
mod handler;
mod read_only;

use crate::db_manager::DatabaseManager;
use crate::handler::SqlxDatabaseHandler;

#[derive(Parser, Debug)]
#[command(name = "sqlx-mcp")]
#[command(about = "SQLx MCP Server - Provides SQL query and modification tools")]
struct Args {
    /// Database connection URL
    #[arg(short, long)]
    database_url: String,

    /// Maximum number of connections
    #[arg(long, default_value = "1")]
    max_connections: u32,

    /// Connection timeout in seconds
    #[arg(long, default_value = "30")]
    timeout: u64,

    /// Log level (error/warn/info/debug/trace)
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Enforce read-only server mode (blocks sql_exec)
    #[arg(long, default_value_t = false)]
    read_only: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive(
                args.log_level
                    .parse()
                    .unwrap_or_else(|_| tracing::Level::INFO.into()),
            ),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    info!("Starting SQLx MCP Server");
    info!("Read-only mode: {}", args.read_only);

    let db_manager = DatabaseManager::new(
        &args.database_url,
        args.max_connections,
        args.timeout,
        args.read_only,
    )?;
    let db_manager = Arc::new(db_manager);

    // Avoid blocking MCP initialize on DB startup latency.
    {
        let db = Arc::clone(&db_manager);
        tokio::spawn(async move {
            match db.test_connection().await {
                Ok(()) => info!("Database connection test successful"),
                Err(e) => error!("Database connection test failed: {e}"),
            }
        });
    }

    let handler = SqlxDatabaseHandler::new(db_manager);
    let service = handler.serve(stdio()).await.inspect_err(|e| {
        error!("Server startup failed: {e:?}");
    })?;

    service.waiting().await?;
    Ok(())
}
