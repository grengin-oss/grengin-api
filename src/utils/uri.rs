// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::dto::oauth::AuthProvider;
use reqwest::Url;

pub fn is_azure_mobile_redirect_uri(provider: &AuthProvider, redirect_uri: &str) -> bool {
    provider.eq_ignore_ascii_case("azure")
        && redirect_uri
            .get(..9)
            .map(|scheme| scheme.eq_ignore_ascii_case("msauth://"))
            .unwrap_or(false)
}

pub fn origin_from_url(value: &str) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    Some(parsed.origin().ascii_serialization())
}
