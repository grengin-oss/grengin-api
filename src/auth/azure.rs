// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::config::setting::OidcClient;
use anyhow::Error;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use openidconnect::{
    AuthUrl, ClientId, ClientSecret, EmptyAdditionalProviderMetadata, IssuerUrl, JsonWebKeySetUrl,
    RedirectUrl, ResponseTypes, TokenUrl, UserInfoUrl,
    core::{
        CoreClient, CoreIdToken, CoreJsonWebKeySet, CoreJwsSigningAlgorithm, CoreProviderMetadata,
        CoreResponseType, CoreSubjectIdentifierType,
    },
};
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Mutex;
use uuid::Uuid;

const AZURE_METADATA_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const AZURE_METADATA_FAILURE_BACKOFF: Duration = Duration::from_secs(30);
const AZURE_METADATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const AZURE_CONSUMER_TENANT_ID: &str = "9188040d-6c67-4c5b-b112-36a304b66dad";
const AZURE_MULTITENANT_ISSUER: &str = "https://login.microsoftonline.com/{tenantid}/v2.0";

#[derive(Clone, Debug)]
pub struct AzureMultitenantValidation {
    authority: String,
    key_issuers: Arc<HashMap<String, String>>,
}

impl AzureMultitenantValidation {
    pub(crate) fn new(authority: impl Into<String>, key_issuers: HashMap<String, String>) -> Self {
        Self {
            authority: normalized_authority(&authority.into()),
            key_issuers: Arc::new(key_issuers),
        }
    }

    pub fn authority(&self) -> &str {
        &self.authority
    }
}

pub struct AzureOidcClient {
    pub client: OidcClient,
    pub multitenant_validation: Option<AzureMultitenantValidation>,
}

#[derive(Clone)]
struct AzureMetadataSnapshot {
    provider_metadata: CoreProviderMetadata,
    validation: AzureMultitenantValidation,
}

#[derive(Clone)]
enum AzureMetadataCacheEntry {
    Ready {
        cached_at: Instant,
        snapshot: Box<AzureMetadataSnapshot>,
    },
    Failed {
        cached_at: Instant,
        message: String,
    },
}

impl AzureMetadataCacheEntry {
    fn cached_result(&self, now: Instant) -> Option<Result<AzureMetadataSnapshot, String>> {
        match self {
            Self::Ready {
                cached_at,
                snapshot,
            } if now.duration_since(*cached_at) < AZURE_METADATA_TTL => {
                Some(Ok((**snapshot).clone()))
            }
            Self::Failed { cached_at, message }
                if now.duration_since(*cached_at) < AZURE_METADATA_FAILURE_BACKOFF =>
            {
                Some(Err(message.clone()))
            }
            _ => None,
        }
    }
}

static AZURE_METADATA_CACHE: OnceLock<Mutex<HashMap<String, AzureMetadataCacheEntry>>> =
    OnceLock::new();

#[derive(Deserialize)]
struct AzureJwksDocument {
    keys: Vec<AzureJwkMetadata>,
}

#[derive(Deserialize)]
struct AzureJwkMetadata {
    kid: Option<String>,
    issuer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AzureTokenHeader {
    kid: String,
}

#[derive(Debug, Deserialize)]
struct AzureTokenTenantClaims {
    iss: String,
    tid: String,
}

pub fn is_azure_multitenant_authority(tenant_id: &str) -> bool {
    matches!(
        tenant_id.trim().to_ascii_lowercase().as_str(),
        "common" | "organizations" | "consumers"
    )
}

fn normalized_authority(tenant_id: &str) -> String {
    tenant_id.trim().to_ascii_lowercase()
}

fn authority_issuer(authority: &str) -> &'static str {
    match authority {
        "consumers" => {
            "https://login.microsoftonline.com/9188040d-6c67-4c5b-b112-36a304b66dad/v2.0"
        }
        _ => AZURE_MULTITENANT_ISSUER,
    }
}

