// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, PaginatorTrait, QueryFilter, Set, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::{
    auth::permissions::ROLE_SUPER_ADMIN,
    models::{roles, user_role_assignments, users},
    services::authorization::AuthorizationService,
};

const BOOTSTRAP_LOCK_SQL: &str =
    "SELECT pg_advisory_xact_lock(hashtext('grengin.bootstrap_super_admin'))";

fn valid_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && domain.contains('.')
        && !domain.contains('@')
        && value.len() <= 254
        && !value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
}

/// Create the owner account before the first login when Hatchery supplies an
/// owner email. The transaction-level advisory lock makes simultaneous Lambda
/// cold starts observe the same empty/non-empty decision.
pub async fn ensure_bootstrap_super_admin(
    database: &DatabaseConnection,
    configured_email: Option<&str>,
) -> Result<(), String> {
    let Some(configured_email) = configured_email else {
        return Ok(());
    };
    let email = configured_email.trim().to_ascii_lowercase();
    if !valid_email(&email) {
        return Err("BOOTSTRAP_SUPER_ADMIN_EMAIL is not a valid email address".to_string());
    }

    let transaction = database
        .begin()
        .await
        .map_err(|error| format!("start owner bootstrap transaction: {error}"))?;
    transaction
        .execute(Statement::from_string(
            DatabaseBackend::Postgres,
            BOOTSTRAP_LOCK_SQL,
        ))
        .await
        .map_err(|error| format!("lock owner bootstrap: {error}"))?;

    let user_count = users::Entity::find()
        .count(&transaction)
        .await
        .map_err(|error| format!("count users for owner bootstrap: {error}"))?;
    if user_count != 0 {
        transaction
            .commit()
            .await
            .map_err(|error| format!("finish owner bootstrap check: {error}"))?;
        return Ok(());
    }

    let role = roles::Entity::find()
        .filter(roles::Column::Name.eq(ROLE_SUPER_ADMIN))
        .one(&transaction)
        .await
        .map_err(|error| format!("find Super Admin role for owner bootstrap: {error}"))?
        .ok_or_else(|| {
            "Super Admin role is missing; run migrations before bootstrap".to_string()
        })?;

    let user_id = Uuid::new_v4();
    let now = Utc::now();
    let user = users::ActiveModel {
        id: Set(user_id),
        status: Set(users::UserStatus::Active),
        picture: Set(None),
        email: Set(email.clone()),
        email_verified: Set(true),
        name: Set(Some(email.clone())),
        password: Set(None),
        google_id: Set(None),
        azure_id: Set(None),
        mfa_enabled: Set(false),
        mfa_secret: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        last_login_at: Set(now),
        password_changed_at: Set(None),
        hd: Set(email.split_once('@').map(|(_, domain)| domain.to_string())),
        department_id: Set(None),
        is_independent: Set(false),
        effective_permissions: Set(None),
        metadata: Set(None),
        identities: Set(None),
    };
    user.insert(&transaction)
        .await
        .map_err(|error| format!("insert bootstrap owner: {error}"))?;

    user_role_assignments::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        role_id: Set(role.id),
        scope_department_id: Set(None),
        assigned_by: Set(user_id),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&transaction)
    .await
    .map_err(|error| format!("assign Super Admin role to bootstrap owner: {error}"))?;

    transaction
        .commit()
        .await
        .map_err(|error| format!("commit owner bootstrap: {error}"))?;

    AuthorizationService::new(database)
        .recompute_effective_permissions(user_id)
        .await
        .map_err(|error| format!("compute bootstrap owner permissions: {error:?}"))?;
    eprintln!("Bootstrapped the configured Super Admin owner");
    Ok(())
}
