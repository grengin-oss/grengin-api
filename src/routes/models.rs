// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{auth::claims::Claims, handlers::models::get_list_models, state::SharedState};
use axum::{Router, middleware::from_extractor, routing::get};

pub fn models_routes() -> Router<SharedState> {
    Router::new()
        .route("/models", get(get_list_models))
        .route_layer(from_extractor::<Claims>())
}
