use crate::{
    auth::claims::Claims,
    handlers::artifacts::{delete_artifact, get_artifact, list_conversation_artifacts},
    state::SharedState,
};
use axum::{
    Router,
    middleware::from_extractor,
    routing::get,
};

pub fn artifacts_routes() -> Router<SharedState> {
    Router::new()
        .route("/artifacts/{id}", get(get_artifact).delete(delete_artifact))
        .route("/conversations/{id}/artifacts", get(list_conversation_artifacts))
        .route_layer(from_extractor::<Claims>())
}
