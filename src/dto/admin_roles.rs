use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct PermissionDto {
    pub id: Uuid,
    pub domain: String,
    pub action: String,
    pub is_scopeable: bool,
    pub description_key: String,
}

#[derive(Serialize, ToSchema)]
pub struct PermissionsResponse {
    pub permissions: Vec<PermissionDto>,
}

#[derive(Serialize, ToSchema)]
pub struct RoleDto {
    pub id: Uuid,
    pub name: String,
    pub is_system: bool,
    pub permissions: Vec<String>,
    pub user_count: u64,
}

#[derive(Serialize, ToSchema)]
pub struct RolesResponse {
    pub roles: Vec<RoleDto>,
}

#[derive(Deserialize, ToSchema)]
pub struct RoleRequest {
    pub name: String,
    pub permissions: Vec<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct RoleUpdateRequest {
    pub name: Option<String>,
    pub permissions: Option<Vec<String>>,
}

#[derive(Serialize, ToSchema)]
pub struct UserRoleAssignmentDto {
    pub id: Uuid,
    pub role_id: Uuid,
    pub role_name: String,
    pub scope_department_id: Option<Uuid>,
    pub assigned_by: Uuid,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct UserRoleAssignmentsResponse {
    pub assignments: Vec<UserRoleAssignmentDto>,
}

#[derive(Deserialize, ToSchema)]
pub struct UserRoleAssignmentRequest {
    pub role_id: Uuid,
    pub scope_department_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct RoleCreatedPayload {
    pub role_id: Uuid,
    pub name: String,
    pub permissions: Vec<String>,
}

#[derive(Serialize)]
pub struct RoleUpdatedPayload {
    pub role_id: Uuid,
    pub name: Option<String>,
    pub permissions: Vec<String>,
}

#[derive(Serialize)]
pub struct RoleDeletedPayload {
    pub role_id: Uuid,
    pub name: String,
}

#[derive(Serialize)]
pub struct RoleAssignmentPayload {
    pub assignment_id: Uuid,
    pub user_id: Uuid,
    pub role_id: Uuid,
    pub scope_department_id: Option<Uuid>,
}
