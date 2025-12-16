use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize,ToSchema)]
pub struct OrgRequest {
    pub name: String,
    pub domain: String,
    pub allowed_domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    pub settings: OrgSettings,
}

#[derive(Serialize,ToSchema)]
pub struct OrgResponse {
    pub id: Uuid,
    pub name: String,
    pub domain: String,
    pub allowed_domains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_url: Option<String>,
    pub settings: OrgSettings,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize,Deserialize,ToSchema)]

pub struct OrgSettings {
    pub sso_providers: Vec<String>,
    pub default_engine: String,
    pub default_model: String,
    pub data_retention_days: i64,
    pub require_mfa: bool,
}