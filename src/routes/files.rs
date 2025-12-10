use axum::{Router, routing::post};
use crate::{handlers::files::upload_file, state::SharedState};

pub fn files_routes() -> Router<SharedState> {
   Router::new()
    .route("/files",post(upload_file))
}