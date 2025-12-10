use openidconnect::{core::{CoreClient, CoreProviderMetadata}};
use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl};
use anyhow::Error;
use reqwest::Client as ReqwestClient;
use crate::config::setting::OidcClient;

pub async fn build_google_client<S: Into<String>>(req_client:&ReqwestClient,client_id:S,client_secret:S,redirect_url:S) -> Result<OidcClient,Error> {
    let provider_metadata = CoreProviderMetadata::discover_async(
        IssuerUrl::new("https://accounts.google.com".to_string())?,
        req_client).await?;
    let client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(client_id.into()),
        Some(ClientSecret::new(client_secret.into())),
    )
    .set_redirect_uri(RedirectUrl::new(format!("{}",redirect_url.into()))?);
  Ok(client)
}