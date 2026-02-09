use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDto {
    pub id: Uuid,
    pub domain: String,
    pub action: String,
    pub is_scopeable: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsResponse {
    pub permissions: Vec<PermissionDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleDto {
    pub id: Uuid,
    pub name: String,
    pub is_system: bool,
    pub permissions: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RolesResponse {
    pub roles: Vec<RoleDto>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleRequest {
    pub name: String,
    pub permissions: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleUpdateRequest {
    pub name: Option<String>,
    pub permissions: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserRoleAssignmentDto {
    pub id: Uuid,
    pub role_id: Uuid,
    pub role_name: String,
    pub scope_department_id: Option<Uuid>,
    pub assigned_by: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserRoleAssignmentsResponse {
    pub assignments: Vec<UserRoleAssignmentDto>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserRoleAssignmentRequest {
    pub role_id: Uuid,
    pub scope_department_id: Option<Uuid>,
}
