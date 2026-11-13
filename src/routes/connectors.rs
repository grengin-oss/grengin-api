// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{handlers::connectors, state::SharedState};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, header::CACHE_CONTROL},
    routing::{get, post},
};

pub fn connectors_routes() -> Router<SharedState> {
    Router::new()
        .route("/connectors", get(connectors::list))
        .route(
            "/connectors/{connector_key}/connect",
            post(connectors::connect),
        )
        .route("/connectors/{connection_id}", get(connectors::status))
        .route(
            "/connectors/{connection_id}/complete",
            post(connectors::complete),
        )
        .route(
            "/connectors/{connection_id}/disconnect",
            post(connectors::disconnect),
        )
        .layer(DefaultBodyLimit::max(16384))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
}
