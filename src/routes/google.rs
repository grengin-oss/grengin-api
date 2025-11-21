use axum::{Router, routing::get};
use crate::{handlers::google_oauth::{google_login_start, google_oauth_callback}, state::SharedState};

pub fn google_routes() -> Router<SharedState> {
   Router::new()
    .route("/auth/google/login",get(google_login_start))
    .route("/auth/google/callback", get(google_oauth_callback))
}