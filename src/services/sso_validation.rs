// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        claims::Claiming,
        encryption::decrypt_key,
        error::AuthError,
        github::{GitHubAdapterError, GitHubOAuthAdapter},
        provider_config::{
            OidcProviderConfiguration, build_discovered_oidc_client, normalize_provider_slug,
            validate_issuer_url_for_provider, validate_redirect_url_for_provider,
        },
        sso_proxy::sso_proxy_jwks_url,
    },
    models::sso_providers,
    state::SharedState,
};
use chrono::{DateTime, Duration, Utc};
use jsonwebtoken::jwk::JwkSet;
use openssl::sha::sha256;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

const EMPTY_VALUE: &str = "<empty>";
const SSO_VALIDATION_TOKEN_TTL_MINUTES: i64 = 10;

#[derive(Clone)]
pub struct SsoDraftConfig {
    pub provider: String,
    pub tenant_id: Option<String>,
    pub client_id: String,
    pub client_secret: String,
    pub issuer_url: String,
    pub redirect_url: String,
    pub configuration: OidcProviderConfiguration,
}

#[derive(Debug, Serialize, Deserialize)]
struct SsoValidationTokenClaims {
    provider_id: Uuid,
    user_id: Uuid,
    config_hash: String,
    exp: usize,
}

impl Claiming for SsoValidationTokenClaims {}

impl SsoValidationTokenClaims {
    fn new(provider_id: Uuid, user_id: Uuid, config_hash: String) -> (Self, DateTime<Utc>) {
        let expires_at = Utc::now() + Duration::minutes(SSO_VALIDATION_TOKEN_TTL_MINUTES);
        (
            Self {
                provider_id,
                user_id,
                config_hash,
                exp: expires_at.timestamp().max(0) as usize,
            },
            expires_at,
        )
    }
}

fn extract_frontend_origin(frontend_hosted_url: &str) -> Result<String, AuthError> {
    let parsed = Url::parse(frontend_hosted_url).map_err(|_| AuthError::InvalidRedirectUri {
        redirect_uri: Some(frontend_hosted_url.to_string()),
    })?;
    let host = parsed
        .host_str()
        .ok_or_else(|| AuthError::InvalidRedirectUri {
            redirect_uri: Some(frontend_hosted_url.to_string()),
        })?;
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    Ok(format!("{}://{host}{port}", parsed.scheme()))
}

fn normalize_secret_for_compare(value: &str) -> String {
    if value == EMPTY_VALUE {
        String::new()
    } else {
        value.to_string()
    }
}

fn resolve_redirect_url(
    provider: &str,
    requested_redirect_url: Option<&String>,
    frontend_hosted_url: Option<&String>,
    existing_redirect_url: &str,
) -> Result<String, AuthError> {
    let is_apple = provider.eq_ignore_ascii_case("apple");
    let derived_from_frontend = if let Some(frontend_hosted_url) = frontend_hosted_url {
        let origin = extract_frontend_origin(frontend_hosted_url)?;
        (!is_apple).then(|| format!("{origin}/auth/{provider}/callback"))
    } else {
        None
    };

    let redirect_url = if let Some(redirect_url) = requested_redirect_url {
        if let Some(derived) = derived_from_frontend.as_ref() {
            if redirect_url != derived {
                return Err(AuthError::InvalidRedirectUri {
                    redirect_uri: Some(redirect_url.clone()),
                });
            }
        }
        redirect_url.clone()
    } else if let Some(derived) = derived_from_frontend {
        derived
    } else {
        existing_redirect_url.to_string()
    };
    validate_redirect_url_for_provider(provider, &redirect_url).map_err(|_| {
        AuthError::InvalidRedirectUri {
            redirect_uri: Some(redirect_url.clone()),
        }
    })?;
    Ok(redirect_url)
}

