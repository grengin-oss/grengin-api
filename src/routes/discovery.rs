// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::{Router, routing::get};

use crate::{
    handlers::discovery::{
        get_ai_provider_plugin, get_auth_provider_template, list_ai_provider_plugins,
        list_auth_provider_templates,
    },
    state::SharedState,
};

pub fn discovery_routes() -> Router<SharedState> {
    Router::new()
        .route(
            "/discovery/auth-providers",
            get(list_auth_provider_templates),
        )
        .route(
            "/discovery/auth-providers/{provider}",
            get(get_auth_provider_template),
        )
        .route("/discovery/ai-providers", get(list_ai_provider_plugins))
        .route(
            "/discovery/ai-providers/{provider}",
            get(get_ai_provider_plugin),
        )
}
