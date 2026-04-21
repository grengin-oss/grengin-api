use crate::{handlers::auth::handle_refresh_token, state::SharedState};
use axum::{routing::post, Router};

pub fn auth_routes() -> Router<SharedState> {
    Router::new().route("/auth/refresh", post(handle_refresh_token))
}