fn mk_urls<S: Into<String>>(
    tenant_id: S,
) -> anyhow::Result<(
    IssuerUrl,
    AuthUrl,
    TokenUrl,
    JsonWebKeySetUrl,
    Option<UserInfoUrl>,
)> {
    let tenant_id = tenant_id.into();
    let authority = normalized_authority(&tenant_id);
    let issuer = IssuerUrl::new(if is_azure_multitenant_authority(&authority) {
        authority_issuer(&authority).to_string()
    } else {
        format!("https://login.microsoftonline.com/{authority}/v2.0")
    })?;
    let auth = AuthUrl::new(format!(
        "https://login.microsoftonline.com/{authority}/oauth2/v2.0/authorize"
    ))?;
    let token = TokenUrl::new(format!(
        "https://login.microsoftonline.com/{authority}/oauth2/v2.0/token"
    ))?;
    let jwks = JsonWebKeySetUrl::new(format!(
        "https://login.microsoftonline.com/{authority}/discovery/v2.0/keys"
    ))?;
    let userinfo = UserInfoUrl::new("https://graph.microsoft.com/oidc/userinfo".into()).ok();
    Ok((issuer, auth, token, jwks, userinfo))
}

fn multitenant_provider_metadata(
    issuer: IssuerUrl,
    auth: AuthUrl,
    token: TokenUrl,
    jwks_url: JsonWebKeySetUrl,
    userinfo: Option<UserInfoUrl>,
    jwks: CoreJsonWebKeySet,
) -> CoreProviderMetadata {
    CoreProviderMetadata::new(
        issuer,
        auth,
        jwks_url,
        vec![ResponseTypes::new(vec![CoreResponseType::Code])],
        vec![CoreSubjectIdentifierType::Public],
        vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256],
        EmptyAdditionalProviderMetadata {},
    )
    .set_token_endpoint(Some(token))
    .set_userinfo_endpoint(userinfo)
    .set_jwks(jwks)
}

async fn fetch_multitenant_provider_metadata(
    req_client: &ReqwestClient,
    authority: &str,
) -> Result<AzureMetadataSnapshot, Error> {
    let (issuer, auth, token, jwks_url, userinfo) = mk_urls(authority)?;
    let response = req_client
        .get(jwks_url.as_str())
        .timeout(AZURE_METADATA_REQUEST_TIMEOUT)
        .send()
        .await?
        .error_for_status()?;
    let document = response.json::<serde_json::Value>().await?;
    let jwks: CoreJsonWebKeySet = serde_json::from_value(document.clone())?;
    if jwks.keys().is_empty() {
        anyhow::bail!("Azure returned an empty JSON Web Key Set");
    }
    let metadata: AzureJwksDocument = serde_json::from_value(document)?;
    let key_issuers = metadata
        .keys
        .into_iter()
        .filter_map(|key| Some((key.kid?, key.issuer?)))
        .collect::<HashMap<_, _>>();
    if key_issuers.is_empty() {
        anyhow::bail!("Azure returned no signing-key issuer metadata");
    }

    Ok(AzureMetadataSnapshot {
        provider_metadata: multitenant_provider_metadata(
            issuer, auth, token, jwks_url, userinfo, jwks,
        ),
        validation: AzureMultitenantValidation::new(authority, key_issuers),
    })
}

async fn cached_multitenant_provider_metadata(
    req_client: &ReqwestClient,
    tenant_id: &str,
) -> Result<AzureMetadataSnapshot, Error> {
    let authority = normalized_authority(tenant_id);
    let cache = AZURE_METADATA_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let cached = {
        let cache = cache.lock().await;
        cache
            .get(&authority)
            .and_then(|entry| entry.cached_result(Instant::now()))
    };
    if let Some(result) = cached {
        match result {
            Ok(snapshot) => return Ok(snapshot),
            Err(message) => anyhow::bail!(message),
        }
    }

    let fetched = fetch_multitenant_provider_metadata(req_client, &authority).await;
    let mut cache = cache.lock().await;
    match fetched {
        Ok(snapshot) => {
            cache.insert(
                authority,
                AzureMetadataCacheEntry::Ready {
                    cached_at: Instant::now(),
                    snapshot: Box::new(snapshot.clone()),
                },
            );
            Ok(snapshot)
        }
        Err(error) => {
            let message = error.to_string();
            cache.insert(
                authority,
                AzureMetadataCacheEntry::Failed {
                    cached_at: Instant::now(),
                    message: message.clone(),
                },
            );
            Err(anyhow::anyhow!(message))
        }
    }
}

