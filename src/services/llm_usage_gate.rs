// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{fmt, time::Instant};

use chrono::Utc;
use llm_plugin::ProviderPlugin;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
use serde_json::json;
use uuid::Uuid;

use crate::{
    dto::models::ModelInfo,
    error::AppError,
    models::{
        usage_logs::{self, UsageLogStatus},
        users::{self, UserStatus},
    },
    services::{
        budget_allocation::refresh_department_budget_available,
        chat_stream_helpers::{
            DepartmentBudgetGate, department_budget_status, ensure_budget_pricing,
            ensure_model_whitelisted, model_identifiers,
        },
        department_policies::check_model_allowed,
        notifications::emit_budget_alerts,
        provider_models::resolve_model_info,
    },
    state::SharedState,
    utils::cost_calc::calculate_cost_decimal,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsagePurpose {
    TitleGeneration,
    SourceExtraction,
    ConversationSummary,
    MessageEmbedding,
    QueryEmbedding,
    SourceEmbedding,
}

impl UsagePurpose {
    // The embedding model is a workspace setting, not a model users pick, so department model
    // lists and engine chat whitelists don't name it; budgets still apply.
    fn is_embedding(self) -> bool {
        matches!(
            self,
            Self::MessageEmbedding | Self::QueryEmbedding | Self::SourceEmbedding
        )
    }
}

impl fmt::Display for UsagePurpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TitleGeneration => "title_generation",
            Self::SourceExtraction => "source_extraction",
            Self::ConversationSummary => "conversation_summary",
            Self::MessageEmbedding => "message_embedding",
            Self::QueryEmbedding => "query_embedding",
            Self::SourceEmbedding => "source_embedding",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsageSubject {
    pub user_id: Uuid,
    pub purpose: UsagePurpose,
    pub conversation_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ModelRates {
    pub input: Option<f64>,
    pub output: Option<f64>,
}

impl From<Option<&ModelInfo>> for ModelRates {
    fn from(model: Option<&ModelInfo>) -> Self {
        Self {
            input: model.and_then(|model| model.input_token_rate),
            output: model.and_then(|model| model.output_token_rate),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenCounts {
    pub input: i32,
    pub output: i32,
    pub estimated: bool,
}

impl TokenCounts {
    pub fn reported(input: i32, output: i32) -> Self {
        Self {
            input,
            output,
            estimated: false,
        }
    }
}

// Spend that never becomes a message (titles, extraction, summaries, embeddings) is charged here.
// Message spend reaches usage_logs only through the messages.cost trigger, so the two never overlap.
#[derive(Debug)]
pub struct UsageCharge {
    subject: UsageSubject,
    department_id: Option<Uuid>,
    provider_key: String,
    model: String,
    rates: ModelRates,
    started: Instant,
}

impl UsageCharge {
    // For calls the chat request already gated with the same user, budget and model checks.
    pub fn pre_authorized(
        subject: UsageSubject,
        department_id: Option<Uuid>,
        provider_key: &str,
        model: &str,
        rates: ModelRates,
    ) -> Self {
        Self {
            subject,
            department_id,
            provider_key: provider_key.to_lowercase(),
            model: model.to_string(),
            rates,
            started: Instant::now(),
        }
    }

    pub fn cost(&self, tokens: TokenCounts) -> rust_decimal::Decimal {
        calculate_cost_decimal(
            tokens.input,
            tokens.output,
            self.rates.input,
            self.rates.output,
        )
    }

    pub async fn record(self, state: &SharedState, tokens: TokenCounts) {
        let cost = self.cost(tokens);
        let id = Uuid::new_v4();
        let row = usage_logs::ActiveModel {
            id: Set(id),
            identifier: Set(id.to_string()),
            user_id: Set(self.subject.user_id),
            model_provider: Set(self.provider_key),
            model_name: Set(self.model),
            conversation_id: Set(self.subject.conversation_id),
            request_tokens: Set(i64::from(tokens.input)),
            response_tokens: Set(i64::from(tokens.output)),
            total_tokens: Set(i64::from(tokens.input) + i64::from(tokens.output)),
            cost_usd: Set(cost),
            latency_ms: Set(i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX)),
            timestamp: Set(Utc::now()),
            department: Set(self.department_id.map(|id| id.to_string())),
            status: Set(UsageLogStatus::Charged),
            error_message: Set(None),
            metadata: Set(Some(json!({
                "purpose": self.subject.purpose.to_string(),
                "estimatedTokens": tokens.estimated,
            }))),
        };
        if let Err(error) = row.insert(&state.database).await {
            eprintln!(
                "usage ledger write failed for {}: {error}",
                self.subject.purpose
            );
            return;
        }
        let Some(department_id) = self.department_id else {
            return;
        };
        if let Err(error) =
            refresh_department_budget_available(&state.database, department_id).await
        {
            eprintln!("refresh budget available error: {error}");
        } else if let Err(error) = emit_budget_alerts(state, department_id).await {
            eprintln!("emit budget alert error: {error:?}");
        }
    }
}

// Refusals a background caller can skip without failing the user's request.
pub fn is_usage_refusal(error: &AppError) -> bool {
    matches!(
        error,
        AppError::DepartmentBudgetExceeded
            | AppError::DepartmentModelNotAllowed { .. }
            | AppError::ValidationMissingField {
                field: "model_pricing"
            }
            | AppError::ResourceNotFound
    )
}

pub async fn authorize_usage(
    state: &SharedState,
    subject: UsageSubject,
    provider: &dyn ProviderPlugin,
    provider_key: &str,
    model: &str,
) -> Result<UsageCharge, AppError> {
    let provider_key = provider_key.to_lowercase();
    let check_model = !subject.purpose.is_embedding();
    let gate = check_user_policy(state, subject.user_id, &provider_key, model, check_model).await?;
    let info = resolve_model_info(state, provider, &provider_key, model).await;
    if check_model {
        ensure_model_whitelisted(
            state,
            &provider_key,
            &provider_key,
            model,
            &model_identifiers(model, &provider_key, info.as_ref(), None),
        )
        .await?;
    }
    ensure_budget_pricing(gate.budget_configured, info.as_ref())?;
    Ok(UsageCharge {
        subject,
        department_id: gate.department_id,
        provider_key,
        model: model.to_string(),
        rates: ModelRates::from(info.as_ref()),
        started: Instant::now(),
    })
}

// Background LLM work billed to a user must respect that user's status, department budget
// and department model policy, the same as their chat requests.
pub async fn ensure_user_llm_allowed(
    state: &SharedState,
    user_id: Uuid,
    provider: &str,
    model: &str,
) -> Result<(), AppError> {
    check_user_policy(state, user_id, provider, model, true)
        .await
        .map(|_| ())
}

pub async fn active_user_department(
    state: &SharedState,
    user_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    Ok(active_user(state, user_id).await?.department_id)
}

struct PolicyGate {
    department_id: Option<Uuid>,
    budget_configured: bool,
}

async fn active_user(state: &SharedState, user_id: Uuid) -> Result<users::Model, AppError> {
    let user = users::Entity::find_by_id(user_id)
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbTimeout)?
        .ok_or(AppError::ResourceNotFound)?;
    if user.status != UserStatus::Active {
        return Err(AppError::ResourceNotFound);
    }
    Ok(user)
}

async fn check_user_policy(
    state: &SharedState,
    user_id: Uuid,
    provider: &str,
    model: &str,
    check_model: bool,
) -> Result<PolicyGate, AppError> {
    let user = active_user(state, user_id).await?;
    let mut budget_configured = false;
    if let Some(department) = user.department_id {
        let (gate, _, configured) = department_budget_status(&state.database, department).await?;
        if gate == DepartmentBudgetGate::Block {
            return Err(AppError::DepartmentBudgetExceeded);
        }
        budget_configured = configured;
    }
    if check_model
        && !check_model_allowed(&state.database, user.department_id, provider, model)
            .await
            .map_err(|_| AppError::DbTimeout)?
    {
        return Err(AppError::DepartmentModelNotAllowed {
            provider: provider.to_string(),
            model: model.to_string(),
        });
    }
    Ok(PolicyGate {
        department_id: user.department_id,
        budget_configured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn charge(rates: ModelRates) -> UsageCharge {
        UsageCharge::pre_authorized(
            UsageSubject {
                user_id: Uuid::new_v4(),
                purpose: UsagePurpose::SourceExtraction,
                conversation_id: None,
            },
            None,
            "Custom",
            "model",
            rates,
        )
    }

    #[test]
    fn background_spend_is_priced_with_the_model_rates() {
        let rates = ModelRates {
            input: Some(1.0),
            output: Some(4.0),
        };
        assert_eq!(
            charge(rates).cost(TokenCounts::reported(1_000, 500)),
            rust_decimal::Decimal::from_str_exact("0.003").unwrap()
        );
        assert_eq!(charge(rates).provider_key, "custom");
    }

    #[test]
    fn embeddings_are_exempt_only_from_the_chat_model_lists() {
        assert!(UsagePurpose::QueryEmbedding.is_embedding());
        assert!(UsagePurpose::SourceEmbedding.is_embedding());
        assert!(UsagePurpose::MessageEmbedding.is_embedding());
        assert!(!UsagePurpose::ConversationSummary.is_embedding());
        assert!(!UsagePurpose::SourceExtraction.is_embedding());
        assert!(!UsagePurpose::TitleGeneration.is_embedding());
    }

    #[test]
    fn budget_and_policy_refusals_are_skippable_but_failures_are_not() {
        assert!(is_usage_refusal(&AppError::DepartmentBudgetExceeded));
        assert!(is_usage_refusal(&AppError::ValidationMissingField {
            field: "model_pricing"
        }));
        assert!(!is_usage_refusal(&AppError::DbTimeout));
        assert!(!is_usage_refusal(&AppError::LlmProviderNotConfigured {
            provider: "custom".into()
        }));
    }
}
