use axum::{Router, routing::get};
use crate::{handlers::models::list_models, state::SharedState};

pub fn models_routes() -> Router<SharedState> {
    Router::new()
        .route("/models", get(list_models))
}