pub fn build_draft_config(
    app_state: &SharedState,
    model: &sso_providers::Model,
    provider: Option<&String>,
    tenant_id: Option<&String>,
    client_id: Option<&String>,
    client_secret: Option<&String>,
    issuer_url: Option<&String>,
    redirect_url: Option<&String>,
    frontend_hosted_url: Option<&String>,
    configuration: Option<&OidcProviderConfiguration>,
) -> Result<SsoDraftConfig, AuthError> {
    let provider_input = provider
        .map(|p| p.trim().to_lowercase())
        .unwrap_or_else(|| model.provider.trim().to_lowercase());
    let provider =
        normalize_provider_slug(&provider_input).map_err(|_| AuthError::InvalidProvider {
            provider: Some(provider_input),
        })?;
    let tenant_id = tenant_id.cloned().or_else(|| model.tenant_id.clone());
    let decrypted_existing_secret =
        decrypt_key(&app_state.settings.auth.app_key, &model.client_secret)
            .unwrap_or_else(|_| String::new());
    let redirect_url = resolve_redirect_url(
        &provider,
        redirect_url,
        frontend_hosted_url,
        &model.redirect_url,
    )?;
    let issuer_url = issuer_url
        .cloned()
        .unwrap_or_else(|| model.issuer_url.clone());
    validate_issuer_url_for_provider(&provider, &issuer_url).map_err(|_| {
        AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        }
    })?;
    let configuration = configuration.cloned().unwrap_or(
        OidcProviderConfiguration::from_value_for_provider(model.configuration.as_ref(), &provider)
            .map_err(|_| AuthError::InvalidProvider {
                provider: Some(provider.clone()),
            })?,
    );
    configuration
        .validate_for_provider(&provider)
        .map_err(|_| AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        })?;

    Ok(SsoDraftConfig {
        provider,
        tenant_id,
        client_id: client_id
            .cloned()
            .unwrap_or_else(|| model.client_id.clone()),
        client_secret: client_secret
            .cloned()
            .unwrap_or_else(|| decrypted_existing_secret.clone()),
        issuer_url,
        redirect_url,
        configuration,
    })
}

pub fn has_sensitive_changes(
    app_state: &SharedState,
    model: &sso_providers::Model,
    draft: &SsoDraftConfig,
) -> bool {
    let existing_secret = decrypt_key(&app_state.settings.auth.app_key, &model.client_secret)
        .unwrap_or_else(|_| String::new());
    let existing_configuration = OidcProviderConfiguration::from_value_for_provider(
        model.configuration.as_ref(),
        &model.provider,
    )
    .unwrap_or_default();
    draft.provider != model.provider.to_lowercase()
        || draft.tenant_id != model.tenant_id
        || draft.client_id != model.client_id
        || normalize_secret_for_compare(&draft.client_secret)
            != normalize_secret_for_compare(&existing_secret)
        || draft.issuer_url != model.issuer_url
        || draft.redirect_url != model.redirect_url
        || draft.configuration != existing_configuration
}

