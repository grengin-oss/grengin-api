use crate::{handlers::branding::get_branding, state::SharedState};
use axum::{routing::get, Router};

pub fn branding_routes() -> Router<SharedState> {
    Router::new().route("/branding", get(get_branding))
}
