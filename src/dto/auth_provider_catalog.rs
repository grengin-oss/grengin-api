// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthProviderCatalog {
    pub schema_version: String,
    pub catalog_version: String,
    pub providers: Vec<AuthProviderTemplateSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthProviderTemplateSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub protocol: AuthProviderProtocol,
    pub status: AuthProviderTemplateStatus,
    pub template_url: String,
    pub icon: String,
    pub icon_dark: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AuthProviderProtocol {
    Oidc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AuthProviderTemplateStatus {
    Stable,
    Preview,
}
