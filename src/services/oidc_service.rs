// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        azure::build_azure_public_client,
        claims::{Claiming as _, Claims, RefreshClaims},
        error::AuthError,
        github::GitHubAdapterError,
        identity::VerifiedIdentity,
        provider_config::EmailLinkingMode,
    },
    config::setting::OidcClient,
    dto::{
        auth::{AuthToken, TokenType, User},
        oauth::{AuthCallback, AuthProvider, CallbackExchangeMode},
    },
    models::{
        oauth_sessions, roles, user_role_assignments,
        users::{self, UserStatus},
    },
    services::{authorization::AuthorizationService, oidc_proxy::verify_proxy_assertion},
    state::{AuthProtocolClient, SharedState},
    utils::uri::{is_azure_mobile_redirect_uri, origin_from_url},
};
use axum::{Json, http::StatusCode};
use chrono::Utc;
use openidconnect::{
    AuthorizationCode, ClaimsVerificationError, Nonce, PkceCodeVerifier, RedirectUrl,
    core::CoreUserInfoClaims,
};
use openidconnect::{OAuth2TokenResponse, TokenResponse as OidcTokenResponse};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter,
    QueryOrder, TryIntoModel, sea_query::Expr,
};
use std::borrow::Cow;
use uuid::Uuid;

pub async fn build_azure_public_client_for_redirect(
    app_state: &SharedState,
    redirect_uri: &str,
) -> Result<OidcClient, AuthError> {
    let azure = app_state.settings.azure.read().await.clone().ok_or(
        AuthError::SsoProviderNotConfigured {
            provider: Some("azure".to_string()),
        },
    )?;
    build_azure_public_client(
        &app_state.req_client,
        azure.client_id,
        redirect_uri.to_string(),
        azure.tenant_id,
    )
    .await
    .map_err(|e| {
        eprintln!("azure public oidc client build error: {e:?}");
        AuthError::ServiceTemporarilyUnavailable
    })
}

/// Merge one provider identity into a user's identity map. Keyed by provider slug so a
/// runtime-configured provider needs no schema change; google/azure keep writing their
/// legacy columns too because dto/auth.rs still derives `sub` from them.
fn merged_identities(
    existing: &users::IdentityMap,
    provider: &str,
    subject: &str,
    email: Option<&str>,
) -> Option<serde_json::Value> {
    let mut map = existing.clone();
    map.insert(
        provider.to_ascii_lowercase(),
        users::ProviderIdentity {
            subject: subject.to_string(),
            email: email.map(str::to_string),
            linked_at: Some(Utc::now()),
        },
    );
    serde_json::to_value(map).ok()
}

