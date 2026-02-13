use axum::{Router, routing::get};
use crate::{handlers::me::{get_my_administered_departments, get_my_permissions}, state::SharedState};

pub fn me_routes() -> Router<SharedState> {
    Router::new()
        .route("/me/permissions", get(get_my_permissions))
        .route("/me/administered-departments", get(get_my_administered_departments))
}
