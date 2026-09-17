// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginConfig {
    #[schema(value_type = Object)]
    pub manifest: Json,
    #[schema(value_type = Object)]
    #[serde(default = "empty_json_object")]
    pub configuration: Json,
    pub base_url_override: Option<String>,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub allow_private_network: bool,
    // Defaults to Custom (not Catalog) on deserialize: this struct also backs the
    // admin-facing create/update request body, so an old stored row or a request
    // that omits `source` must never be mistaken for catalog-tracked and picked
    // up by reconcile's auto-upgrade. Only reconcile_catalog_ai_engines is allowed
    // to write `Catalog` — see services/ai_engine_catalog.rs.
    #[serde(default)]
    pub source: PluginConfigSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

fn empty_json_object() -> Json {
    Json::Object(Default::default())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PluginConfigSource {
    Catalog,
    Custom,
    Embedded,
}

impl Default for PluginConfigSource {
    fn default() -> Self {
        Self::Custom
    }
}

impl std::fmt::Display for PluginConfigSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Catalog => "catalog",
            Self::Custom => "custom",
            Self::Embedded => "embedded",
        })
    }
}

impl TryFrom<String> for PluginConfigSource {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "catalog" => Ok(Self::Catalog),
            "custom" => Ok(Self::Custom),
            "embedded" => Ok(Self::Embedded),
            other => Err(anyhow::anyhow!("unknown plugin config source: {other}")),
        }
    }
}

#[derive(
    Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema,
)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "snake_case"
)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyStatus {
    Valid,
    Invalid,
    NotValidated,
    NotConfigured,
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ai_engines", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique, indexed)]
    pub id: Uuid,
    pub display_name: String,
    pub is_enabled: bool,
    #[sea_orm(indexed)]
    pub engine_key: String,
    pub api_key_status: ApiKeyStatus,
    pub api_key: Option<String>,
    pub whitelist_models: Vec<String>,
    pub default_model: String,
    pub default_image_gen_model: Option<String>,
    pub plugin_config: Option<Json>,
    pub api_key_validated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
