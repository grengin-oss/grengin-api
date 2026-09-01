// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth::provider_config::OidcProviderConfiguration;

pub struct SsoProviderTemplate {
    pub name: String,
    pub provider: String,
    pub issuer_url: String,
    pub redirect_url: String,
    pub tenant_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct SsoProvider {
    pub id: Uuid,
    pub provider: String,
    pub name: String,
    pub client_id: String,
    #[serde(rename = "client_secret_preview")]
    #[schema(value_type = String, rename = "client_secret_preview")]
    pub client_secret: String,
    pub issuer_url: String,
    pub redirect_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,
    pub allowed_domains: Vec<String>,
    pub is_enabled: bool,
    pub use_grengin_proxy: bool,
    pub jit_provisioning: bool,
    pub configuration: OidcProviderConfiguration,
    pub grengin_proxy_available: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GrenginProxySetupRequest {
    pub allowed_domains: Vec<String>,
    pub tenant_id: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct EditableField {
    pub editable: bool,
    pub value: String,
}

#[derive(Serialize, ToSchema)]
pub struct SsoProviderEditable {
    pub id: Uuid,
    pub provider: EditableField,
    pub name: EditableField,
    pub client_id: EditableField,
    #[serde(rename = "client_secret_preview")]
    #[schema(value_type = String, rename = "client_secret_preview")]
    pub client_secret: Option<EditableField>,
    pub issuer_url: EditableField,
    pub redirect_url: EditableField,
    pub tenant_id: Option<EditableField>,
    pub allowed_domains: Vec<String>,
    pub is_enabled: bool,
    pub jit_provisioning: bool,
    pub configuration: OidcProviderConfiguration,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SsoProviderUpdate {
    pub provider: Option<String>,
    pub tenant_id: Option<String>,
    pub name: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub issuer_url: Option<String>,
    pub redirect_url: Option<String>,
    pub frontend_hosted_url: Option<String>,
    pub validation_token: Option<String>,
    pub allowed_domains: Option<Vec<String>>,
    pub is_enabled: Option<bool>,
    pub jit_provisioning: Option<bool>,
    pub configuration: Option<OidcProviderConfiguration>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SsoProviderCreate {
    pub provider: String,
    pub name: String,
    pub client_id: String,
    pub client_secret: String,
    pub issuer_url: String,
    pub redirect_url: String,
    pub tenant_id: Option<String>,
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    #[serde(default = "default_true")]
    pub jit_provisioning: bool,
    #[serde(default)]
    pub configuration: OidcProviderConfiguration,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SsoProviderValidationRequest {
    pub provider: Option<String>,
    pub tenant_id: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub issuer_url: Option<String>,
    pub redirect_url: Option<String>,
    pub frontend_hosted_url: Option<String>,
    pub configuration: Option<OidcProviderConfiguration>,
}

#[derive(Serialize, ToSchema)]
pub struct SsoProviderValidationResponse {
    pub valid: bool,
    pub message: String,
    pub redirect_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_token_expires_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_payload_uses_snake_case_with_camel_case_configuration() {
        let request: SsoProviderCreate = serde_json::from_value(serde_json::json!({
            "provider": "keycloak",
            "name": "Keycloak",
            "client_id": "client-id",
            "client_secret": "client-secret",
            "issuer_url": "https://id.example.com/realms/acme",
            "redirect_url": "https://app.example.com/auth/keycloak/callback",
            "configuration": {
                "version": "1.0",
                "scopes": ["openid", "email"],
                "authorizationParams": {"prompt": "login"},
                "emailLinking": "verifiedEmail",
                "autoRedirect": false
            }
        }))
        .expect("snake_case provider request");

        assert_eq!(request.client_id, "client-id");
        assert_eq!(request.configuration.scopes, ["openid", "email"]);
    }

    #[test]
    fn update_payload_rejects_camel_case_api_fields() {
        let request = serde_json::from_value::<SsoProviderUpdate>(serde_json::json!({
            "clientId": "silently-ignored-without-deny-unknown-fields"
        }));
        assert!(request.is_err());
    }
}
