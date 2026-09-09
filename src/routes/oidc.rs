// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    handlers::oidc::{
        apple_oauth_callback_form, azure_mobile_oauth_callback_get,
        azure_mobile_oauth_callback_post, list_auth_providers, oidc_login_start,
        oidc_oauth_callback_get, oidc_oauth_callback_post,
    },
    state::SharedState,
};
use axum::{Router, extract::DefaultBodyLimit, routing::get};

pub fn oidc_routes() -> Router<SharedState> {
    Router::new()
        .route("/auth/providers", get(list_auth_providers))
        .route("/auth/{provider}", get(oidc_login_start))
        .route(
            "/auth/apple/callback",
            axum::routing::post(apple_oauth_callback_form).layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/auth/{provider}/callback",
            get(oidc_oauth_callback_get).post(oidc_oauth_callback_post),
        )
        .route(
            "/auth/azure/mobile/callback",
            get(azure_mobile_oauth_callback_get).post(azure_mobile_oauth_callback_post),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_form_callback_coexists_with_dynamic_oidc_callback() {
        let _ = oidc_routes();
    }
}
