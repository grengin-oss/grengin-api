use crate::{auth::claims::Claims, handlers::models::get_list_models, state::SharedState};
use axum::{middleware::from_extractor, routing::get, Router};

pub fn models_routes() -> Router<SharedState> {
    Router::new()
        .route("/models", get(get_list_models))
        .route_layer(from_extractor::<Claims>())
}
