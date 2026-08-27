// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;

/// One linked external identity, keyed in [`IdentityMap`] by the auth provider
/// slug (matches `auth_providers.provider`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderIdentity {
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linked_at: Option<DateTime<Utc>>,
}

/// Contents of `users.identities`. Keyed by provider slug so a runtime-configured
/// provider needs no schema change, unlike the fixed google_id/azure_id columns.
pub type IdentityMap = HashMap<String, ProviderIdentity>;

#[derive(
    Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema,
)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "lowercase"
)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Deactivated,
    Deleted,
    Suspended,
    Pending,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "users", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique, indexed)]
    pub id: Uuid,
    pub status: UserStatus,
    pub picture: Option<String>,
    #[sea_orm(column_type = "Text", indexed)]
    pub email: String,
    pub email_verified: bool,
    pub name: Option<String>,
    pub password: Option<String>,
    #[sea_orm(column_type = "Text", unique, indexed, nullable)]
    pub google_id: Option<String>,
    #[sea_orm(column_type = "Text", unique, indexed, nullable)]
    pub azure_id: Option<String>,
    pub mfa_enabled: bool,
    pub mfa_secret: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: DateTime<Utc>,
    pub password_changed_at: Option<DateTime<Utc>>,
    pub hd: Option<String>,          //hosted domain of user email/website
    pub department_id: Option<Uuid>, // previously department Option<String>
    pub is_independent: bool,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub effective_permissions: Option<serde_json::Value>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub metadata: Option<serde_json::Value>,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub identities: Option<serde_json::Value>,
}

impl Model {
    pub fn identity_map(&self) -> IdentityMap {
        self.identities
            .as_ref()
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default()
    }

    pub fn identity_for(&self, provider: &str) -> Option<ProviderIdentity> {
        self.identity_map().remove(provider)
    }
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::conversations::Entity")]
    Conversations,
    #[sea_orm(has_many = "super::files::Entity")]
    Files,
    #[sea_orm(
        belongs_to = "super::departments::Entity",
        from = "Column::DepartmentId",
        to = "super::departments::Column::Id"
    )]
    Departments,
}

impl Related<super::conversations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversations.def()
    }
}

impl Related<super::files::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Files.def()
    }
}

impl Related<super::departments::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Departments.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
