use axum::{Router, middleware::from_extractor, routing::get};
use crate::{auth::claims::Claims, handlers::models::list_models, state::SharedState};

pub fn models_routes() -> Router<SharedState> {
    Router::new()
        .route("/models", get(list_models))
        .route_layer(from_extractor::<Claims>())
}