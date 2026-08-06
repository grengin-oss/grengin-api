// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde::Deserialize;
use utoipa::ToSchema;

// Free-form string identifier for auth providers (e.g., "google", "azure", "keycloak", "authentik", "okta")
// The provider serves as a URL slug and display name. Actual OIDC behavior is determined by configuration.
pub type AuthProvider = String;

#[derive(Deserialize)]
pub struct StartParams {
    pub redirect_uri: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct AuthCallback {
    pub code: Option<String>,
    pub state: String,
    pub assertion: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Clone, Copy)]
pub enum CallbackExchangeMode {
    Auto,
    AzureMobilePublic,
}
