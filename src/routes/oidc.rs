use axum::{Router, routing::get};
use crate::{handlers::oidc::{oidc_login_start, oidc_oauth_callback}, state::SharedState};

pub fn oidc_routes() -> Router<SharedState> {
   Router::new()
    .route("/auth/{oidc_provider}/login",get(oidc_login_start))
    .route("/auth/{oidc_provider}/callback", get(oidc_oauth_callback))
}