pub async fn invalidate_azure_multitenant_cache(tenant_id: &str) {
    if let Some(cache) = AZURE_METADATA_CACHE.get() {
        cache.lock().await.remove(&normalized_authority(tenant_id));
    }
}

fn decode_jwt_segment<T>(token: &str, index: usize) -> Result<T, Error>
where
    T: for<'de> Deserialize<'de>,
{
    let encoded = token
        .split('.')
        .nth(index)
        .ok_or_else(|| anyhow::anyhow!("Azure ID token is malformed"))?;
    let decoded = URL_SAFE_NO_PAD.decode(encoded)?;
    Ok(serde_json::from_slice(&decoded)?)
}

fn key_issuer_for_tenant(key_issuer: &str, tenant_id: &str) -> String {
    let lowercase = key_issuer.to_ascii_lowercase();
    if let Some(start) = lowercase.find("{tenantid}") {
        let end = start + "{tenantid}".len();
        format!(
            "{}{}{}",
            &key_issuer[..start],
            tenant_id,
            &key_issuer[end..]
        )
    } else {
        key_issuer.to_string()
    }
}

pub fn validate_azure_multitenant_id_token(
    id_token: &CoreIdToken,
    validation: &AzureMultitenantValidation,
) -> Result<Uuid, Error> {
    let raw_token = id_token.to_string();
    let header: AzureTokenHeader = decode_jwt_segment(&raw_token, 0)?;
    let claims: AzureTokenTenantClaims = decode_jwt_segment(&raw_token, 1)?;
    let tenant_id = Uuid::parse_str(&claims.tid)
        .map_err(|_| anyhow::anyhow!("Azure ID token tid claim is not a GUID"))?;
    let canonical_tenant_id = tenant_id.to_string();
    if claims.tid != canonical_tenant_id {
        anyhow::bail!("Azure ID token tid claim is not canonical");
    }
    let expected_issuer = format!("https://login.microsoftonline.com/{canonical_tenant_id}/v2.0");
    if claims.iss != expected_issuer {
        anyhow::bail!("Azure ID token issuer does not match its tid claim");
    }
    match validation.authority.as_str() {
        "organizations" if canonical_tenant_id == AZURE_CONSUMER_TENANT_ID => {
            anyhow::bail!("Azure consumer accounts are not allowed by organizations authority");
        }
        "consumers" if canonical_tenant_id != AZURE_CONSUMER_TENANT_ID => {
            anyhow::bail!("Azure organizational accounts are not allowed by consumers authority");
        }
        _ => {}
    }
    let key_issuer = validation
        .key_issuers
        .get(&header.kid)
        .ok_or_else(|| anyhow::anyhow!("Azure signing key has no trusted issuer metadata"))?;
    if key_issuer_for_tenant(key_issuer, &canonical_tenant_id) != claims.iss {
        anyhow::bail!("Azure signing key issuer does not match the token issuer");
    }
    Ok(tenant_id)
}

