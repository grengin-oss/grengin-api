// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::config::setting::OidcClient;
use crate::services::sso_egress::SsoHttpClient;
use anyhow::Error;
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use openidconnect::{ClientId, ClientSecret, IssuerUrl, RedirectUrl};

pub async fn build_google_client<S: Into<String>>(
    sso_client: &SsoHttpClient,
    client_id: S,
    client_secret: S,
    redirect_url: S,
) -> Result<OidcClient, Error> {
    let provider_metadata = CoreProviderMetadata::discover_async(
        IssuerUrl::new("https://accounts.google.com".to_string())?,
        sso_client,
    )
    .await?;
    let client = CoreClient::from_provider_metadata(
        provider_metadata,
        ClientId::new(client_id.into()),
        Some(ClientSecret::new(client_secret.into())),
    )
    .set_redirect_uri(RedirectUrl::new(format!("{}", redirect_url.into()))?);
    Ok(client)
}
