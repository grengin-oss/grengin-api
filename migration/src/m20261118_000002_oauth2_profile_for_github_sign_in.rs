// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// GitHub sign-in used to be built into the code; it is now the generic OAuth2 profile the catalog's
// github template carries. Existing GitHub providers get that profile so they keep signing in.
const UP: &str = r#"
UPDATE sso_providers
SET configuration = COALESCE(configuration, '{"version": "1.1", "scopes": ["read:user", "user:email"]}'::jsonb)
    || jsonb_build_object('oauth2', '{
        "authorizationUrl": "https://github.com/login/oauth/authorize",
        "tokenUrl": "https://github.com/login/oauth/access_token",
        "apiHeaders": {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"},
        "profile": {"url": "https://api.github.com/user", "subject": "/id", "displayName": ["/name", "/login"],
                    "picture": "/avatar_url", "email": "/email"},
        "verifiedEmails": {"url": "https://api.github.com/user/emails", "address": "/email",
                           "verified": "/verified", "primary": "/primary"},
        "requiredScopes": ["read:user", "user:email"],
        "credentialProbe": {"validError": "bad_verification_code",
                            "invalidCredentials": ["incorrect_client_credentials", "unauthorized_client"]}
    }'::jsonb)
WHERE provider = 'github' AND (configuration IS NULL OR NOT configuration ? 'oauth2');
"#;

const DOWN: &str = r#"
UPDATE sso_providers SET configuration = configuration - 'oauth2' WHERE provider = 'github';
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(UP)
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN)
            .await
            .map(|_| ())
    }
}
