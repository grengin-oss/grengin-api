// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Deserialize, ToSchema)]
pub struct EmbeddingConfigUpdateRequest {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub dimensions: Option<i32>,
    pub is_enabled: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct EmbeddingConfigResponse {
    pub provider: String,
    pub model: String,
    pub dimensions: Option<i32>,
    pub is_enabled: bool,
    pub api_key_configured: bool,
    pub provider_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
