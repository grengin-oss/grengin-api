use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;
use crate::dto::admin_user::UserDetails;

#[derive(Serialize,IntoParams,ToSchema)]
pub struct DepartmentMembersResponse{
    pub total:i32,
    pub members:Vec<UserDetails>
}

#[derive(Serialize,IntoParams,ToSchema)]
pub struct DepartmentsListResponse {
    pub departments:Vec<DepartmentResponse>,
    pub total:i64,
}

#[derive(Deserialize,ToSchema)]
pub struct DepartmentListQuery {
    pub parent_id: Option<String>,
    #[serde(default)]
    pub include_children: bool,
}

#[derive(Deserialize,ToSchema)]
pub struct DepartmentMemeberListQuery {
    pub force: Option<bool>,
    #[serde(default)]
    pub include_sub_department: Option<bool>,
}

#[derive(Serialize,IntoParams,ToSchema)]
pub struct DepartmentResponse {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub parent_id: Option<Uuid>,
    pub path: String,
    pub depth: i32,
    pub leader_ids: Vec<Uuid>,
    pub member_count: i32,
    pub total_member_count: i32,
    pub child_count: i32,
    pub budget_allocated: f64,
    pub budget_distributed: f64,
    pub budget_available: f64,
    pub budget_used: f64,
    pub budget_period: BudgetPeriod,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize,IntoParams,ToSchema)]
pub struct DepartmentRequest {
    pub name: String,
    pub description: String,
    pub parent_id: Option<Uuid>,
    pub leader_ids: Vec<Uuid>,
}

#[derive(Deserialize,Serialize,ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum BudgetPeriod {
    Daily,
    Weekly,
    Monthly,
    Quarterly,
    Yearly,
}
