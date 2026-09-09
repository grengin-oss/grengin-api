// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Deserialize, IntoParams)]
pub struct DiscoveryQuery {
    /// A major version such as `1`, or an exact release such as `1.1.1`.
    pub version: Option<String>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DiscoveryVersion {
    pub version: String,
    pub schema_version: String,
    pub contract_version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DiscoveryProviderSummary {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub icon: Option<String>,
    pub icon_dark: Option<String>,
    pub selected_version: String,
    pub latest_version: String,
    pub available_versions: Vec<DiscoveryVersion>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DiscoveryListResponse {
    pub catalog_type: String,
    pub catalog_version: String,
    pub providers: Vec<DiscoveryProviderSummary>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AuthProviderDiscoveryResponse {
    pub id: String,
    pub version: String,
    pub schema_version: String,
    pub configuration_version: String,
    pub sha256: String,
    #[schema(value_type = Object)]
    pub template: Value,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AiProviderDiscoveryResponse {
    pub id: String,
    pub version: String,
    pub manifest_version: String,
    pub sha256: String,
    #[schema(value_type = Object)]
    pub plugin: Value,
}
