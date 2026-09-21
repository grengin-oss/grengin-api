// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    config::setting::Settings,
    middleware::audit_log::audit_log_middleware,
    routes::{
        admin::admin_routes, artifacts::artifacts_routes, auth::auth_routes,
        branding::branding_routes, chat::chat_routes, discovery::discovery_routes,
        file::files_routes, mcp::mcp_routes, me::me_routes, me_skills::me_skills_routes,
        message::message_routes, models::models_routes, oidc::oidc_routes,
        open_error::errors_routes, projects::projects_routes, skills::skills_routes,
        swagger_ui::swagger_ui_routes,
    },
    services::{
        analytics_cache::spawn_analytics_cache_refresh,
        audit_logs::spawn_audit_log_retention_worker, deployment_health::load_deployment_health,
        file_storage::prepare_storage_root, startup_migrations::run_startup_migrations,
    },
    state::{AppState, SharedState},
};
use anyhow::Error;
use axum::http::HeaderValue;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    middleware::from_fn_with_state,
    routing::get,
};
use reqwest::StatusCode;
use serde_json::json;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

async fn sample_root(
    State(app_state): State<SharedState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let health = load_deployment_health(&app_state.database).await;
    let status = if health.is_ready() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    let body = serde_json::to_value(health).unwrap_or_else(|_| {
        json!({
            "status": "Degraded",
            "version": env!("CARGO_PKG_VERSION"),
        })
    });
    (status, Json(body))
}

pub async fn init_app() -> Result<(), Error> {
    tracing_subscriber::fmt::init();
    let settings = Settings::from_env()?;
    let address = format!("{}:{}", settings.server.host, settings.server.port);

    prepare_storage_root(&settings.file_storage_root)
        .await
        .map_err(|error| anyhow::anyhow!("file storage is unavailable: {error:?}"))?;

    if settings.auth.auto_migrate {
        let database = sea_orm::Database::connect(&settings.auth.database_url).await?;
        run_startup_migrations(&database).await?;
    }

    let app_state = AppState::from_settings(settings).await?;
    spawn_analytics_cache_refresh(app_state.database.clone());
    spawn_audit_log_retention_worker(app_state.database.clone());
    let configured_origins = std::env::var("CORS_ALLOWED_ORIGINS")
        .or_else(|_| std::env::var("REDIRECT_URL"))
        .ok();
    let cors_allow_origin = configured_origins.map_or_else(AllowOrigin::any, |raw| {
        let origins = raw
            .split(',')
            .map(|origin| origin.trim().trim_end_matches('/'))
            .filter(|origin| !origin.is_empty())
            .filter_map(|origin| origin.parse::<HeaderValue>().ok())
            .collect::<Vec<_>>();
        AllowOrigin::list(origins)
    });
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
        .merge(discovery_routes())
        .merge(files_routes())
        .merge(message_routes())
        .merge(admin_routes())
        .merge(mcp_routes())
        .merge(me_routes())
        .merge(me_skills_routes())
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
