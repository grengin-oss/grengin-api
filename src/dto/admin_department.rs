use crate::{
    dto::{admin_user::User, common::SortRule},
    models::{
        departments::{ActionOnExceed, BudgetPeriod},
        users::UserStatus,
    },
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sea_orm::FromQueryResult;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, IntoParams, ToSchema)]
pub struct DepartmentMembersResponse {
    pub total: i32,
    pub members: Vec<User>,
}

#[derive(Serialize, IntoParams, ToSchema)]
pub struct DepartmentsListResponse {
    pub departments: Vec<Department>,
    pub total: i64,
}

#[derive(Deserialize, ToSchema)]
pub struct DepartmentListQuery {
    pub parent_id: Option<String>,
    #[serde(default)]
    pub include_children: bool,
    pub search: Option<String>,
    pub sort: Option<DepartmentSortRule>,
    pub ascending: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DepartmentSortRule {
    Name,
    CreatedAt,
    UpdatedAt,
    Members,
    SubDepartments,
}

#[derive(Deserialize, IntoParams, ToSchema)]
pub struct DepartmentTreeQuery {
    pub root_id: Option<Uuid>,
    pub max_depth: Option<i32>,
}

#[derive(Deserialize, ToSchema)]
pub struct DepartmentMemeberListQuery {
    pub force: Option<bool>,
    #[serde(default)]
    pub include_sub_department: Option<bool>,
    pub search: Option<String>,
    pub archived: Option<bool>,
    pub order: Option<String>,
    pub role_id: Option<Uuid>,
    pub status: Option<UserStatus>,
    pub sort: Option<SortRule>,
}

#[derive(Serialize, IntoParams, ToSchema)]
pub struct Department {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub parent_id: Option<Uuid>,
    pub path: String,
    pub depth: i32,
    pub admin_ids: Vec<Uuid>,
    pub member_count: i32,
    pub total_member_count: i32,
    pub child_count: i32,
    pub budget_allocated: f64,
    pub budget_distributed: f64,
    pub budget_available: f64,
    pub budget_used: f64,
    pub budget_period: BudgetPeriod,
    pub retention_days: Option<i32>,
    #[serde(default)]
    pub allowed_models: Vec<DepartmentModelKey>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema, Clone)]
pub struct DepartmentTreeNode {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub parent_id: Option<Uuid>,
    pub path: String,
    pub depth: i32,
    pub admin_ids: Vec<Uuid>,
    pub member_count: i32,
    pub total_member_count: i32,
    pub child_count: i32,
    pub budget_allocated: f64,
    pub budget_distributed: f64,
    pub budget_available: f64,
    pub budget_used: f64,
    pub budget_period: BudgetPeriod,
    pub retention_days: Option<i32>,
    #[serde(default)]
    pub allowed_models: Vec<DepartmentModelKey>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[schema(no_recursion)]
    pub children: Vec<DepartmentTreeNode>,
}

#[derive(Serialize, ToSchema)]
pub struct DepartmentTree {
    pub tree: Vec<DepartmentTreeNode>,
}

#[derive(Deserialize, IntoParams, ToSchema)]
pub struct DepartmentCreate {
    pub name: String,
    pub description: String,
    pub parent_id: Option<Uuid>,
    pub admin_ids: Option<Vec<Uuid>>,
    pub retention_days: Option<i32>,
    pub allowed_models: Option<Vec<DepartmentModelKey>>,
}

#[derive(Deserialize, IntoParams, ToSchema)]
pub struct DepartmentUpdate {
    pub name: Option<String>,
    pub description: Option<String>,
    pub parent_id: Option<Uuid>,
    pub admin_ids: Option<Vec<Uuid>>,
    pub budget_allocated: Option<f32>,
    pub budget_period: Option<BudgetPeriod>,
    pub action_on_exceed: Option<ActionOnExceed>,
    pub retention_days: Option<i32>,
    pub allowed_models: Option<Vec<DepartmentModelKey>>,
}

#[derive(Serialize, Deserialize, ToSchema, IntoParams, Clone, PartialEq, Eq, Hash)]
pub struct DepartmentModelKey {
    pub provider: String,
    pub model: String,
}

#[derive(Deserialize, ToSchema)]
pub struct DepartmentMove {
    pub new_parent_id: Uuid,
}

#[derive(Serialize)]
pub struct RoleAssignmentPayload {
    pub assignment_id: Uuid,
    pub user_id: Uuid,
    pub role_id: Uuid,
    pub scope_department_id: Option<Uuid>,
}

#[derive(Debug, Clone, FromQueryResult)]
pub struct DepartmentRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    #[sea_orm(from_alias = "parentId")]
    pub parent_id: Option<Uuid>,
    pub depth: i32,
    pub path: String,
    #[sea_orm(from_alias = "budgetAllocated")]
    pub budget_allocated: Decimal,
    #[sea_orm(from_alias = "budgetPeriod")]
    pub budget_period: BudgetPeriod,
    #[sea_orm(from_alias = "actionOnExceed")]
    pub action_on_exceed: ActionOnExceed,
    #[sea_orm(from_alias = "retentionDays")]
    pub retention_days: Option<i32>,
    #[sea_orm(from_alias = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[sea_orm(from_alias = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, FromQueryResult)]
pub struct DepartmentTreeRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    #[sea_orm(from_alias = "parentId")]
    pub parent_id: Option<Uuid>,
    pub depth: i32,
    pub path: String,
    #[sea_orm(from_alias = "budgetAllocated")]
    pub budget_allocated: Decimal,
    #[sea_orm(from_alias = "budgetPeriod")]
    pub budget_period: BudgetPeriod,
    #[sea_orm(from_alias = "retentionDays")]
    pub retention_days: Option<i32>,
    #[sea_orm(from_alias = "createdAt")]
    pub created_at: DateTime<Utc>,
    #[sea_orm(from_alias = "updatedAt")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
pub struct DeptCountRow {
    #[sea_orm(from_alias = "departmentId")]
    pub department_id: Uuid,
    pub cnt: i64,
}

#[derive(Debug, FromQueryResult)]
pub struct ChildCountRow {
    #[sea_orm(from_alias = "parentId")]
    pub parent_id: Uuid,
    pub cnt: i64,
}
