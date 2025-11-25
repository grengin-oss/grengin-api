use axum::{Json, Router, routing::get};
use reqwest::StatusCode;
use serde_json::json;
use anyhow::Error;
use migration::MigratorTrait;

use crate::{config::setting::Settings, routes::{google::google_routes, swagger_ui::swagger_ui_routes}, state::AppState};

async fn sample_root() -> (StatusCode,Json<serde_json::Value>){
    (StatusCode::OK,Json(json!({"status":"Okay","version":env!("CARGO_PKG_VERSION")})))
}

pub async fn init_app() -> Result<(),Error>{
    tracing_subscriber::fmt::init();
    let settings = Settings::from_env()?;
    let address = format!("{}:{}",settings.server.host,settings.server.port);
    let app_state = AppState::from_settings(settings).await?;
    migration::Migrator::up(&app_state.database, None).await?; // Auto migration
    let app = Router::new()
      .route("/", get(sample_root))
      .merge(swagger_ui_routes())
      .merge(google_routes())
      .with_state(app_state);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("Started listening to {}",address);
    axum::serve(listener, app).await?;
 Ok(())
}