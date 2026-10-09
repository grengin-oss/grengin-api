// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use llm_plugin::ProviderError;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, UpdateMany, prelude::Decimal,
    sea_query::Expr,
};
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

use crate::{
    dto::{chat_stream::ResponseStopped, models::ModelInfo},
    error::{AppError, ErrorDetailVariant, ErrorResponse},
    models::{departments::ActionOnExceed, messages, messages::StopReason},
    services::budget_allocation::{BudgetHealth, get_department_budget_status},
    state::SharedState,
};

const CONTENT_SAVE_INTERVAL: Duration = Duration::from_millis(500);

pub fn model_identifiers<'a>(
    requested: &'a str,
    provider_key: &str,
    plugin_model: Option<&'a ModelInfo>,
    catalog_model: Option<&'a ModelInfo>,
) -> Vec<&'a str> {
    let mut identifiers = vec![requested];
    for model in [plugin_model, catalog_model].into_iter().flatten() {
        if model.engine.eq_ignore_ascii_case(provider_key) {
            identifiers.push(model.key.as_str());
            identifiers.push(model.name.as_str());
        }
    }
    identifiers
}

// Mirrors GET /models: an engine with an empty whitelist exposes no models.
pub fn is_model_whitelisted(whitelist: &[String], identifiers: &[&str]) -> bool {
    identifiers
        .iter()
        .filter(|identifier| !identifier.is_empty())
        .any(|identifier| whitelist.iter().any(|entry| entry == identifier))
}

