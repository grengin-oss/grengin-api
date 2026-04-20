use crate::{
    config::setting::Settings,
    middleware::audit_log::audit_log_middleware,
    routes::{
        admin::admin_routes, auth::auth_routes, branding::branding_routes, chat::chat_routes,
        file::files_routes, mcp::mcp_routes, me::me_routes, message::message_routes,
        models::models_routes, oidc::oidc_routes, open_error::errors_routes,
        swagger_ui::swagger_ui_routes,
    },
    services::{
        analytics_cache::spawn_analytics_cache_refresh,
        audit_logs::spawn_audit_log_retention_worker,
    },
    state::AppState,
};
use anyhow::Error;
use axum::{middleware::from_fn_with_state, routing::get, Json, Router};
use migration::MigratorTrait;
use reqwest::StatusCode;
use serde_json::json;
use tower_http::cors::{Any, CorsLayer};

async fn sample_root() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::OK,
        Json(json!({"status":"Okay","version":env!("CARGO_PKG_VERSION")})),
    )
}

pub async fn init_app() -> Result<(), Error> {
    tracing_subscriber::fmt::init();
    let settings = Settings::from_env()?;
    let address = format!("{}:{}", settings.server.host, settings.server.port);

    // Run migrations BEFORE creating app state (which loads data from DB)
    let database = sea_orm::Database::connect(&settings.auth.database_url).await?;
    migration::Migrator::up(&database, None).await?;
    drop(database); // Close this connection, AppState will create its own

    let app_state = AppState::from_settings(settings).await?;
    spawn_analytics_cache_refresh(app_state.database.clone());
    spawn_audit_log_retention_worker(app_state.database.clone());
    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_origin(Any)
        .allow_headers(Any)
        .allow_credentials(false);
    let app = Router::new()
        .route("/", get(sample_root))
        .merge(swagger_ui_routes())
        .merge(oidc_routes())
        .merge(chat_routes())
        .merge(files_routes())
        .merge(message_routes())
        .merge(admin_routes())
        .merge(mcp_routes())
        .merge(me_routes())
        .merge(branding_routes())
        .merge(models_routes())
        .merge(auth_routes())
        .merge(errors_routes())
        .layer(from_fn_with_state(app_state.clone(), audit_log_middleware))
        .layer(cors)
        .with_state(app_state);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("Started listening to {}", address);
    axum::serve(listener, app).await?;
    Ok(())
}