pub async fn build_azure_client<S: Into<String>>(
    req_client: &ReqwestClient,
    client_id: S,
    client_secret: S,
    redirect_url: S,
    tenant_id: S,
) -> Result<AzureOidcClient, Error> {
    let client_id = ClientId::new(client_id.into());
    let client_secret = ClientSecret::new(client_secret.into());
    let tenant_id = tenant_id.into();
    let redirect_uri = RedirectUrl::new(redirect_url.into())?;
    let (client, multitenant_validation) = if is_azure_multitenant_authority(&tenant_id) {
        let snapshot = cached_multitenant_provider_metadata(req_client, &tenant_id).await?;
        (
            CoreClient::from_provider_metadata(
                snapshot.provider_metadata,
                client_id,
                Some(client_secret),
            ),
            Some(snapshot.validation),
        )
    } else {
        let issuer = IssuerUrl::new(format!(
            "https://login.microsoftonline.com/{}/v2.0",
            tenant_id
        ))?;
        let provider = CoreProviderMetadata::discover_async(issuer, req_client).await?;
        (
            CoreClient::from_provider_metadata(provider, client_id, Some(client_secret)),
            None,
        )
    };
    Ok(AzureOidcClient {
        client: client.set_redirect_uri(redirect_uri),
        multitenant_validation,
    })
}