pub async fn oidc_oauth_callback(
    provider: AuthProvider,
    cb: AuthCallback,
    app_state: SharedState,
    exchange_mode: CallbackExchangeMode,
) -> Result<(StatusCode, Json<AuthToken>), AuthError> {
    if let Some(error) = cb.error {
        eprintln!("OAuth error: {} - {:?}", error, cb.error_description);
        return Err(AuthError::InvalidCallbackParameters);
    }
    if matches!(exchange_mode, CallbackExchangeMode::AzureMobilePublic)
        && !provider.eq_ignore_ascii_case("azure")
    {
        return Err(AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        });
    }
    let runtime = app_state
        .get_oidc_provider_runtime(&provider)
        .await
        .map_err(|_| AuthError::InvalidProvider {
            provider: Some(provider.clone()),
        })?;
    let default_redirect_uri = Some(runtime.redirect_url.clone());
    let sess = oauth_sessions::Entity::find()
        .filter(oauth_sessions::Column::State.eq(Some(cb.state.to_owned())))
        .order_by_desc(oauth_sessions::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while fetching session: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?
        .ok_or(AuthError::InvalidToken)?;
    // Reject sessions older than 15 minutes — prevents indefinitely-valid state tokens.
    if sess.created_at < Utc::now() - chrono::Duration::minutes(15) {
        return Err(AuthError::InvalidToken);
    }
    let redirect_uri_value = sess
        .redirect_uri
        .clone()
        .unwrap_or(
            default_redirect_uri.ok_or(AuthError::SsoProviderNotConfigured {
                provider: Some(provider.clone()),
            })?,
        );
    let is_mobile_redirect = is_azure_mobile_redirect_uri(&provider, &redirect_uri_value);
    if matches!(exchange_mode, CallbackExchangeMode::AzureMobilePublic) && !is_mobile_redirect {
        return Err(AuthError::InvalidRedirectUri {
            redirect_uri: Some(redirect_uri_value),
        });
    }
    let redirect_uri = RedirectUrl::new(redirect_uri_value.clone()).map_err(|_| {
        AuthError::InvalidRedirectUri {
            redirect_uri: sess.redirect_uri.clone(),
        }
    })?;
    let active: oauth_sessions::ActiveModel = sess.clone().into();
    active.delete(&app_state.database).await.map_err(|e| {
        eprintln!("db error while deleting oauth_session: {e:?}");
        AuthError::ServiceTemporarilyUnavailable
    })?;
    let identity = if let Some(assertion) = cb.assertion.clone() {
        let expected_audience =
            origin_from_url(&redirect_uri_value).ok_or(AuthError::InvalidRedirectUri {
                redirect_uri: Some(redirect_uri_value.clone()),
            })?;
        let claims = verify_proxy_assertion(
            &app_state,
            &provider,
            &assertion,
            &sess.nonce,
            &expected_audience,
        )
        .await?;
        VerifiedIdentity {
            subject: claims.provider_sub.ok_or(AuthError::InvalidToken)?,
            email: claims.email,
            email_verified: true,
            display_name: claims.name,
            picture: claims.picture,
            hosted_domain: None,
        }
    } else {
        let code = cb.code.ok_or(AuthError::InvalidCallbackParameters)?;
        if provider.eq_ignore_ascii_case("github") {
            let adapter = match runtime.client.clone() {
                Some(AuthProtocolClient::GitHub(adapter)) => adapter,
                _ => {
                    return Err(AuthError::SsoProviderNotConfigured {
                        provider: Some(provider.clone()),
                    });
                }
            };
            adapter
                .complete(code, sess.pkce_verifier.clone(), &app_state.req_client)
                .await
                .map_err(|error| {
                    eprintln!("GitHub OAuth completion failed: {error}");
                    match error {
                        GitHubAdapterError::MissingVerifiedEmail
                        | GitHubAdapterError::InvalidIdentity => AuthError::InvalidToken,
                        _ => AuthError::ServiceTemporarilyUnavailable,
                    }
                })?
        } else {
            let use_public_azure_client = is_mobile_redirect;
            let mut oidc_client = if use_public_azure_client {
                build_azure_public_client_for_redirect(&app_state, &redirect_uri_value).await?
            } else {
                match runtime.client.clone() {
                    Some(AuthProtocolClient::Oidc(client)) => client,
                    _ => {
                        return Err(AuthError::SsoProviderNotConfigured {
                            provider: Some(provider.clone()),
                        });
                    }
                }
            };
            let token_resp = oidc_client
                .exchange_code(AuthorizationCode::new(code))
                .expect("Failed to get token response")
                .set_pkce_verifier(PkceCodeVerifier::new(sess.pkce_verifier.clone()))
                .set_redirect_uri(Cow::Owned(redirect_uri))
                .request_async(&app_state.req_client)
                .await
                .map_err(|e| {
                    eprintln!("token exchange err: {e:?}");
                    AuthError::ServiceTemporarilyUnavailable
                })?;
            let nonce = Nonce::new(sess.nonce.clone());
            let id_token = token_resp
                .id_token()
                .ok_or(AuthError::ServiceTemporarilyUnavailable)?;
            let claims = match {
                let verifier = oidc_client.id_token_verifier();
                id_token.claims(&verifier, &nonce)
            } {
                Ok(c) => c,
                Err(e) => {
                    let should_refresh =
                        matches!(e, ClaimsVerificationError::SignatureVerification(_));
                    if !should_refresh {
                        eprintln!("id_token claims verification failed (non-refreshable): {e:?}");
                        return Err(AuthError::InvalidToken);
                    }
                    if use_public_azure_client {
                        oidc_client =
                            build_azure_public_client_for_redirect(&app_state, &redirect_uri_value)
                                .await?;
                    } else {
                        app_state
                            .refresh_oidc_client(&provider)
                            .await
                            .map_err(|err| {
                                eprintln!("oidc client refresh error: {err:?}");
                                AuthError::ServiceTemporarilyUnavailable
                            })?;

                        oidc_client = match app_state
                            .get_oidc_provider_runtime(&provider)
                            .await
                            .map_err(|_| AuthError::SsoProviderNotConfigured {
                                provider: Some(provider.clone()),
                            })?
                            .client
                        {
                            Some(AuthProtocolClient::Oidc(client)) => client,
                            _ => {
                                return Err(AuthError::SsoProviderNotConfigured {
                                    provider: Some(provider.clone()),
                                });
                            }
                        };
                    }

                    let verifier2 = oidc_client.id_token_verifier();
                    id_token.claims(&verifier2, &nonce).map_err(|e2| {
                        eprintln!("id_token claims verification failed after refresh: {e2:?}");
                        AuthError::InvalidToken
                    })?
                }
            };
            let subject = claims.subject().as_str().to_string();
            let mut email = claims.email().map(|e| e.as_str().to_string());
            let mut email_verified = claims
                .email_verified()
                .unwrap_or_else(|| provider.eq_ignore_ascii_case("azure"));
            let picture = claims
                .picture()
                .and_then(|pic_claim| pic_claim.get(None))
                .map(|url| url.as_str().to_owned());
            let hosted_domain = claims
                .website()
                .and_then(|website_claim| website_claim.get(None))
                .map(|url| url.as_str().to_owned());
            let mut display_name = claims
                .name()
                .and_then(|n| n.get(None).map(|s| s.to_string()));
            if email.is_none() {
                let info: CoreUserInfoClaims = oidc_client
                    .user_info(token_resp.access_token().to_owned(), None)
                    .expect("userinfo req")
                    .request_async(&app_state.req_client)
                    .await
                    .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
                email = info.email().map(|e| e.as_str().to_string());
                email_verified = info.email_verified().unwrap_or(email_verified);
                if display_name.is_none() {
                    display_name = info.name().and_then(|n| n.get(None).map(|s| s.to_string()));
                }
            }
            VerifiedIdentity {
                subject,
                email,
                email_verified,
                display_name,
                picture,
                hosted_domain,
            }
        }
    };
    let VerifiedIdentity {
        subject: sub,
        email,
        email_verified,
        display_name,
        picture,
        hosted_domain: hd,
    } = identity;

    let google_id = if provider == "google" {
        Some(sub.clone())
    } else {
        None
    };
    let azure_id = if provider == "azure" {
        Some(sub.clone())
    } else {
        None
    };
    if let Some(email) = email.as_ref() {
        if !email_verified && !runtime.allowed_domains.is_empty() {
            return Err(AuthError::InvalidToken);
        }
        let (is_allowed, domain) = app_state.is_email_domain_allowed(email, &provider).await;
        if !is_allowed {
            return Err(AuthError::EmailDomainNotAllowed { domain });
        }
    }
    let normalized_provider = provider.trim().to_ascii_lowercase();
    let identity_match = serde_json::json!({
        normalized_provider.clone(): { "subject": sub.clone() }
    });
    let mut user = users::Entity::find()
        .filter(Expr::cust_with_values(
            r#""identities" @> $1::jsonb"#,
            [identity_match],
        ))
        .filter(users::Column::Status.ne(UserStatus::Deleted))
        .order_by_desc(users::Column::CreatedAt)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("db error while fetching user: {e:?}");
            AuthError::ServiceTemporarilyUnavailable
        })?;
    if user.is_none() {
        let legacy_column = match normalized_provider.as_str() {
            "google" => Some(users::Column::GoogleId),
            "azure" => Some(users::Column::AzureId),
            _ => None,
        };
        if let Some(column) = legacy_column {
            user = users::Entity::find()
                .filter(column.eq(Some(sub.clone())))
                .filter(users::Column::Status.ne(UserStatus::Deleted))
                .order_by_desc(users::Column::CreatedAt)
                .one(&app_state.database)
                .await
                .map_err(|error| {
                    eprintln!("db error while fetching legacy OIDC identity: {error:?}");
                    AuthError::ServiceTemporarilyUnavailable
                })?;
        }
    }
    if let Some(u) = &user {
        match &u.status {
            UserStatus::Deactivated | UserStatus::Suspended => {
                return Err(AuthError::AccountDeactivated);
            }
            UserStatus::Pending => {
                return Err(AuthError::AccountPendingApproval);
            }
            _ => (),
        }
        let mut active_user: users::ActiveModel = u.clone().into();
        active_user.identities = Set(merged_identities(
            &u.identity_map(),
            &provider,
            &sub,
            email.as_deref(),
        ));
        active_user.last_login_at = Set(Utc::now());
        active_user.update(&app_state.database).await.map_err(|e| {
            eprintln!("db error while updating user {:?}", e);
            AuthError::ServiceTemporarilyUnavailable
        })?;
    }
    if user.is_none() {
        if let Some(ref em) = email {
            user = users::Entity::find()
                .filter(users::Column::Email.eq(em))
                .filter(users::Column::Status.ne(UserStatus::Deleted))
                .one(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("{:?}", e);
                    AuthError::ServiceTemporarilyUnavailable
                })?;

            if let Some(u) = &user {
                match &u.status {
                    UserStatus::Deactivated | UserStatus::Suspended => {
                        return Err(AuthError::AccountDeactivated);
                    }
                    _ => (),
                }
                let configured_linking = matches!(
                    runtime.configuration.email_linking,
                    EmailLinkingMode::VerifiedEmail
                );
                if !configured_linking || !email_verified {
                    return Err(AuthError::InvalidToken);
                }
                if u.identity_for(&normalized_provider)
                    .is_some_and(|identity| identity.subject != sub)
                {
                    return Err(AuthError::InvalidToken);
                }
                let can_link_google = google_id.is_some() && u.google_id.is_none();
                let can_link_azure = azure_id.is_some() && u.azure_id.is_none();
                let mut active_user: users::ActiveModel = u.clone().into();
                if can_link_google {
                    active_user.google_id = Set(google_id.clone());
                }
                if can_link_azure {
                    active_user.azure_id = Set(azure_id.clone());
                }
                active_user.identities = Set(merged_identities(
                    &u.identity_map(),
                    &provider,
                    &sub,
                    email.as_deref(),
                ));
                active_user.updated_at = Set(Utc::now());
                active_user.last_login_at = Set(Utc::now());
                active_user.update(&app_state.database).await.map_err(|e| {
                    eprintln!("db error while updating user {:?}", e);
                    AuthError::ServiceTemporarilyUnavailable
                })?;
            }
        }
    }
    if user.is_none() {
        let is_first_user = users::Entity::find()
            .count(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("db error while counting users {:?}", e);
                AuthError::ServiceTemporarilyUnavailable
            })?
            == 0;
        let jit_provisioning = app_state.sso_jit_provisioning_enabled(&provider).await;
        if !is_first_user && !jit_provisioning {
            return Err(AuthError::SsoJitProvisioningDisabled {
                provider: Some(provider.clone()),
            });
        }
        let initial_status = if is_first_user {
            UserStatus::Active
        } else {
            UserStatus::Pending
        };
        let new_user = users::ActiveModel {
            id: Set(Uuid::new_v4()),
            email: Set(email
                .clone()
                .unwrap_or_else(|| format!("{sub}@users.noreply.oidc"))),
            name: Set(display_name.into()),
            google_id: Set(google_id),
            azure_id: Set(azure_id),
            email_verified: Set(email_verified),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            last_login_at: Set(Utc::now()),
            password_changed_at: Set(None),
            department_id: Set(None),
            is_independent: Set(false),
            effective_permissions: Set(None),
            status: Set(initial_status),
            mfa_enabled: Set(false),
            mfa_secret: Set(None),
            picture: Set(picture.clone()),
            password: Set(None),
            metadata: Set(None),
            hd: Set(hd),
            identities: Set(merged_identities(
                &users::IdentityMap::new(),
                &provider,
                &sub,
                email.as_deref(),
            )),
        };
        new_user
            .clone()
            .insert(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("{:?}", e);
                AuthError::ServiceTemporarilyUnavailable
            })?;
        let inserted_user = new_user.try_into_model().map_err(|e| {
            eprintln!("db error while parsing user {:?}", e);
            AuthError::ServiceTemporarilyUnavailable
        })?;
        let role_name = if is_first_user { "Super Admin" } else { "User" };
        if let Some(role) = roles::Entity::find()
            .filter(roles::Column::Name.eq(role_name))
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::ServiceTemporarilyUnavailable
            })?
        {
            let now = Utc::now();
            let assignment = user_role_assignments::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(inserted_user.id),
                role_id: Set(role.id),
                scope_department_id: Set(None),
                assigned_by: Set(inserted_user.id),
                created_at: Set(now),
                updated_at: Set(now),
            };
            let _ = assignment.insert(&app_state.database).await;
        }
        let authz = AuthorizationService::new(&app_state.database);
        let _ = authz
            .recompute_effective_permissions(inserted_user.id)
            .await;
        let pending_approval = !is_first_user && jit_provisioning;
        user = Some(inserted_user);
        if pending_approval {
            return Err(AuthError::AccountPendingApproval);
        }
    };
    let user = user.ok_or(AuthError::EmailDoesNotExist)?;

    let access_token_claims =
        Claims::new_access_token(user.email.clone(), user.name.clone(), user.id);
    let refresh_token_claims = RefreshClaims::new_refresh_token(user.email.clone(), user.id);
    let authz = AuthorizationService::new(&app_state.database);
    let mut roles_map = authz.user_roles_map(&[user.id]).await?;
    let roles = roles_map.remove(&user.id).unwrap_or_default();
    let is_super_admin = roles.iter().any(|r| r == "Super Admin");
    let user_response = User {
        id: user.id,
        sub: sub.clone(),
        email: user.email,
        name: user.name,
        picture: picture,
        hd: user.hd,
        roles,
        status: user.status,
        department_id: user.department_id,
        is_super_admin,
        has_password: user.password.is_some(),
        mfa_enabled: user.mfa_enabled,
        last_login_at: Some(user.last_login_at),
        password_changed_at: None,
        created_at: user.created_at,
        updated_at: user.updated_at,
        effective_permissions: user.effective_permissions,
    };

    let resp = AuthToken {
        access_token: access_token_claims.get_token_string(),
        token_type: TokenType::Bearer,
        expires_in: 3600,
        refresh_token: Some(refresh_token_claims.get_token_string()),
        user: Some(user_response),
    };
    Ok((StatusCode::OK, Json(resp)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_merge_is_provider_scoped() {
        let mut existing = users::IdentityMap::new();
        existing.insert(
            "google".to_string(),
            users::ProviderIdentity {
                subject: "google-subject".to_string(),
                email: Some("user@example.com".to_string()),
                linked_at: None,
            },
        );

        let value = merged_identities(
            &existing,
            "Keycloak-EU",
            "keycloak-subject",
            Some("user@example.com"),
        )
        .expect("serialized identity map");
        let merged: users::IdentityMap =
            serde_json::from_value(value).expect("parsed identity map");

        assert_eq!(merged.len(), 2);
        assert_eq!(merged["google"].subject, "google-subject");
        assert_eq!(merged["keycloak-eu"].subject, "keycloak-subject");
        assert!(merged["keycloak-eu"].linked_at.is_some());
    }

    #[test]
    fn identity_merge_replaces_only_the_same_provider() {
        let first = merged_identities(&users::IdentityMap::new(), "okta", "old-subject", None)
            .expect("first identity map");
        let first: users::IdentityMap =
            serde_json::from_value(first).expect("parsed first identity map");
        let second =
            merged_identities(&first, "OKTA", "new-subject", None).expect("second identity map");
        let second: users::IdentityMap =
            serde_json::from_value(second).expect("parsed second identity map");

        assert_eq!(second.len(), 1);
        assert_eq!(second["okta"].subject, "new-subject");
    }
}
