// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::{
    dto::chat_stream::ChatInput, error::AppError, models::conversations, state::SharedState,
};

pub fn validate_turn_messages(input: &ChatInput) -> Result<(), AppError> {
    if input.messages.is_empty() {
        return Err(AppError::ValidationInvalidField { field: "messages" });
    }
    Ok(())
}

pub async fn delete_chat_owned(
    state: &SharedState,
    user_id: Uuid,
    chat_id: Uuid,
) -> Result<(), AppError> {
    let result = conversations::Entity::delete_many()
        .filter(conversations::Column::Id.eq(chat_id))
        .filter(conversations::Column::UserId.eq(user_id))
        .exec(&state.database)
        .await
        .map_err(|error| {
            eprintln!("chat delete error: {error}");
            AppError::DbTimeout
        })?;
    if result.rows_affected == 0 {
        return Err(AppError::DbNotFound);
    }
    state
        .stream_cancellations
        .read()
        .await
        .cancel_conversation(chat_id);
    Ok(())
}

pub fn resolve_web_search_enabled(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|value| value.get("webSearch"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub fn effective_native_web_search(requested: bool, model_supports_web_search: bool) -> bool {
    requested && model_supports_web_search
}

pub fn supports_native_web_search(
    plugin_model_support: Option<bool>,
    catalog_model_support: Option<bool>,
) -> bool {
    plugin_model_support
        .or(catalog_model_support)
        .unwrap_or(false)
}

pub fn effective_max_tokens(requested: Option<u32>, model_max: Option<u32>) -> Option<u32> {
    requested.map(|requested| model_max.map_or(requested, |model_max| requested.min(model_max)))
}

#[cfg(test)]
mod tests {
    use super::{effective_max_tokens, effective_native_web_search, supports_native_web_search};

    #[test]
    fn empty_turns_are_rejected() {
        let input = serde_json::from_value(serde_json::json!({
            "provider": "fixture", "model_name": "model", "messages": []
        }))
        .unwrap();
        assert!(matches!(
            super::validate_turn_messages(&input),
            Err(crate::error::AppError::ValidationInvalidField { field: "messages" })
        ));
    }

    #[test]
    fn text_and_attachment_only_turns_remain_valid() {
        for message in [
            serde_json::json!({"role": "user", "content": "question", "files": []}),
            serde_json::json!({"role": "user", "content": "", "files": [{
                "id": uuid::Uuid::new_v4(), "name": "image.png", "type": "image/png"
            }]}),
        ] {
            let input = serde_json::from_value(serde_json::json!({
                "provider": "fixture", "model_name": "model", "messages": [message]
            }))
            .unwrap();
            assert!(super::validate_turn_messages(&input).is_ok());
        }
    }

    #[test]
    fn unknown_models_never_gain_search_from_the_provider_name() {
        assert!(!supports_native_web_search(None, None));
    }

    #[test]
    fn explicit_runtime_model_support_wins_over_the_global_catalog() {
        assert!(!supports_native_web_search(Some(false), Some(true)));
        assert!(supports_native_web_search(Some(true), Some(false)));
        assert!(!supports_native_web_search(Some(false), None));
        assert!(supports_native_web_search(Some(true), None));
    }

    #[test]
    fn catalog_support_is_used_only_when_the_runtime_has_no_model_flag() {
        assert!(supports_native_web_search(None, Some(true)));
        assert!(!supports_native_web_search(None, Some(false)));
    }

    #[test]
    fn native_web_search_requires_request_and_model_support() {
        assert!(effective_native_web_search(true, true));
        assert!(!effective_native_web_search(true, false));
        assert!(!effective_native_web_search(false, true));
        assert!(!effective_native_web_search(false, false));
    }

    #[test]
    fn omitted_max_tokens_uses_the_provider_default() {
        assert_eq!(effective_max_tokens(None, Some(128_000)), None);
    }

    #[test]
    fn requested_max_tokens_is_capped_by_the_model_limit() {
        assert_eq!(
            effective_max_tokens(Some(2_048), Some(128_000)),
            Some(2_048)
        );
        assert_eq!(effective_max_tokens(Some(8_192), Some(4_096)), Some(4_096));
        assert_eq!(effective_max_tokens(Some(2_048), None), Some(2_048));
    }
}
