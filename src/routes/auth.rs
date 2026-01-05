use axum::{Router, routing::post};
use crate::{handlers::auth::handle_refresh_token, state::SharedState};

pub fn auth_routes() -> Router<SharedState> {
   Router::new()
    .route("/auth/refresh",post(handle_refresh_token))
}