pub fn config_hash(draft: &SsoDraftConfig) -> String {
    let configuration = serde_json::to_string(&draft.configuration).unwrap_or_default();
    let material = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}",
        draft.provider.trim().to_lowercase(),
        draft
            .tenant_id
            .clone()
            .unwrap_or_default()
            .trim()
            .to_lowercase(),
        draft.client_id.trim(),
        normalize_secret_for_compare(draft.client_secret.trim()),
        draft.issuer_url.trim().to_lowercase(),
        draft.redirect_url.trim(),
        configuration,
    );
    sha256(material.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

pub fn issue_validation_token(
    provider_id: Uuid,
    user_id: Uuid,
    draft: &SsoDraftConfig,
) -> (String, DateTime<Utc>) {
    issue_validation_token_for_hash(provider_id, user_id, config_hash(draft))
}

pub fn issue_validation_token_for_hash(
    provider_id: Uuid,
    user_id: Uuid,
    config_hash: String,
) -> (String, DateTime<Utc>) {
    let (claims, expires_at) = SsoValidationTokenClaims::new(provider_id, user_id, config_hash);
    (claims.get_token_string(), expires_at)
}

pub fn validate_validation_token(
    token: &str,
    provider_id: Uuid,
    user_id: Uuid,
    expected_hash: &str,
) -> Result<(), AuthError> {
    let claims =
        SsoValidationTokenClaims::from_token_string(token).map_err(|_| AuthError::InvalidToken)?;
    if claims.provider_id != provider_id
        || claims.user_id != user_id
        || claims.config_hash != expected_hash
    {
        return Err(AuthError::InvalidToken);
    }
    Ok(())
}

pub fn normalize_allowed_domains(domains: &[String]) -> Vec<String> {
    let mut normalized = domains
        .iter()
        .map(|domain| domain.trim().trim_start_matches('@').to_ascii_lowercase())
        .filter(|domain| !domain.is_empty())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
}

pub fn grengin_proxy_config_hash(
    provider: &str,
    tenant_id: Option<&str>,
    redirect_url: &str,
    allowed_domains: &[String],
) -> String {
    let domains = normalize_allowed_domains(allowed_domains);
    let domains = serde_json::to_string(&domains).unwrap_or_default();
    let material = format!(
        "grengin-proxy-v1\n{}\n{}\n{}\n{}",
        provider.trim().to_ascii_lowercase(),
        tenant_id.unwrap_or_default().trim().to_ascii_lowercase(),
        redirect_url.trim(),
        domains,
    );
    sha256(material.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}

pub async fn validate_grengin_proxy(app_state: &SharedState) -> Result<(bool, String), AuthError> {
    let response = app_state
        .req_client
        .get(sso_proxy_jwks_url())
        .send()
        .await
        .map_err(|error| {
            eprintln!("Grengin SSO proxy validation request failed: {error}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    if !response.status().is_success() {
        return Ok((
            false,
            "Grengin SSO proxy verification endpoint is unavailable".to_string(),
        ));
    }
    let jwks = response.json::<JwkSet>().await.map_err(|error| {
        eprintln!("Grengin SSO proxy JWKS validation failed: {error}");
        AuthError::ServiceTemporarilyUnavailable
    })?;
    if jwks.keys.is_empty() {
        return Ok((
            false,
            "Grengin SSO proxy published no verification keys".to_string(),
        ));
    }
    Ok((true, "Grengin SSO proxy connection validated".to_string()))
}

fn parse_oauth_error(body: &str) -> (String, String) {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = value
        .get("error")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_lowercase();
    let error_description = value
        .get("error_description")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_lowercase();
    (error, error_description)
}

async fn probe_google_config(
    app_state: &SharedState,
    draft: &SsoDraftConfig,
) -> Result<(bool, String), AuthError> {
    let response = app_state
        .req_client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", draft.client_id.as_str()),
            ("client_secret", draft.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", "grengin-sso-validation-probe"),
            ("redirect_uri", draft.redirect_url.as_str()),
        ])
        .send()
        .await
        .map_err(|e| {
            eprintln!("google validation request failed: {e}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|e| {
        eprintln!("google validation response read failed: {e}");
        AuthError::ServiceTemporarilyUnavailable
    })?;
    let (error, description) = parse_oauth_error(&body);
    if error == "invalid_grant" {
        return Ok((
            true,
            "Google SSO credentials and redirect URI validated".to_string(),
        ));
    }
    if error.contains("redirect_uri") || description.contains("redirect_uri") {
        return Ok((false, "Google redirect URI is invalid".to_string()));
    }
    if error == "invalid_client"
        || description.contains("invalid client")
        || description.contains("unauthorized")
    {
        return Ok((false, "Google client credentials are invalid".to_string()));
    }
    Ok((
        false,
        format!("Google validation failed with status {}", status.as_u16()),
    ))
}

async fn probe_azure_config(
    app_state: &SharedState,
    draft: &SsoDraftConfig,
) -> Result<(bool, String), AuthError> {
    let tenant_id = draft
        .tenant_id
        .clone()
        .unwrap_or_else(|| "common".to_string());
    let token_url = format!("https://login.microsoftonline.com/{tenant_id}/oauth2/v2.0/token");
    let response = app_state
        .req_client
        .post(token_url)
        .form(&[
            ("client_id", draft.client_id.as_str()),
            ("client_secret", draft.client_secret.as_str()),
            ("grant_type", "authorization_code"),
            ("code", "grengin-sso-validation-probe"),
            ("redirect_uri", draft.redirect_url.as_str()),
        ])
        .send()
        .await
        .map_err(|e| {
            eprintln!("azure validation request failed: {e}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|e| {
        eprintln!("azure validation response read failed: {e}");
        AuthError::ServiceTemporarilyUnavailable
    })?;
    let (error, description) = parse_oauth_error(&body);
    if error == "invalid_grant" {
        return Ok((
            true,
            "Azure SSO credentials and redirect URI validated".to_string(),
        ));
    }
    if description.contains("aadsts50011") || description.contains("reply url") {
        return Ok((false, "Azure redirect URI is invalid".to_string()));
    }
    if error == "invalid_client"
        || description.contains("aadsts7000215")
        || description.contains("aadsts700016")
        || description.contains("invalid client")
    {
        return Ok((false, "Azure client credentials are invalid".to_string()));
    }
    Ok((
        false,
        format!("Azure validation failed with status {}", status.as_u16()),
    ))
}

pub async fn validate_sso_draft(
    app_state: &SharedState,
    draft: &SsoDraftConfig,
) -> Result<(bool, String), AuthError> {
    if draft.client_id.trim().is_empty() || draft.client_id == EMPTY_VALUE {
        return Ok((false, "Client ID is required".to_string()));
    }
    if draft.client_secret.trim().is_empty() || draft.client_secret == EMPTY_VALUE {
        return Ok((false, "Client secret is required".to_string()));
    }
    draft
        .configuration
        .validate_for_provider(&draft.provider)
        .map_err(|_| AuthError::InvalidProvider {
            provider: Some(draft.provider.clone()),
        })?;
    match draft.provider.as_str() {
        "google" => probe_google_config(app_state, draft).await,
        "azure" => probe_azure_config(app_state, draft).await,
        "github" => {
            if !GitHubOAuthAdapter::supports_issuer(&draft.issuer_url) {
                return Ok((
                    false,
                    "GitHub issuer must be https://github.com".to_string(),
                ));
            }
            let adapter = GitHubOAuthAdapter::new(
                draft.client_id.clone(),
                draft.client_secret.clone(),
                draft.redirect_url.clone(),
            )
            .map_err(|_| AuthError::InvalidProvider {
                provider: Some(draft.provider.clone()),
            })?;
            match adapter.validate_remote(&app_state.req_client).await {
                Ok(()) => Ok((
                    true,
                    "GitHub OAuth credentials and callback configuration validated".to_string(),
                )),
                Err(GitHubAdapterError::InvalidCredentials) => {
                    Ok((false, "GitHub client credentials are invalid".to_string()))
                }
                Err(GitHubAdapterError::InvalidConfiguration) => {
                    Ok((false, "GitHub OAuth configuration is invalid".to_string()))
                }
                Err(error) => {
                    eprintln!("GitHub validation request failed: {error}");
                    Err(AuthError::ServiceTemporarilyUnavailable)
                }
            }
        }
        _ => {
            build_discovered_oidc_client(
                &app_state.req_client,
                &draft.issuer_url,
                draft.client_id.clone(),
                draft.client_secret.clone(),
                draft.redirect_url.clone(),
            )
            .await
            .map_err(|error| {
                eprintln!("OIDC discovery validation failed: {error:?}");
                AuthError::InvalidProvider {
                    provider: Some(draft.provider.clone()),
                }
            })?;
            Ok((
                true,
                "OIDC discovery and provider configuration validated".to_string(),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::jwt::{KEYS, Keys};

    fn draft() -> SsoDraftConfig {
        SsoDraftConfig {
            provider: "keycloak".to_string(),
            tenant_id: None,
            client_id: "client-id".to_string(),
            client_secret: "client-secret".to_string(),
            issuer_url: "https://id.example.com/realms/acme".to_string(),
            redirect_url: "https://app.example.com/auth/keycloak/callback".to_string(),
            configuration: OidcProviderConfiguration::default(),
        }
    }

    #[test]
    fn validation_hash_covers_provider_configuration() {
        let first = draft();
        let mut second = first.clone();
        second.configuration.scopes.push("groups".to_string());

        assert_ne!(config_hash(&first), config_hash(&second));
    }

    #[test]
    fn proxy_hash_is_stable_for_equivalent_domain_lists() {
        let first = grengin_proxy_config_hash(
            "Azure",
            Some("COMMON"),
            "https://app.example.com/auth/azure/callback",
            &[" Example.com ".to_string(), "@example.com".to_string()],
        );
        let second = grengin_proxy_config_hash(
            "azure",
            Some("common"),
            "https://app.example.com/auth/azure/callback",
            &["example.com".to_string()],
        );

        assert_eq!(first, second);
    }

    #[test]
    fn proxy_hash_changes_with_security_sensitive_fields() {
        let domains = ["example.com".to_string()];
        let original = grengin_proxy_config_hash(
            "azure",
            Some("common"),
            "https://app.example.com/auth/azure/callback",
            &domains,
        );

        assert_ne!(
            original,
            grengin_proxy_config_hash(
                "azure",
                Some("tenant-id"),
                "https://app.example.com/auth/azure/callback",
                &domains,
            )
        );
        assert_ne!(
            original,
            grengin_proxy_config_hash(
                "azure",
                Some("common"),
                "https://other.example.com/auth/azure/callback",
                &domains,
            )
        );
        assert_ne!(
            original,
            grengin_proxy_config_hash(
                "azure",
                Some("common"),
                "https://app.example.com/auth/azure/callback",
                &["other.example".to_string()],
            )
        );
    }

    #[test]
    fn validation_token_is_bound_to_provider_user_and_configuration() {
        let _ = KEYS.set(Keys::new(b"sso-validation-test-secret"));
        let provider_id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let config_hash = "expected-config-hash".to_string();
        let (token, _) = issue_validation_token_for_hash(provider_id, user_id, config_hash.clone());

        assert!(validate_validation_token(&token, provider_id, user_id, &config_hash).is_ok());
        assert!(matches!(
            validate_validation_token(&token, Uuid::new_v4(), user_id, &config_hash),
            Err(AuthError::InvalidToken)
        ));
        assert!(matches!(
            validate_validation_token(&token, provider_id, Uuid::new_v4(), &config_hash),
            Err(AuthError::InvalidToken)
        ));
        assert!(matches!(
            validate_validation_token(&token, provider_id, user_id, "changed-config-hash"),
            Err(AuthError::InvalidToken)
        ));
    }

    #[test]
    fn frontend_origin_cannot_override_derived_callback() {
        let error = resolve_redirect_url(
            "keycloak",
            Some(&"https://attacker.example/callback".to_string()),
            Some(&"https://app.example.com/login".to_string()),
            "https://app.example.com/auth/keycloak/callback",
        );
        assert!(matches!(error, Err(AuthError::InvalidRedirectUri { .. })));
    }

    #[test]
    fn apple_keeps_api_callback_when_frontend_origin_is_supplied() {
        let redirect = resolve_redirect_url(
            "apple",
            Some(&"https://api.example.com/auth/apple/callback".to_string()),
            Some(&"https://chat.example.com/login".to_string()),
            "https://api.example.com/auth/apple/callback",
        )
        .expect("Apple API callback");

        assert_eq!(redirect, "https://api.example.com/auth/apple/callback");
    }
}