pub async fn build_azure_public_client<C, R, T>(
    req_client: &ReqwestClient,
    client_id: C,
    redirect_url: R,
    tenant_id: T,
) -> Result<AzureOidcClient, Error>
where
    C: Into<String>,
    R: Into<String>,
    T: Into<String>,
{
    let client_id = ClientId::new(client_id.into());
    let tenant_id = tenant_id.into();
    let redirect_uri = RedirectUrl::new(redirect_url.into())?;
    let (client, multitenant_validation) = if is_azure_multitenant_authority(&tenant_id) {
        let snapshot = cached_multitenant_provider_metadata(req_client, &tenant_id).await?;
        (
            CoreClient::from_provider_metadata(snapshot.provider_metadata, client_id, None),
            Some(snapshot.validation),
        )
    } else {
        let issuer = IssuerUrl::new(format!(
            "https://login.microsoftonline.com/{}/v2.0",
            tenant_id
        ))?;
        let provider = CoreProviderMetadata::discover_async(issuer, req_client).await?;
        (
            CoreClient::from_provider_metadata(provider, client_id, None),
            None,
        )
    };
    Ok(AzureOidcClient {
        client: client.set_redirect_uri(redirect_uri),
        multitenant_validation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn test_multitenant_metadata() -> AzureMetadataSnapshot {
        let document = serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "kid": "test-key",
                "n": "AQAB",
                "e": "AQAB",
                "issuer": AZURE_MULTITENANT_ISSUER
            }]
        });
        let jwks: CoreJsonWebKeySet = serde_json::from_value(document.clone()).expect("test JWKS");
        let metadata: AzureJwksDocument = serde_json::from_value(document).expect("key metadata");
        let key_issuers = metadata
            .keys
            .into_iter()
            .filter_map(|key| Some((key.kid?, key.issuer?)))
            .collect();
        let (issuer, auth, token, jwks_url, userinfo) = mk_urls("common").expect("common URLs");
        AzureMetadataSnapshot {
            provider_metadata: multitenant_provider_metadata(
                issuer, auth, token, jwks_url, userinfo, jwks,
            ),
            validation: AzureMultitenantValidation::new("common", key_issuers),
        }
    }

    fn unsigned_test_token(kid: &str, tenant_id: &str, issuer: &str) -> CoreIdToken {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({"alg": "RS256", "kid": kid})).expect("header"),
        );
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "iss": issuer,
                "sub": "subject",
                "aud": "client-id",
                "exp": 4_102_444_800_i64,
                "iat": 1_700_000_000_i64,
                "tid": tenant_id
            }))
            .expect("payload"),
        );
        CoreIdToken::from_str(&format!("{header}.{payload}.c2lnbmF0dXJl")).expect("ID token")
    }

    #[test]
    fn multitenant_metadata_contains_verification_keys_and_template_issuer() {
        let snapshot = test_multitenant_metadata();
        assert_eq!(snapshot.provider_metadata.jwks().keys().len(), 1);
        assert_eq!(
            snapshot.provider_metadata.issuer().as_str(),
            AZURE_MULTITENANT_ISSUER
        );
    }

    #[test]
    fn confidential_client_keeps_the_exact_configured_callback() {
        let callback = "https://app.example.com/auth/azure/callback";
        let snapshot = test_multitenant_metadata();
        let client = CoreClient::from_provider_metadata(
            snapshot.provider_metadata,
            ClientId::new("client-id".to_string()),
            Some(ClientSecret::new("client-secret".to_string())),
        )
        .set_redirect_uri(RedirectUrl::new(callback.to_string()).expect("callback URL"));

        assert_eq!(
            client.redirect_uri().map(|url| url.as_str()),
            Some(callback)
        );
    }

    #[test]
    fn multitenant_token_requires_issuer_to_match_tid() {
        let tenant_id = "ff507be6-32aa-4573-99a0-185d88089a7e";
        let validation = test_multitenant_metadata().validation;
        let valid = unsigned_test_token(
            "test-key",
            tenant_id,
            &format!("https://login.microsoftonline.com/{tenant_id}/v2.0"),
        );
        let wrong_issuer = unsigned_test_token(
            "test-key",
            tenant_id,
            "https://login.microsoftonline.com/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee/v2.0",
        );

        assert_eq!(
            validate_azure_multitenant_id_token(&valid, &validation)
                .expect("valid issuer")
                .to_string(),
            tenant_id
        );
        assert!(validate_azure_multitenant_id_token(&wrong_issuer, &validation).is_err());
    }

    #[test]
    fn multitenant_token_rejects_invalid_tid_and_key_issuer() {
        let tenant_id = "ff507be6-32aa-4573-99a0-185d88089a7e";
        let issuer = format!("https://login.microsoftonline.com/{tenant_id}/v2.0");
        let validation = test_multitenant_metadata().validation;

        assert!(
            validate_azure_multitenant_id_token(
                &unsigned_test_token("test-key", "not-a-guid", &issuer),
                &validation,
            )
            .is_err()
        );
        assert!(
            validate_azure_multitenant_id_token(
                &unsigned_test_token("unknown-key", tenant_id, &issuer),
                &validation,
            )
            .is_err()
        );
    }

    #[test]
    fn multitenant_authorities_enforce_their_account_type() {
        let organization_tenant_id = "ff507be6-32aa-4573-99a0-185d88089a7e";
        let organization_issuer =
            format!("https://login.microsoftonline.com/{organization_tenant_id}/v2.0");
        let consumer_issuer =
            format!("https://login.microsoftonline.com/{AZURE_CONSUMER_TENANT_ID}/v2.0");
        let key_issuers =
            HashMap::from([("test-key".to_string(), AZURE_MULTITENANT_ISSUER.to_string())]);
        let organizations = AzureMultitenantValidation::new("organizations", key_issuers.clone());
        let consumers = AzureMultitenantValidation::new("consumers", key_issuers);

        assert!(
            validate_azure_multitenant_id_token(
                &unsigned_test_token("test-key", AZURE_CONSUMER_TENANT_ID, &consumer_issuer,),
                &organizations,
            )
            .is_err()
        );
        assert!(
            validate_azure_multitenant_id_token(
                &unsigned_test_token("test-key", organization_tenant_id, &organization_issuer,),
                &consumers,
            )
            .is_err()
        );
    }

    #[test]
    fn metadata_cache_honors_success_ttl_and_failure_backoff() {
        let now = Instant::now();
        let ready = AzureMetadataCacheEntry::Ready {
            cached_at: now,
            snapshot: Box::new(test_multitenant_metadata()),
        };
        let failed = AzureMetadataCacheEntry::Failed {
            cached_at: now,
            message: "temporary failure".to_string(),
        };

        assert!(matches!(ready.cached_result(now), Some(Ok(_))));
        assert!(ready.cached_result(now + AZURE_METADATA_TTL).is_none());
        assert!(matches!(failed.cached_result(now), Some(Err(_))));
        assert!(
            failed
                .cached_result(now + AZURE_METADATA_FAILURE_BACKOFF)
                .is_none()
        );
    }
}
