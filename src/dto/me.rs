// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{dto::common::SortRule, models::users::UserStatus};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub enum PermissionScope {
    Missing,
    OrgWide,
    Scoped(Vec<Uuid>),
}

#[derive(Serialize, ToSchema)]
pub struct AdministeredDepartmentsResponse {
    pub administered_departments: Vec<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct EffectivePermissionsResponse {
    #[schema(value_type = Object)]
    pub permissions: serde_json::Value,
    #[schema(value_type = Object)]
    pub mcp_access: serde_json::Value,
    pub administered_departments: Vec<String>,
}

#[derive(Serialize, ToSchema)]
pub struct MeDepartmentUsersResponse {
    pub total: i32,
    pub users: Vec<crate::dto::admin_user::User>,
}

#[derive(Deserialize, ToSchema)]
pub struct AdministeredDepartmentUsersQuery {
    pub department_id: Option<Uuid>,
    #[serde(default)]
    pub include_sub_department: Option<bool>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub search: Option<String>,
    pub order: Option<String>,
    pub role_id: Option<Uuid>,
    pub status: Option<UserStatus>,
    pub sort: Option<SortRule>,
}
