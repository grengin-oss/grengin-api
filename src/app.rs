use crate::{
    config::setting::Settings,
    middleware::audit_log::audit_log_middleware,
    routes::{
        admin::admin_routes, artifacts::artifacts_routes, auth::auth_routes,
        branding::branding_routes, chat::chat_routes, file::files_routes, mcp::mcp_routes,
        me::me_routes, message::message_routes, models::models_routes, oidc::oidc_routes,
        open_error::errors_routes, projects::projects_routes, skills::skills_routes,
        swagger_ui::swagger_ui_routes,
    },
    services::{
        analytics_cache::spawn_analytics_cache_refresh,
        audit_logs::spawn_audit_log_retention_worker,
    },
    state::AppState,
};
use anyhow::Error;
use axum::{Json, Router, extract::DefaultBodyLimit, middleware::from_fn_with_state, routing::get};
use migration::MigratorTrait;
use reqwest::StatusCode;
use serde_json::json;
use axum::http::HeaderValue;
use tower_http::cors::{Any, AllowOrigin, CorsLayer};

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
    // Restrict CORS to the configured instance origin so a phishing webapp
    // pointed at this API cannot receive auth responses in a cross-origin fetch.
    // CORS_ALLOWED_ORIGINS overrides (comma-separated, useful in dev).
    // Falls back to REDIRECT_URL (the canonical instance base URL).
    // Falls back to Any only when neither env var is set (local dev without config).
    let cors_allow_origin: AllowOrigin = std::env::var("CORS_ALLOWED_ORIGINS")
        .or_else(|_| std::env::var("REDIRECT_URL"))
        .ok()
        .and_then(|raw| {
            let origins: Vec<HeaderValue> = raw
                .split(',')
                .map(|s| s.trim().trim_end_matches('/').to_owned())
                .filter_map(|s| s.parse::<HeaderValue>().ok())
                .collect();
            if origins.is_empty() { None } else { Some(AllowOrigin::list(origins)) }
        })
        .unwrap_or_else(|| AllowOrigin::any());
    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_origin(cors_allow_origin)
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
        .merge(projects_routes())
        .merge(skills_routes())
        .merge(artifacts_routes())
        .merge(auth_routes())
        .merge(errors_routes())
        .layer(DefaultBodyLimit::max(50 * 1024 * 1024))
        .layer(from_fn_with_state(app_state.clone(), audit_log_middleware))
        .layer(cors)
        .with_state(app_state);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("Started listening to {}", address);
    axum::serve(listener, app).await?;
    Ok(())
}
