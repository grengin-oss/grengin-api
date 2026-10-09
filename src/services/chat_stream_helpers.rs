// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, prelude::Decimal, sea_query::Expr,
};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

use crate::{
    dto::models::ModelInfo,
    error::{AppError, ErrorDetailVariant, ErrorResponse},
    models::{departments::ActionOnExceed, messages},
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
}

impl ContentCheckpoint {
    pub fn new(db: DatabaseConnection, message_id: Uuid) -> Self {
        Self {
            db,
            message_id,
            last_saved_at: None,
            unsaved: None,
        }
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

// Covers streams dropped mid-response (client disconnect) between throttled saves.
impl Drop for ContentCheckpoint {
    fn drop(&mut self) {
        let Some(content) = self.unsaved.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let db = self.db.clone();
        let message_id = self.message_id;
        runtime.spawn(async move {
            let result = messages::Entity::update_many()
                .col_expr(messages::Column::MessageContent, Expr::value(content))
                .col_expr(messages::Column::UpdatedAt, Expr::value(Utc::now()))
                .filter(messages::Column::Id.eq(message_id))
                .exec(&db)
                .await;
            if let Err(error) = result {
                eprintln!("deferred assistant content save error: {error}");
            }
        });
    }
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
        let handle = StreamCancel::new();
        handle.cancel();

        let result = tokio::time::timeout(Duration::from_millis(100), handle.cancelled()).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn waiting_stream_is_woken_by_cancel() {
        let handle = std::sync::Arc::new(StreamCancel::new());
        let waiter = tokio::spawn({
            let handle = handle.clone();
            async move { handle.cancelled().await }
        });
        tokio::task::yield_now().await;
        handle.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), waiter).await;

        assert!(matches!(result, Ok(Ok(()))));
    }

    #[tokio::test]
    async fn uncancelled_stream_keeps_waiting() {
        let handle = StreamCancel::new();

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
