use openidconnect::{core::{CoreClient, CoreProviderMetadata}};
use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl};
use anyhow::Error;
use crate::config::setting::OidcClient;

pub async fn build_azure_client<S: Into<String>>(client_id:S,client_secret:S,redirect_url:S,tenant_id:S) -> Result<OidcClient,Error> {
    let http_client = reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let provider_metadata = CoreProviderMetadata::discover_async(
        IssuerUrl::new(format!("https://login.microsoftonline.com/{}/v2.0",tenant_id.into()))?,
        &http_client,).await?;
    let client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(client_id.into()),
        Some(ClientSecret::new(client_secret.into())),
    )
    .set_redirect_uri(RedirectUrl::new(format!("{}",redirect_url.into()))?);
  Ok(client)
}