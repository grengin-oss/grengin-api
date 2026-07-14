use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Serialize, ToSchema)]
pub struct ArtifactResponse {
    pub id: Uuid,
    pub file_id: Uuid,
    pub message_id: Uuid,
    pub conversation_id: Uuid,
    pub title: String,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize, ToSchema)]
pub struct ArtifactListResponse {
    pub artifacts: Vec<ArtifactResponse>,
}
