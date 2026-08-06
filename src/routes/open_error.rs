// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    handlers::open_error::{get_app_error_catalog, get_auth_error_catalog},
    state::SharedState,
};
use axum::Router;

pub fn errors_routes() -> Router<SharedState> {
    Router::new()
        .route("/errors/app", axum::routing::get(get_app_error_catalog))
        .route("/errors/auth", axum::routing::get(get_auth_error_catalog))
}
