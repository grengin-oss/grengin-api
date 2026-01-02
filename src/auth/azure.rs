use openidconnect::{AuthUrl, ClientId, ClientSecret, EmptyAdditionalProviderMetadata, IssuerUrl, JsonWebKeySetUrl, RedirectUrl, ResponseTypes, TokenUrl, UserInfoUrl, core::{CoreClient, CoreJwsSigningAlgorithm, CoreProviderMetadata, CoreResponseType, CoreSubjectIdentifierType}};
use anyhow::Error;
use reqwest::Client as ReqwestClient;
use crate::config::setting::OidcClient;

fn mk_urls<S: Into<String>>(tenant_id:S) -> anyhow::Result<(IssuerUrl, AuthUrl, TokenUrl, JsonWebKeySetUrl, Option<UserInfoUrl>)> {
    let tenant_id = tenant_id.into();
    let issuer = IssuerUrl::new(format!("https://login.microsoftonline.com/{}/v2.0",&tenant_id))?;
    let auth = AuthUrl::new(format!("https://login.microsoftonline.com/{}/oauth2/v2.0/authorize",&tenant_id))?;
    let token = TokenUrl::new(format!("https://login.microsoftonline.com/{}/oauth2/v2.0/token",&tenant_id))?;
    let jwks = JsonWebKeySetUrl::new(format!("https://login.microsoftonline.com/{}/discovery/v2.0/keys",&tenant_id))?;
    let userinfo = UserInfoUrl::new("https://graph.microsoft.com/oidc/userinfo".into()).ok();
 Ok((issuer, auth, token, jwks, userinfo))
}

pub async fn build_azure_client<S: Into<String>>(req_client:&ReqwestClient,client_id:S,client_secret:S,redirect_url:S,tenant_id:S) -> Result<OidcClient,Error> {
    let client_id = ClientId::new(client_id.into());
    let client_secret = ClientSecret::new(client_secret.into());
    let tenant_id = tenant_id.into();
    let redirect_uri = RedirectUrl::new(format!("{}/auth/azure/login", redirect_url.into()))?;
    let client = match tenant_id.as_str() {
        "common" | "organizations" | "consumers" => {
            let (issuer, auth, token, jwks, userinfo) = mk_urls(&tenant_id)?;
            let provider = CoreProviderMetadata::new(
                issuer,
                auth,
                jwks,
                vec![ResponseTypes::new(vec![CoreResponseType::Code])],
                vec![CoreSubjectIdentifierType::Public],
                vec![CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256], // RS256
                EmptyAdditionalProviderMetadata {},
            )
            .set_token_endpoint(Some(token))
            .set_userinfo_endpoint(userinfo);

            CoreClient::from_provider_metadata(provider, client_id, Some(client_secret))
        }
        _ => {
            let issuer = IssuerUrl::new(format!("https://login.microsoftonline.com/{}/v2.0",tenant_id))?;
            let provider = CoreProviderMetadata::discover_async(issuer,req_client).await?;
            CoreClient::from_provider_metadata(provider, client_id, Some(client_secret))
        }
    }
    .set_redirect_uri(redirect_uri);
    Ok(client)
}