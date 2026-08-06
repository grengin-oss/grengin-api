// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::dto::mcp::{McpAccessRule, McpAccessRuleInput};
use crate::models::mcp_access_policies::{McpAccessType, McpPermission};
use crate::models::mcp_servers::McpDefaultAccess;

#[derive(Serialize, ToSchema)]
pub struct McpServerAccess {
    pub server_id: Uuid,
    pub default_access: McpDefaultAccess,
    pub rules: Vec<McpAccessRule>,
}

#[derive(Deserialize, ToSchema)]
pub struct McpAccessDefault {
    pub default_access: McpDefaultAccess,
}

pub type McpAccessRuleRequest = McpAccessRuleInput;

#[derive(Serialize)]
pub struct McpAccessDefaultChangedPayload {
    pub server_id: Uuid,
    pub default_access: McpDefaultAccess,
}

#[derive(Serialize)]
pub struct McpAccessRuleCreatedPayload {
    pub rule_id: Uuid,
    pub server_id: Uuid,
    pub access_type: McpAccessType,
    pub permission: McpPermission,
    pub role_id: Option<Uuid>,
    pub role_name: Option<String>,
    pub department_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub inherit_departments: bool,
}

#[derive(Serialize)]
pub struct McpAccessRuleDeletedPayload {
    pub rule_id: Uuid,
    pub server_id: Uuid,
}
