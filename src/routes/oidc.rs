use axum::{Router, routing::get};
use crate::{handlers::oidc::{oidc_login_start, oidc_oauth_callback}, state::SharedState};

pub fn oidc_routes() -> Router<SharedState> {
   Router::new()
    .route("/auth/{provider}",get(oidc_login_start))
    .route("/auth/{provider}/callback", get(oidc_oauth_callback))
}