// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl, core::CoreProviderMetadata};
use reqwest::{Client as ReqwestClient, Url};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use thiserror::Error;
use utoipa::ToSchema;

use crate::config::setting::OidcClient;

pub const OIDC_PROVIDER_CONFIG_VERSION: &str = "1.0";

const RESERVED_AUTHORIZATION_PARAMS: &[&str] = &[
    "client_id",
    "code_challenge",
    "code_challenge_method",
    "nonce",
    "redirect_uri",
    "response_type",
    "scope",
    "state",
];

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub enum EmailLinkingMode {
    Disabled,
    #[default]
    VerifiedEmail,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct OidcProviderConfiguration {
    pub version: String,
    pub scopes: Vec<String>,
    pub authorization_params: BTreeMap<String, String>,
    pub email_linking: EmailLinkingMode,
    pub auto_redirect: bool,
}

impl Default for OidcProviderConfiguration {
    fn default() -> Self {
        Self {
            version: OIDC_PROVIDER_CONFIG_VERSION.to_string(),
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            authorization_params: BTreeMap::new(),
            email_linking: EmailLinkingMode::VerifiedEmail,
            auto_redirect: false,
        }
    }
}

impl OidcProviderConfiguration {
    pub fn from_value(value: Option<&serde_json::Value>) -> Result<Self, ProviderConfigError> {
        let configuration = match value {
            Some(value) => serde_json::from_value(value.clone())
                .map_err(|error| ProviderConfigError::InvalidConfiguration(error.to_string()))?,
            None => Self::default(),
        };
        configuration.validate()?;
        Ok(configuration)
    }

    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        if self.version != OIDC_PROVIDER_CONFIG_VERSION {
            return Err(ProviderConfigError::UnsupportedVersion(
                self.version.clone(),
            ));
        }
        if self.scopes.is_empty() || self.scopes.len() > 32 {
            return Err(ProviderConfigError::InvalidScopes);
        }

        let mut seen = HashSet::new();
        for scope in &self.scopes {
            let scope = scope.trim();
            if scope.is_empty()
                || scope.len() > 128
                || scope.chars().any(char::is_whitespace)
                || !seen.insert(scope.to_string())
            {
                return Err(ProviderConfigError::InvalidScopes);
            }
        }
        if !seen.contains("openid") {
            return Err(ProviderConfigError::MissingOpenIdScope);
        }

        for (key, value) in &self.authorization_params {
            let normalized = key.trim().to_ascii_lowercase();
            if normalized.is_empty()
                || key.len() > 128
                || value.len() > 2048
                || RESERVED_AUTHORIZATION_PARAMS.contains(&normalized.as_str())
            {
                return Err(ProviderConfigError::InvalidAuthorizationParameter(
                    key.clone(),
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderConfigError {
    #[error("authorization parameter '{0}' is reserved or invalid")]
    InvalidAuthorizationParameter(String),
    #[error("provider configuration is invalid: {0}")]
    InvalidConfiguration(String),
    #[error("provider slug is invalid")]
    InvalidProviderSlug,
    #[error("provider URL is invalid")]
    InvalidUrl,
    #[error("OIDC scopes are invalid")]
    InvalidScopes,
    #[error("OIDC providers must request the openid scope")]
    MissingOpenIdScope,
    #[error("provider configuration version '{0}' is unsupported")]
    UnsupportedVersion(String),
}

pub fn normalize_provider_slug(value: &str) -> Result<String, ProviderConfigError> {
    let slug = value.trim().to_ascii_lowercase();
    let mut chars = slug.chars();
    let starts_with_letter = chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase());
    if !starts_with_letter
        || slug.len() > 63
        || !slug.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
        || slug.ends_with('-')
    {
        return Err(ProviderConfigError::InvalidProviderSlug);
    }
    Ok(slug)
}

pub fn validate_provider_url(
    value: &str,
    allow_loopback_http: bool,
) -> Result<Url, ProviderConfigError> {
    let url = Url::parse(value).map_err(|_| ProviderConfigError::InvalidUrl)?;
    let loopback = url.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if url.scheme() != "https" && !(allow_loopback_http && url.scheme() == "http" && loopback) {
        return Err(ProviderConfigError::InvalidUrl);
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(ProviderConfigError::InvalidUrl);
    }
    Ok(url)
}

pub async fn build_discovered_oidc_client(
    req_client: &ReqwestClient,
    issuer_url: &str,
    client_id: String,
    client_secret: String,
    redirect_url: String,
) -> Result<OidcClient, anyhow::Error> {
    validate_provider_url(issuer_url, true)?;
    validate_provider_url(&redirect_url, true)?;
    let metadata =
        CoreProviderMetadata::discover_async(IssuerUrl::new(issuer_url.to_string())?, req_client)
            .await?;
    Ok(OidcClient::from_provider_metadata(
        metadata,
        ClientId::new(client_id),
        Some(ClientSecret::new(client_secret)),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_url)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_valid_provider_slugs() {
        assert_eq!(
            normalize_provider_slug(" Keycloak-EU ").unwrap(),
            "keycloak-eu"
        );
    }

    #[test]
    fn rejects_unsafe_provider_slugs() {
        for value in ["", "-okta", "okta-", "okta_eu", "okta/eu", "42-okta"] {
            assert!(normalize_provider_slug(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn requires_openid_and_rejects_reserved_parameters() {
        let missing_openid = OidcProviderConfiguration {
            scopes: vec!["email".to_string()],
            ..Default::default()
        };
        assert_eq!(
            missing_openid.validate(),
            Err(ProviderConfigError::MissingOpenIdScope)
        );

        let mut reserved = OidcProviderConfiguration::default();
        reserved.authorization_params.insert(
            "redirect_uri".to_string(),
            "https://attacker.example".to_string(),
        );
        assert!(matches!(
            reserved.validate(),
            Err(ProviderConfigError::InvalidAuthorizationParameter(_))
        ));
    }

    #[test]
    fn provider_urls_require_https_except_loopback() {
        assert!(validate_provider_url("https://id.example.com/realms/acme", true).is_ok());
        assert!(validate_provider_url("http://localhost:5556/dex", true).is_ok());
        assert!(validate_provider_url("http://id.example.com", true).is_err());
        assert!(validate_provider_url("https://user:pass@id.example.com", true).is_err());
        assert!(validate_provider_url("https://id.example.com/#fragment", true).is_err());
    }
}
