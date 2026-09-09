// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::dto::oauth::AuthProvider;
use reqwest::Url;

pub fn is_azure_mobile_redirect_uri(provider: &AuthProvider, redirect_uri: &str) -> bool {
    if !provider.eq_ignore_ascii_case("azure") {
        return false;
    }
    let Ok(uri) = Url::parse(redirect_uri) else {
        return false;
    };
    if !uri.username().is_empty()
        || uri.password().is_some()
        || uri.port().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
    {
        return false;
    }

    match uri.scheme() {
        "msauth" => {
            valid_mobile_app_id(uri.host_str())
                && uri.path().len() > 1
                && !has_raw_dot_path_segment(redirect_uri)
        }
        scheme if scheme.starts_with("msauth.") => {
            valid_mobile_app_id(scheme.strip_prefix("msauth."))
                && uri.host_str() == Some("auth")
                && matches!(uri.path(), "" | "/")
        }
        _ => false,
    }
}

fn has_raw_dot_path_segment(value: &str) -> bool {
    value
        .split_once("://")
        .and_then(|(_, authority_and_path)| authority_and_path.split_once('/'))
        .map(|(_, path)| path.split(['?', '#']).next().unwrap_or(path))
        .is_some_and(|path| {
            path.split('/').any(|segment| {
                matches!(
                    segment.to_ascii_lowercase().as_str(),
                    "." | ".." | "%2e" | "%2e%2e" | ".%2e" | "%2e."
                )
            })
        })
}

fn valid_mobile_app_id(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        value.len() <= 255
            && value.contains('.')
            && value.split('.').all(|segment| {
                !segment.is_empty()
                    && segment.chars().all(|character| {
                        character.is_ascii_alphanumeric() || character == '_' || character == '-'
                    })
            })
    })
}

pub fn origin_from_url(value: &str) -> Option<String> {
    let parsed = Url::parse(value).ok()?;
    Some(parsed.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_android_and_ios_msal_redirects() {
        let provider = "azure".to_string();

        assert!(is_azure_mobile_redirect_uri(
            &provider,
            "msauth://com.grengin.mobile/6%2FaB1cD2eF3gH4iJ5kL6-mN7oP8qR%3D"
        ));
        assert!(is_azure_mobile_redirect_uri(
            &provider,
            "msauth.com.grengin.mobile://auth"
        ));
    }

    #[test]
    fn rejects_malformed_or_non_azure_mobile_redirects() {
        for redirect_uri in [
            "msauth://missing-signature/",
            "msauth://com.grengin.mobile/../callback",
            "msauth://com.grengin.mobile/signature?code=leak",
            "msauth.com.grengin.mobile://attacker",
            "msauth.com.grengin.mobile://auth#fragment",
            "https://example.com/callback",
        ] {
            assert!(
                !is_azure_mobile_redirect_uri(&"azure".to_string(), redirect_uri),
                "accepted {redirect_uri}"
            );
        }
        assert!(!is_azure_mobile_redirect_uri(
            &"google".to_string(),
            "msauth://com.grengin.mobile/signature"
        ));
    }
}
