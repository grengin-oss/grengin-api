// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderPluginValidationRequest {
    #[schema(value_type = Object)]
    pub manifest: Value,
    pub base_url_override: Option<String>,
    #[schema(value_type = Object)]
    #[serde(default = "empty_object")]
    pub configuration: Value,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub allow_private_network: bool,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPluginValidationResponse {
    pub valid: bool,
    pub provider_key: Option<String>,
    pub version: Option<String>,
    pub name: Option<String>,
    pub digest: Option<String>,
    pub destination: Option<String>,
    pub credential_slots: Vec<ProviderCredentialDefinitionResponse>,
    #[schema(value_type = Object)]
    pub capabilities: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredentialDefinitionResponse {
    pub slot_id: String,
    pub label: Option<String>,
    pub credential_type: String,
    pub required: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderPluginInstallRequest {
    #[schema(value_type = Object)]
    pub manifest: Value,
    #[schema(value_type = Object)]
    #[serde(default)]
    pub credentials: BTreeMap<String, String>,
    #[schema(value_type = Object)]
    #[serde(default = "empty_object")]
    pub configuration: Value,
    pub base_url_override: Option<String>,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default)]
    pub allow_private_network: bool,
    #[serde(default)]
    pub enabled: bool,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCredentialSlotResponse {
    pub slot_id: String,
    pub configured: bool,
    pub status: String,
    pub validated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPluginResponse {
    pub id: Uuid,
    pub provider_key: String,
    pub version: String,
    pub name: String,
    pub digest: String,
    pub source: String,
    pub status: String,
    pub validation_error: Option<String>,
    pub destination: String,
    #[schema(value_type = Object)]
    pub capabilities: Value,
    pub credential_slots: Vec<ProviderCredentialSlotResponse>,
    pub allow_insecure_http: bool,
    pub allow_private_network: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPluginConnectionTestResponse {
    pub valid: bool,
    pub mode: String,
    pub models_available: Option<usize>,
    pub error_class: Option<String>,
}
