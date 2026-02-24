use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::models::mcp_server_access_rules::{McpRuleType, McpSubjectType};
use crate::models::mcp_servers::McpAccessDefault;

#[derive(Serialize, ToSchema)]
pub struct McpAccessRuleDto {
    pub id: Uuid,
    pub subject_type: McpSubjectType,
    pub subject_id: Uuid,
    pub rule_type: McpRuleType,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct McpServerAccessResponse {
    pub server_id: Uuid,
    pub access_default: McpAccessDefault,
    pub rules: Vec<McpAccessRuleDto>,
}

#[derive(Deserialize, ToSchema)]
pub struct McpAccessDefaultRequest {
    pub access_default: McpAccessDefault,
}

#[derive(Deserialize, ToSchema)]
pub struct McpAccessRuleRequest {
    pub subject_type: McpSubjectType,
    pub subject_id: Uuid,
    pub rule_type: McpRuleType,
}

#[derive(Serialize)]
pub struct McpAccessDefaultChangedPayload {
    pub server_id: Uuid,
    pub access_default: McpAccessDefault,
}

#[derive(Serialize)]
pub struct McpAccessRuleCreatedPayload {
    pub rule_id: Uuid,
    pub server_id: Uuid,
    pub subject_type: McpSubjectType,
    pub subject_id: Uuid,
    pub rule_type: McpRuleType,
}

#[derive(Serialize)]
pub struct McpAccessRuleDeletedPayload {
    pub rule_id: Uuid,
    pub server_id: Uuid,
}
