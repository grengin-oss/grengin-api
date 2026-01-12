use axum::{Router, routing::get};
use crate::{handlers::branding::get_branding, state::SharedState};

pub fn branding_routes() -> Router<SharedState> {
    Router::new()
        .route("/branding", get(get_branding))
}
