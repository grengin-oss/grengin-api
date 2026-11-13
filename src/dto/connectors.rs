// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConnectConnectorRequest {
    pub version: String,
    pub server_id: Option<Uuid>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CompleteConnectorRequest {
    pub confirmation_code: String,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub enum ProviderRevocation {
    #[serde(rename = "revoked")]
    Revoked,
    #[serde(rename = "pending")]
    Pending,
    #[serde(rename = "unsupported")]
    Unsupported,
    #[serde(rename = "not_applicable")]
    NotApplicable,
}

#[derive(Serialize, ToSchema)]
pub struct ConnectorConnection {
    pub id: Uuid,
    pub connector_key: String,
    pub catalog_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_id: Option<Uuid>,
    pub status: crate::models::connectors::ConnectorStatus,
    pub revocation_pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_revocation: Option<ProviderRevocation>,
    pub scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connected_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization_url: Option<String>,
}

#[derive(Deserialize)]
pub struct BrokerStatus {
    pub status: crate::models::connectors::ConnectorStatus,
    pub authorization_url: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub scopes: Vec<String>,
    pub mcp_url: Option<String>,
    pub transport: Option<String>,
    #[serde(default)]
    pub revocation_pending: bool,
    pub provider_revocation: Option<ProviderRevocation>,
}

#[derive(Deserialize)]
pub struct BrokerToken {
    pub access_token: String,
    pub expires_at: Option<DateTime<Utc>>,
}
