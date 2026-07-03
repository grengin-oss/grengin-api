use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateRequest {
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub visibility: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUpdateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub visibility: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub search: Option<String>,
    pub category: Option<String>,
    pub visibility: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectResponse {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub category: String,
    pub visibility: String,
    pub owner_id: Uuid,
    pub chat_count: i64,
    pub source_count: i64,
    pub member_count: i64,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListResponse {
    pub projects: Vec<ProjectResponse>,
    pub total: u64,
    pub limit: u64,
    pub offset: u64,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSourceResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub file_name: String,
    pub file_type: String,
    pub file_size: i64,
    pub origin: String,
    pub uploaded_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactCreateRequest {
    pub title: String,
    pub content: String,
    pub content_type: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectChatResponse {
    pub id: Uuid,
    pub title: Option<String>,
    pub message_count: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDetailResponse {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub category: String,
    pub visibility: String,
    pub owner_id: Uuid,
    pub instructions: String,
    pub chat_count: i64,
    pub source_count: i64,
    pub member_count: i64,
    pub last_activity_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub sources: Vec<ProjectSourceResponse>,
    pub chats: Vec<ProjectChatResponse>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddMemberRequest {
    pub user_id: Uuid,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstructionsUpdateRequest {
    pub instructions: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AddSourceRequest {
    pub file_name: String,
    pub file_type: String,
    pub file_size: i64,
    pub origin: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ShareProjectResponse {
    pub share_url: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkProjectRequest {
    pub project_id: Uuid,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectMemberResponse {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: Option<String>,
    pub email: String,
    pub picture: Option<String>,
    pub role: String,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchItem {
    pub id: Uuid,
    pub name: Option<String>,
    pub email: String,
    pub picture: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserSearchResponse {
    pub users: Vec<UserSearchItem>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberSearchQuery {
    pub q: Option<String>,
    pub limit: Option<u64>,
}
