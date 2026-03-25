use chrono::{DateTime, Utc};
use sea_orm::JsonValue;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Serialize, ToSchema, Clone)]
pub struct NotificationDto {
    pub id: Uuid,
    pub department_id: Option<Uuid>,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[schema(value_type = Object)]
    pub payload: JsonValue,
    pub period_start: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NotificationsListResponse {
    pub notifications: Vec<NotificationDto>,
    pub total: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct NotificationsListQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
    pub unread_only: Option<bool>,
    pub created_from: Option<DateTime<Utc>>,
    pub created_to: Option<DateTime<Utc>>,
}