pub async fn ensure_model_whitelisted(
    app_state: &SharedState,
    provider: &str,
    provider_key: &str,
    model_name: &str,
    identifiers: &[&str],
) -> Result<(), AppError> {
    let whitelist = app_state
        .settings
        .get_ai_engine_whitelist(provider_key)
        .await
        .unwrap_or_default();
    if is_model_whitelisted(&whitelist, identifiers) {
        Ok(())
    } else {
        Err(AppError::DepartmentModelNotAllowed {
            provider: provider.to_string(),
            model: model_name.to_string(),
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum DepartmentBudgetGate {
    Open,
    Warn,
    Block,
}

pub fn department_budget_gate(
    allocated: Decimal,
    available: Decimal,
    action_on_exceed: &ActionOnExceed,
) -> DepartmentBudgetGate {
    if BudgetHealth::classify(allocated, available) != BudgetHealth::Exhausted {
        return DepartmentBudgetGate::Open;
    }
    match action_on_exceed {
        ActionOnExceed::Warn => DepartmentBudgetGate::Warn,
        ActionOnExceed::Block => DepartmentBudgetGate::Block,
    }
}

pub async fn department_budget_status(
    db: &DatabaseConnection,
    department_id: Uuid,
) -> Result<(DepartmentBudgetGate, Decimal), AppError> {
    let status = get_department_budget_status(db, department_id)
        .await
        .map_err(|e| {
            eprintln!("get department budget status error: {e}");
            AppError::DbTimeout
        })?;
    Ok((
        department_budget_gate(status.allocated, status.available, &status.action_on_exceed),
        status.available,
    ))
}

#[derive(Debug, PartialEq, Eq)]
pub enum ToolRoundOutcome {
    Continue,
    Finish,
    ReportCancel,
}

pub fn next_tool_round(
    cancelled: bool,
    oauth_required: bool,
    has_tool_results: bool,
) -> ToolRoundOutcome {
    if cancelled {
        ToolRoundOutcome::ReportCancel
    } else if oauth_required || !has_tool_results {
        ToolRoundOutcome::Finish
    } else {
        ToolRoundOutcome::Continue
    }
}

pub fn persistence_error_event_data() -> String {
    let (_, detail) = AppError::DbTimeout.to_detail();
    serde_json::to_string(&ErrorResponse {
        detail: ErrorDetailVariant::Rich(detail),
    })
    .unwrap_or_else(|_| "{}".to_string())
}

pub struct ContentCheckpoint {
    db: DatabaseConnection,
    message_id: Uuid,
    last_saved_at: Option<Instant>,
    unsaved: Option<String>,
    stop_reason_if_dropped: Option<StopReason>,
}

impl ContentCheckpoint {
    pub fn new(db: DatabaseConnection, message_id: Uuid) -> Self {
        Self {
            db,
            message_id,
            last_saved_at: None,
            unsaved: None,
            stop_reason_if_dropped: Some(StopReason::NetworkError),
        }
    }

    // The reply's final state was saved, including any stop reason, so dropping the stream
    // afterwards must not mark it as interrupted.
    pub fn finish(&mut self) {
        self.stop_reason_if_dropped = None;
    }

    pub fn fail_with(&mut self, reason: StopReason) {
        self.stop_reason_if_dropped = Some(reason);
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.last_saved_at
            .is_none_or(|last| now.saturating_duration_since(last) >= CONTENT_SAVE_INTERVAL)
    }

    pub fn saved(&mut self, now: Instant) {
        self.last_saved_at = Some(now);
        self.unsaved = None;
    }

    pub fn defer(&mut self, content: &str) {
        content.clone_into(self.unsaved.get_or_insert_default());
    }
}

// Covers streams dropped mid-response (client disconnect, or a failure that ended the stream
// early) between throttled saves: the latest text is kept and the reply is marked as stopped.
impl Drop for ContentCheckpoint {
    fn drop(&mut self) {
        let content = self.unsaved.take();
        let reason = self.stop_reason_if_dropped.take();
        if content.is_none() && reason.is_none() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let db = self.db.clone();
        let update = interrupted_reply_update(self.message_id, content, reason);
        runtime.spawn(async move {
            if let Err(error) = update.exec(&db).await {
                eprintln!("interrupted assistant reply save error: {error}");
            }
        });
    }
}

fn interrupted_reply_update(
    message_id: Uuid,
    content: Option<String>,
    reason: Option<StopReason>,
) -> UpdateMany<messages::Entity> {
    let mut update = messages::Entity::update_many()
        .col_expr(messages::Column::UpdatedAt, Expr::value(Utc::now()))
        .filter(messages::Column::Id.eq(message_id));
    if let Some(content) = content {
        update = update.col_expr(messages::Column::MessageContent, Expr::value(content));
    }
    if let Some(reason) = reason {
        // SeaQuery has no jsonb merge, and the other metadata keys must be kept.
        update = update.col_expr(
            messages::Column::Metadata,
            Expr::cust_with_values(
                r#"COALESCE("metadata", '{}'::jsonb) || jsonb_build_object('stopReason', $1::text)"#,
                [reason.as_str()],
            ),
        );
    }
    update
}

pub fn with_stop_reason(metadata: Option<Value>, reason: StopReason) -> Value {
    let mut map = match metadata {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    map.insert(
        StopReason::METADATA_KEY.to_string(),
        Value::String(reason.as_str().to_string()),
    );
    Value::Object(map)
}

// A provider that couldn't be reached or dropped the connection is a network problem;
// anything it answered with is the provider's.
pub fn stop_reason_for_provider_error(error: &ProviderError) -> StopReason {
    match error {
        ProviderError::Transport(_) | ProviderError::StreamEnded => StopReason::NetworkError,
        _ => StopReason::ProviderError,
    }
}

pub fn response_stopped_event_data(message_id: Uuid, reason: StopReason) -> String {
    serde_json::to_string(&ResponseStopped {
        message_id,
        stop_reason: reason,
    })
    .unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{dto::models::ModelType, state::StreamCancel};

    fn model(engine: &str, key: &str, name: &str) -> ModelInfo {
        ModelInfo {
            key: key.to_string(),
            name: name.to_string(),
            engine: engine.to_string(),
            model_type: ModelType::TextGenerator,
            comment: None,
            input_token_rate: None,
            output_token_rate: None,
            image_input_token_rate: None,
            image_cached_input_token_rate: None,
            image_output_token_rate: None,
            cached_input_token_rate: None,
            cache_creation_token_rate: None,
            max_input_tokens: None,
            max_output_tokens: None,
            supports_streaming: true,
            supports_tools: false,
            supports_reasoning: false,
            supports_vision: false,
            supports_audio: false,
            supports_pdf_native: false,
            supports_web_search: false,
            supports_multiple_images: false,
            max_images: None,
            dimensions: None,
            price_per_image: None,
        }
    }

    fn whitelist(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|entry| entry.to_string()).collect()
    }

    fn checkpoint() -> ContentCheckpoint {
        ContentCheckpoint::new(DatabaseConnection::Disconnected, Uuid::from_u128(5))
    }

    #[test]
    fn a_dropped_stream_is_marked_as_a_network_interruption_by_default() {
        let mut checkpoint = checkpoint();
        assert_eq!(
            checkpoint.stop_reason_if_dropped,
            Some(StopReason::NetworkError)
        );
        checkpoint.stop_reason_if_dropped = None;
    }

    #[test]
    fn a_finished_reply_is_not_marked_when_the_stream_is_dropped() {
        let mut checkpoint = checkpoint();
        checkpoint.finish();
        assert_eq!(checkpoint.stop_reason_if_dropped, None);
    }

    #[test]
    fn a_failure_that_ends_the_stream_keeps_its_own_reason() {
        let mut checkpoint = checkpoint();
        checkpoint.fail_with(StopReason::ServerError);
        assert_eq!(
            checkpoint.stop_reason_if_dropped,
            Some(StopReason::ServerError)
        );
        checkpoint.stop_reason_if_dropped = None;
    }

    #[test]
    fn interrupted_reply_update_keeps_other_metadata_and_saves_the_text() {
        use sea_orm::{DbBackend, QueryTrait};
        let sql = interrupted_reply_update(
            Uuid::from_u128(5),
            Some("partial answer".to_string()),
            Some(StopReason::NetworkError),
        )
        .build(DbBackend::Postgres)
        .to_string();

        assert!(
            sql.contains(r#"COALESCE("metadata", '{}'::jsonb) ||"#),
            "{sql}"
        );
        assert!(
            sql.contains("jsonb_build_object('stopReason', 'network_error'"),
            "{sql}"
        );
        assert!(sql.contains("'partial answer'"), "{sql}");
        assert!(
            sql.contains(&format!(r#""id" = '{}'"#, Uuid::from_u128(5))),
            "{sql}"
        );
    }

    #[test]
    fn stop_reason_is_merged_into_existing_metadata() {
        let metadata = serde_json::json!({"webSearch": true, "cancelled": true});
        let merged = with_stop_reason(Some(metadata), StopReason::UserEdited);
        assert_eq!(merged["webSearch"], true);
        assert_eq!(merged["cancelled"], true);
        assert_eq!(merged["stopReason"], "user_edited");
        assert_eq!(
            with_stop_reason(None, StopReason::ProviderError)["stopReason"],
            "provider_error"
        );
    }

    #[test]
    fn unreachable_or_dropped_providers_are_network_errors() {
        assert_eq!(
            stop_reason_for_provider_error(&ProviderError::StreamEnded),
            StopReason::NetworkError
        );
        assert_eq!(
            stop_reason_for_provider_error(&ProviderError::QuotaExhausted),
            StopReason::ProviderError
        );
        assert_eq!(
            stop_reason_for_provider_error(&ProviderError::PaymentRequired),
            StopReason::ProviderError
        );
    }

    #[test]
    fn response_stopped_event_names_the_message_and_the_reason() {
        let data = response_stopped_event_data(Uuid::from_u128(5), StopReason::UserCancelled);
        let value: Value = serde_json::from_str(&data).unwrap();
        assert_eq!(value["message_id"], Uuid::from_u128(5).to_string());
        assert_eq!(value["stop_reason"], "user_cancelled");
    }

    #[test]
    fn whitelisted_model_is_allowed() {
        assert!(is_model_whitelisted(&whitelist(&["gpt-4o"]), &["gpt-4o"]));
    }

    #[test]
    fn model_missing_from_whitelist_is_rejected() {
        assert!(!is_model_whitelisted(
            &whitelist(&["gpt-4o"]),
            &["gpt-4o-mini"]
        ));
    }

    #[test]
    fn empty_whitelist_rejects_every_model() {
        assert!(!is_model_whitelisted(&[], &["gpt-4o"]));
    }

    #[test]
    fn whitelist_match_is_case_sensitive_like_the_models_listing() {
        assert!(!is_model_whitelisted(&whitelist(&["gpt-4o"]), &["GPT-4o"]));
    }

    #[test]
    fn empty_identifier_never_matches_an_empty_whitelist_entry() {
        assert!(!is_model_whitelisted(&whitelist(&[""]), &[""]));
    }

    #[test]
    fn model_whitelisted_by_display_name_is_allowed_by_key() {
        let resolved = model("openai", "gpt-4o-2024", "GPT-4o");
        let identifiers = model_identifiers("gpt-4o-2024", "openai", Some(&resolved), None);

        assert!(is_model_whitelisted(&whitelist(&["GPT-4o"]), &identifiers));
    }

    #[test]
    fn catalog_model_from_another_engine_does_not_widen_the_whitelist() {
        let other_engine = model("azure", "shared-key", "Whitelisted Elsewhere");
        let identifiers = model_identifiers("shared-key", "openai", None, Some(&other_engine));

        assert_eq!(identifiers, vec!["shared-key"]);
        assert!(!is_model_whitelisted(
            &whitelist(&["Whitelisted Elsewhere"]),
            &identifiers
        ));
    }

    #[test]
    fn engine_comparison_ignores_case() {
        let resolved = model("OpenAI", "gpt-4o", "GPT-4o");
        let identifiers = model_identifiers("gpt-4o", "openai", None, Some(&resolved));

        assert_eq!(identifiers, vec!["gpt-4o", "gpt-4o", "GPT-4o"]);
    }

    #[test]
    fn department_without_budget_is_unlimited_even_with_block_action() {
        assert_eq!(
            department_budget_gate(Decimal::ZERO, Decimal::ZERO, &ActionOnExceed::Block),
            DepartmentBudgetGate::Open
        );
    }

    #[test]
    fn department_without_budget_gets_no_warning() {
        assert_eq!(
            department_budget_gate(Decimal::ZERO, Decimal::ZERO, &ActionOnExceed::Warn),
            DepartmentBudgetGate::Open
        );
    }

    #[test]
    fn negative_allocation_is_treated_as_no_budget() {
        assert_eq!(
            department_budget_gate(Decimal::NEGATIVE_ONE, Decimal::ZERO, &ActionOnExceed::Block),
            DepartmentBudgetGate::Open
        );
    }

    #[test]
    fn department_with_remaining_budget_is_open() {
        assert_eq!(
            department_budget_gate(
                Decimal::from(100),
                Decimal::new(1, 2),
                &ActionOnExceed::Block
            ),
            DepartmentBudgetGate::Open
        );
    }

    #[test]
    fn exhausted_budget_with_block_action_blocks() {
        assert_eq!(
            department_budget_gate(Decimal::from(100), Decimal::ZERO, &ActionOnExceed::Block),
            DepartmentBudgetGate::Block
        );
    }

    #[test]
    fn exhausted_budget_with_warn_action_warns() {
        assert_eq!(
            department_budget_gate(Decimal::from(100), Decimal::ZERO, &ActionOnExceed::Warn),
            DepartmentBudgetGate::Warn
        );
    }

    #[test]
    fn overspent_budget_counts_as_exhausted() {
        assert_eq!(
            department_budget_gate(
                Decimal::from(100),
                Decimal::NEGATIVE_ONE,
                &ActionOnExceed::Block
            ),
            DepartmentBudgetGate::Block
        );
    }

    #[test]
    fn tool_round_continues_with_results_when_not_cancelled() {
        assert_eq!(
            next_tool_round(false, false, true),
            ToolRoundOutcome::Continue
        );
    }

    #[test]
    fn cancel_during_tool_round_is_reported_instead_of_continuing() {
        assert_eq!(
            next_tool_round(true, false, true),
            ToolRoundOutcome::ReportCancel
        );
    }

    #[test]
    fn cancel_before_any_tool_ran_is_still_reported() {
        assert_eq!(
            next_tool_round(true, false, false),
            ToolRoundOutcome::ReportCancel
        );
    }

    #[test]
    fn cancel_wins_over_pending_oauth() {
        assert_eq!(
            next_tool_round(true, true, true),
            ToolRoundOutcome::ReportCancel
        );
    }

    #[test]
    fn oauth_prompt_finishes_the_tool_round() {
        assert_eq!(next_tool_round(false, true, true), ToolRoundOutcome::Finish);
    }

    #[test]
    fn tool_round_without_results_finishes() {
        assert_eq!(
            next_tool_round(false, false, false),
            ToolRoundOutcome::Finish
        );
    }

    #[tokio::test]
    async fn cancel_sent_while_nobody_waits_is_not_lost() {
        let handle = StreamCancel::new(Uuid::nil());
        handle.cancel(StopReason::UserCancelled);

        let result = tokio::time::timeout(Duration::from_millis(100), handle.cancelled()).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn waiting_stream_is_woken_by_cancel() {
        let handle = std::sync::Arc::new(StreamCancel::new(Uuid::nil()));
        let waiter = tokio::spawn({
            let handle = handle.clone();
            async move { handle.cancelled().await }
        });
        tokio::task::yield_now().await;
        handle.cancel(StopReason::UserCancelled);

        let result = tokio::time::timeout(Duration::from_secs(1), waiter).await;

        assert!(matches!(result, Ok(Ok(()))));
    }

    #[tokio::test]
    async fn uncancelled_stream_keeps_waiting() {
        let handle = StreamCancel::new(Uuid::nil());

        let result = tokio::time::timeout(Duration::from_millis(20), handle.cancelled()).await;

        assert!(result.is_err());
    }

    #[test]
    fn persistence_error_event_uses_db_timeout_code() {
        let payload: serde_json::Value =
            serde_json::from_str(&persistence_error_event_data()).expect("json payload");

        assert_eq!(payload["detail"]["type"], "rich");
        assert_eq!(payload["detail"]["code"], 5001);
    }

    #[test]
    fn first_content_chunk_is_saved_immediately() {
        let checkpoint = ContentCheckpoint::new(DatabaseConnection::default(), Uuid::new_v4());

        assert!(checkpoint.is_due(Instant::now()));
    }

    #[test]
    fn content_chunks_inside_the_interval_are_deferred() {
        let mut checkpoint = ContentCheckpoint::new(DatabaseConnection::default(), Uuid::new_v4());
        let start = Instant::now();
        checkpoint.saved(start);

        assert!(!checkpoint.is_due(start + Duration::from_millis(499)));
    }

    #[test]
    fn content_is_saved_again_once_the_interval_elapses() {
        let mut checkpoint = ContentCheckpoint::new(DatabaseConnection::default(), Uuid::new_v4());
        let start = Instant::now();
        checkpoint.saved(start);

        assert!(checkpoint.is_due(start + CONTENT_SAVE_INTERVAL));
    }

    #[test]
    fn deferred_content_keeps_only_the_latest_text() {
        let mut checkpoint = ContentCheckpoint::new(DatabaseConnection::default(), Uuid::new_v4());
        checkpoint.defer("Hel");
        checkpoint.defer("Hello");

        assert_eq!(checkpoint.unsaved.as_deref(), Some("Hello"));
    }

    #[test]
    fn a_save_clears_deferred_content() {
        let mut checkpoint = ContentCheckpoint::new(DatabaseConnection::default(), Uuid::new_v4());
        checkpoint.defer("Hello");
        checkpoint.saved(Instant::now());

        assert!(checkpoint.unsaved.is_none());
    }

    #[test]
    fn dropping_with_unsaved_content_outside_a_runtime_does_not_panic() {
        let mut checkpoint = ContentCheckpoint::new(DatabaseConnection::default(), Uuid::new_v4());
        checkpoint.defer("Hello");

        drop(checkpoint);
    }
}
