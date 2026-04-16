use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PromptSource {
    UserCustom,
    UserPrompt,
    Department,
    SystemDefault,
    None,
}

#[derive(Serialize, ToSchema)]
pub struct RolePromptResponse {
    pub id: Uuid,
    pub name: String,
    pub role_id: Uuid,
    pub prompt_text: String,
    pub variables: Option<Vec<String>>,
    pub is_system: bool,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub usage_count: i32,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct RolePromptCreate {
    pub name: String,
    pub role_id: Uuid,
    pub prompt_text: String,
    pub variables: Option<Vec<String>>,
    pub is_system: bool,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct RolePromptUpdate {
    pub name: Option<String>,
    pub role_id: Option<Uuid>,
    pub prompt_text: Option<String>,
    pub variables: Option<Vec<String>>,
    pub is_system: Option<bool>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct RolePromptListQuery {
    pub role_id: Option<Uuid>,
    pub is_system: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct DepartmentPromptAssignmentResponse {
    pub id: Uuid,
    pub department_id: Uuid,
    pub prompt_id: Uuid,
    pub priority: i32,
    pub assigned_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct DepartmentPromptAssignmentCreate {
    pub department_id: Uuid,
    pub prompt_id: Uuid,
    pub priority: i32,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct DepartmentPromptAssignmentUpdate {
    pub priority: i32,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct DepartmentPromptAssignmentListQuery {
    pub department_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct PromptMetricsResponse {
    pub prompt_id: Uuid,
    pub name: String,
    pub role_id: Uuid,
    pub usage_count: i32,
    pub feedback_count: i64,
    pub average_rating: Option<f64>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct PromptMetricsQuery {
    pub prompt_id: Option<Uuid>,
    pub role_id: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct SystemPromptResponse {
    pub prompt_text: Option<String>,
    pub prompt_id: Option<Uuid>,
    pub source: PromptSource,
    pub variables: Option<Vec<String>>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct UserPromptPreferenceRequest {
    pub prompt_id: Option<Uuid>,
    pub custom_prompt_text: Option<String>,
    pub is_active: Option<bool>,
}

#[derive(Deserialize, ToSchema, IntoParams)]
pub struct PromptFeedbackRequest {
    pub prompt_id: Option<Uuid>,
    pub rating: i32,
    pub comment: Option<String>,
}
