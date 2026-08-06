// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::{department_prompt_assignments, role_prompts};
use chrono::{DateTime, Utc};
use sea_orm::FromQueryResult;
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

#[derive(serde::Deserialize, FromQueryResult)]
pub struct PromptMetricRow {
    #[sea_orm(from_alias = "promptId")]
    pub prompt_id: Uuid,
    #[sea_orm(from_alias = "name")]
    pub name: String,
    #[sea_orm(from_alias = "roleId")]
    pub role_id: Uuid,
    #[sea_orm(from_alias = "usageCount")]
    pub usage_count: i32,
    #[sea_orm(from_alias = "feedbackCount")]
    pub feedback_count: i64,
    #[sea_orm(from_alias = "averageRating")]
    pub average_rating: Option<f64>,
}

pub fn to_role_prompt_response(model: role_prompts::Model) -> RolePromptResponse {
    RolePromptResponse {
        id: model.id,
        name: model.name,
        role_id: model.role_id,
        prompt_text: model.prompt_text,
        variables: model
            .variables
            .as_ref()
            .and_then(|value| value.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|item| item.as_str().map(|s| s.to_string()))
                    .collect()
            }),
        is_system: model.is_system,
        created_by: model.created_by,
        created_at: model.created_at,
        updated_at: model.updated_at,
        usage_count: model.usage_count,
    }
}

pub fn to_assignment_response(
    model: department_prompt_assignments::Model,
) -> DepartmentPromptAssignmentResponse {
    DepartmentPromptAssignmentResponse {
        id: model.id,
        department_id: model.department_id,
        prompt_id: model.prompt_id,
        priority: model.priority,
        assigned_by: model.assigned_by,
        created_at: model.created_at,
        updated_at: model.updated_at,
    }
}
