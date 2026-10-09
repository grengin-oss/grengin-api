// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize, ToSchema,
)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "lowercase"
)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
    System,
    Tool,
}

// Stored as `stopReason` in message metadata so the UI can explain an incomplete reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    UserCancelled,
    UserEdited,
    ProviderError,
    NetworkError,
    ServerError,
}

impl StopReason {
    pub const METADATA_KEY: &'static str = "stopReason";

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserCancelled => "user_cancelled",
            Self::UserEdited => "user_edited",
            Self::ProviderError => "provider_error",
            Self::NetworkError => "network_error",
            Self::ServerError => "server_error",
        }
    }

    // Replies cancelled before stop reasons existed only carry `cancelled: true`.
    pub fn from_metadata(metadata: Option<&serde_json::Value>) -> Option<Self> {
        let metadata = metadata?;
        if let Some(reason) = metadata.get(Self::METADATA_KEY).and_then(|v| v.as_str()) {
            return Self::try_from(reason.to_string()).ok();
        }
        metadata
            .get("cancelled")
            .and_then(|v| v.as_bool())
            .filter(|cancelled| *cancelled)
            .map(|_| Self::UserCancelled)
    }
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<String> for StopReason {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "user_cancelled" => Ok(Self::UserCancelled),
            "user_edited" => Ok(Self::UserEdited),
            "provider_error" => Ok(Self::ProviderError),
            "network_error" => Ok(Self::NetworkError),
            "server_error" => Ok(Self::ServerError),
            _ => Err(format!("unknown stop reason: {value}")),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "messages", rename_all = "camelCase")]
#[serde(rename_all = "camelCase")]
pub struct Model {
    #[sea_orm(primary_key, unique, indexed)]
    pub id: Uuid,
    pub conversation_id: Uuid,
    // Self refrence one to one
    #[sea_orm(nullable)]
    pub previous_message_id: Option<Uuid>,
    pub deleted: bool,
    pub role: ChatRole,
    pub message_content: String,
    pub model_provider: String,
    pub model_name: String,
    pub request_tokens: i32,
    pub response_tokens: i32,
    #[sea_orm(nullable)]
    pub request_id: Option<String>,
    pub tools_calls: Vec<serde_json::Value>,
    pub tools_results: Vec<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    // Total tokens used across all messages in this session.
    pub total_tokens: i32,
    // Latency in milliseconds
    pub latency: i32,
    // Cost in USD
    pub cost: Decimal,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::conversations::Entity",
        from = "Column::ConversationId",
        to = "super::conversations::Column::Id"
    )]
    Conversations,
    #[sea_orm(
        has_one = "super::messages::Entity",
        belongs_to = "super::messages::Entity",
        from = "Column::PreviousMessageId",
        to = "super::messages::Column::Id"
    )]
    Messages,
}

impl Related<super::conversations::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Conversations.def()
    }
}

impl Related<super::messages::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Messages.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::StopReason;
    use serde_json::json;

    #[test]
    fn stop_reasons_round_trip_through_their_stored_names() {
        for reason in [
            StopReason::UserCancelled,
            StopReason::UserEdited,
            StopReason::ProviderError,
            StopReason::NetworkError,
            StopReason::ServerError,
        ] {
            assert_eq!(StopReason::try_from(reason.to_string()), Ok(reason));
            assert_eq!(
                serde_json::to_value(reason).unwrap(),
                json!(reason.as_str())
            );
        }
        assert!(StopReason::try_from("finished".to_string()).is_err());
    }

    #[test]
    fn stop_reason_is_read_from_message_metadata() {
        let metadata = json!({"webSearch": false, "stopReason": "provider_error"});
        assert_eq!(
            StopReason::from_metadata(Some(&metadata)),
            Some(StopReason::ProviderError)
        );
    }

    #[test]
    fn legacy_cancelled_flag_reads_as_user_cancelled() {
        let metadata = json!({"cancelled": true});
        assert_eq!(
            StopReason::from_metadata(Some(&metadata)),
            Some(StopReason::UserCancelled)
        );
    }

    #[test]
    fn completed_replies_have_no_stop_reason() {
        assert_eq!(StopReason::from_metadata(None), None);
        assert_eq!(
            StopReason::from_metadata(Some(&json!({"cancelled": false}))),
            None
        );
        assert_eq!(
            StopReason::from_metadata(Some(&json!({"webSearch": true}))),
            None
        );
    }
}
