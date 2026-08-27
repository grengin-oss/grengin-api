// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::config::setting::OidcClient;
use anyhow::Error;
use openidconnect::{
    AuthUrl, ClientId, ClientSecret, EmptyAdditionalProviderMetadata, IssuerUrl, JsonWebKeySetUrl,
    RedirectUrl, ResponseTypes, TokenUrl, UserInfoUrl,
    core::{
        CoreClient, CoreJsonWebKeySet, CoreJwsSigningAlgorithm, CoreProviderMetadata,
        CoreResponseType, CoreSubjectIdentifierType,
    },
};
use reqwest::Client as ReqwestClient;

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
    let issuer = IssuerUrl::new(format!(
        "https://login.microsoftonline.com/{}/v2.0",
        tenant_id
    ))?;
    let auth = AuthUrl::new(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize",
        tenant_id
    ))?;
    let token = TokenUrl::new(format!(
        "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
        tenant_id
    ))?;
    let jwks = JsonWebKeySetUrl::new(format!(
        "https://login.microsoftonline.com/{}/discovery/v2.0/keys",
        tenant_id
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
    tenant_id: &str,
) -> Result<CoreProviderMetadata, Error> {
    let (issuer, auth, token, jwks_url, userinfo) = mk_urls(tenant_id)?;
    let jwks = CoreJsonWebKeySet::fetch_async(&jwks_url, req_client).await?;
    if jwks.keys().is_empty() {
        anyhow::bail!("Azure returned an empty JSON Web Key Set");
    }
    Ok(multitenant_provider_metadata(
        issuer, auth, token, jwks_url, userinfo, jwks,
    ))
}

pub async fn build_azure_client<S: Into<String>>(
    req_client: &ReqwestClient,
    client_id: S,
    client_secret: S,
    redirect_url: S,
    tenant_id: S,
) -> Result<OidcClient, Error> {
    let client_id = ClientId::new(client_id.into());
    let client_secret = ClientSecret::new(client_secret.into());
    let tenant_id = tenant_id.into();
    let redirect_uri = RedirectUrl::new(redirect_url.into())?;
    let client = match tenant_id.as_str() {
        "common" | "organizations" | "consumers" => {
            let provider = fetch_multitenant_provider_metadata(req_client, &tenant_id).await?;

            CoreClient::from_provider_metadata(provider, client_id, Some(client_secret))
        }
        _ => {
            let issuer = IssuerUrl::new(format!(
                "https://login.microsoftonline.com/{}/v2.0",
                tenant_id
            ))?;
            let provider = CoreProviderMetadata::discover_async(issuer, req_client).await?;
            CoreClient::from_provider_metadata(provider, client_id, Some(client_secret))
        }
    }
    .set_redirect_uri(redirect_uri);
    Ok(client)
}

pub async fn build_azure_public_client<C, R, T>(
    req_client: &ReqwestClient,
    client_id: C,
    redirect_url: R,
    tenant_id: T,
) -> Result<OidcClient, Error>
where
    C: Into<String>,
    R: Into<String>,
    T: Into<String>,
{
    let client_id = ClientId::new(client_id.into());
    let tenant_id = tenant_id.into();
    let redirect_uri = RedirectUrl::new(redirect_url.into())?;
    let client = match tenant_id.as_str() {
        "common" | "organizations" | "consumers" => {
            let provider = fetch_multitenant_provider_metadata(req_client, &tenant_id).await?;

            CoreClient::from_provider_metadata(provider, client_id, None)
        }
        _ => {
            let issuer = IssuerUrl::new(format!(
                "https://login.microsoftonline.com/{}/v2.0",
                tenant_id
            ))?;
            let provider = CoreProviderMetadata::discover_async(issuer, req_client).await?;
            CoreClient::from_provider_metadata(provider, client_id, None)
        }
    }
    .set_redirect_uri(redirect_uri);
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_multitenant_metadata() -> CoreProviderMetadata {
        let jwks: CoreJsonWebKeySet = serde_json::from_value(serde_json::json!({
            "keys": [{
                "kty": "RSA",
                "use": "sig",
                "kid": "test-key",
                "n": "AQAB",
                "e": "AQAB"
            }]
        }))
        .expect("test JWKS");
        let (issuer, auth, token, jwks_url, userinfo) = mk_urls("common").expect("common URLs");
        multitenant_provider_metadata(issuer, auth, token, jwks_url, userinfo, jwks)
    }

    #[test]
    fn multitenant_metadata_contains_verification_keys() {
        assert_eq!(test_multitenant_metadata().jwks().keys().len(), 1);
    }

    #[test]
    fn confidential_client_keeps_the_exact_configured_callback() {
        let callback = "https://app.example.com/auth/azure/callback";
        let client = CoreClient::from_provider_metadata(
            test_multitenant_metadata(),
            ClientId::new("client-id".to_string()),
            Some(ClientSecret::new("client-secret".to_string())),
        )
        .set_redirect_uri(RedirectUrl::new(callback.to_string()).expect("callback URL"));

        assert_eq!(
            client.redirect_uri().map(|url| url.as_str()),
            Some(callback)
        );
    }
}
