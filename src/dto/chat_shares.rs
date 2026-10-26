// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::models::{chat_shares::ShareAccess, messages::ChatRole};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareCreateRequest {
    pub access: Option<ShareAccess>,
    pub department_ids: Option<Vec<Uuid>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub notify: Option<bool>,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareUpdateRequest {
    pub access: Option<ShareAccess>,
    pub department_ids: Option<Vec<Uuid>>,
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub clear_expiry: bool,
    #[serde(default)]
    pub refresh_snapshot: bool,
    pub notify: Option<bool>,
}

#[derive(Serialize, ToSchema)]
pub struct ChatShareLink {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub share_url: String,
    pub access: ShareAccess,
    pub department_ids: Vec<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct SharedFile {
    pub id: Uuid,
    pub name: String,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct SharedArtifact {
    pub id: Uuid,
    pub file_id: Uuid,
    pub title: String,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct SharedMessage {
    pub id: Uuid,
    pub role: ChatRole,
    pub model_provider: String,
    pub model_name: String,
    pub created_at: DateTime<Utc>,
    pub text: String,
    pub files: Vec<SharedFile>,
    pub artifacts: Vec<SharedArtifact>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct SharedChatContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub model_provider: String,
    pub model_name: String,
    pub messages: Vec<SharedMessage>,
}

#[derive(Serialize, Deserialize)]
pub struct ShareSnapshot {
    pub schema_version: u32,
    pub chat: SharedChatContent,
    pub inline_files: HashMap<Uuid, String>,
}

#[derive(Serialize, ToSchema)]
pub struct SharedChatView {
    pub share_id: Uuid,
    pub access: ShareAccess,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    pub chat: SharedChatContent,
}

#[derive(Serialize, ToSchema)]
pub struct ForkedChat {
    pub conversation_id: Uuid,
}

#[derive(Serialize, ToSchema)]
pub struct SharedChatLink {
    pub share_id: Uuid,
    pub share_url: String,
}

#[derive(Serialize)]
pub struct ShareNotificationPayload {
    pub share_id: Uuid,
}
