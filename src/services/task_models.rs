// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::HashSet, fmt};

use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use crate::{
    dto::models::{ModelInfo, ModelType},
    error::AppError,
    models::ai_engines,
    services::{
        chat_stream_helpers::{is_model_whitelisted, model_identifiers},
        department_policies::{effective_allowed_models, model_policy_key},
        llm_usage_gate::active_user_department,
        provider_models::resolve_model_info,
        provider_resolver::{model_route, resolve_provider},
    },
    state::SharedState,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCapability {
    Text,
    Vision,
    Document,
}

impl TaskCapability {
    pub fn for_content_type(content_type: &str) -> Self {
        if content_type.starts_with("image/") {
            Self::Vision
        } else {
            Self::Document
        }
    }

    fn satisfied_by(self, model: &ModelInfo) -> bool {
        model.model_type == ModelType::TextGenerator
            && match self {
                Self::Text => true,
                Self::Vision => model.supports_vision,
                Self::Document => model.supports_pdf_native,
            }
    }
}

impl fmt::Display for TaskCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Text => "text",
            Self::Vision => "vision",
            Self::Document => "document",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskModel {
    pub provider_key: String,
    pub model: String,
}

struct EngineOffer {
    engine_key: String,
    models: Vec<ModelInfo>,
}

type AllowedModels = HashSet<(String, String)>;

// Enabled engines in the order an admin added them; within an engine its default model first,
// then its whitelist order. The first model the department may use that has the capability wins.
pub async fn select_task_model(
    state: &SharedState,
    user_id: Uuid,
    capability: TaskCapability,
) -> Result<Option<TaskModel>, AppError> {
    select_task_model_with_gateway(state, user_id, capability, true).await
}

async fn select_task_model_with_gateway(
    state: &SharedState,
    user_id: Uuid,
    capability: TaskCapability,
    allow_gateway: bool,
) -> Result<Option<TaskModel>, AppError> {
    let allowed = department_allowed_models(state, user_id).await?;
    let engines = ai_engines::Entity::find()
        .filter(ai_engines::Column::IsEnabled.eq(true))
        .order_by_asc(ai_engines::Column::CreatedAt)
        .order_by_asc(ai_engines::Column::EngineKey)
        .all(&state.database)
        .await
        .map_err(|error| {
            eprintln!("task model engine query error: {error}");
            AppError::DbTimeout
        })?;
    let mut offers = Vec::with_capacity(engines.len());
    for engine in engines {
        let engine_key = engine.engine_key.to_lowercase();
        let Ok(provider) = resolve_provider(state, &engine_key).await else {
            continue;
        };
        if provider.chat().is_none() {
            continue;
        }
        let mut models = Vec::new();
        for model in candidate_order(&engine.default_model, &engine.whitelist_models) {
            if let Some(info) =
                resolve_model_info(state, provider.as_ref(), &engine_key, model).await
            {
                models.push(info);
            }
        }
        offers.push(EngineOffer { engine_key, models });
    }
    while let Some(choice) = pick_task_model(&offers, capability, allowed.as_ref()) {
        let routed = route_task_model(state, &choice).await?;
        if allow_gateway || routed.provider_key != "grengin" {
            return Ok(Some(routed));
        }
        for offer in &mut offers {
            if offer.engine_key == choice.provider_key {
                offer.models.retain(|model| model.key != choice.model);
            }
        }
    }
    Ok(None)
}

async fn route_task_model(state: &SharedState, choice: &TaskModel) -> Result<TaskModel, AppError> {
    let (provider_key, model, _) = model_route(state, &choice.provider_key, &choice.model).await?;
    Ok(TaskModel {
        provider_key,
        model,
    })
}

// The configured summary model (RAG_SUMMARY_LLM_*) or the conversation's own model, when it can
// write text; otherwise the workspace's first text model the user may use.
pub async fn select_summary_model(
    state: &SharedState,
    user_id: Uuid,
    conversation_provider: &str,
    conversation_model: &str,
) -> Result<Option<TaskModel>, AppError> {
    let rag = &state.settings.rag;
    let allowed = department_allowed_models(state, user_id).await?;
    let preferred = TaskModel {
        provider_key: rag
            .summary_llm_provider
            .as_deref()
            .unwrap_or(conversation_provider)
            .to_lowercase(),
        model: rag
            .summary_llm_model
            .clone()
            .unwrap_or_else(|| conversation_model.to_string()),
    };
    if let Ok(provider) = resolve_provider(state, &preferred.provider_key).await
        && provider.chat().is_some()
    {
        let info = resolve_model_info(
            state,
            provider.as_ref(),
            &preferred.provider_key,
            &preferred.model,
        )
        .await;
        let identifiers = model_identifiers(
            &preferred.model,
            &preferred.provider_key,
            info.as_ref(),
            None,
        );
        let whitelist = state
            .settings
            .get_ai_engine_whitelist(&preferred.provider_key)
            .await
            .unwrap_or_default();
        let department_allows = allowed.as_ref().is_none_or(|allowed| {
            identifiers
                .iter()
                .any(|id| allowed.contains(&model_policy_key(&preferred.provider_key, id)))
        });
        if writes_text(info.as_ref())
            && state
                .check_ai_engine_is_enabled(&preferred.provider_key)
                .await
                == Some(true)
            && is_model_whitelisted(&whitelist, &identifiers)
            && department_allows
        {
            let routed = route_task_model(state, &preferred).await?;
            if conversation_provider.eq_ignore_ascii_case("grengin")
                || routed.provider_key != "grengin"
                || rag
                    .summary_llm_provider
                    .as_deref()
                    .is_some_and(|provider| provider.eq_ignore_ascii_case("grengin"))
            {
                return Ok(Some(routed));
            }
        }
    }
    select_task_model_with_gateway(
        state,
        user_id,
        TaskCapability::Text,
        conversation_provider.eq_ignore_ascii_case("grengin"),
    )
    .await
}

async fn department_allowed_models(
    state: &SharedState,
    user_id: Uuid,
) -> Result<Option<AllowedModels>, AppError> {
    let Some(department_id) = active_user_department(state, user_id).await? else {
        return Ok(None);
    };
    let allowed = effective_allowed_models(&state.database, department_id)
        .await
        .map_err(|error| {
            eprintln!("task model department policy error: {error}");
            AppError::DbTimeout
        })?;
    Ok(allowed.map(|models| {
        models
            .into_iter()
            .map(|model| model_policy_key(&model.provider, &model.model))
            .collect()
    }))
}

// A model on a chat-capable engine that no catalog or plugin listing describes is still assumed to
// write text, as chat assumes; budgeted departments refuse it later for lack of pricing.
fn writes_text(info: Option<&ModelInfo>) -> bool {
    info.is_none_or(|info| TaskCapability::Text.satisfied_by(info))
}

fn candidate_order<'a>(default_model: &'a str, whitelist: &'a [String]) -> Vec<&'a str> {
    let mut order = Vec::with_capacity(whitelist.len());
    if whitelist.iter().any(|entry| entry == default_model) {
        order.push(default_model);
    }
    for entry in whitelist {
        if !order.contains(&entry.as_str()) {
            order.push(entry);
        }
    }
    order
}

fn pick_task_model(
    offers: &[EngineOffer],
    capability: TaskCapability,
    allowed: Option<&AllowedModels>,
) -> Option<TaskModel> {
    offers.iter().find_map(|offer| {
        offer
            .models
            .iter()
            .find(|model| {
                capability.satisfied_by(model) && is_allowed(allowed, &offer.engine_key, model)
            })
            .map(|model| TaskModel {
                provider_key: offer.engine_key.clone(),
                model: model.key.clone(),
            })
    })
}

fn is_allowed(allowed: Option<&AllowedModels>, engine_key: &str, model: &ModelInfo) -> bool {
    allowed.is_none_or(|allowed| {
        [&model.key, &model.name]
            .iter()
            .any(|identifier| allowed.contains(&model_policy_key(engine_key, identifier)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(engine: &str, key: &str, vision: bool, pdf: bool) -> ModelInfo {
        ModelInfo {
            key: key.to_string(),
            name: format!("{key} display"),
            engine: engine.to_string(),
            model_type: ModelType::TextGenerator,
            specialty: "general".into(),
            summaries: None,
            summary: None,
            price_tier: None,
            recommended: false,
            r#default: false,
            launch_date: None,
            comment: None,
            input_token_rate: Some(1.0),
            output_token_rate: Some(2.0),
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
            supports_vision: vision,
            supports_audio: false,
            supports_pdf_native: pdf,
            supports_web_search: false,
            supports_multiple_images: false,
            max_images: None,
            dimensions: None,
            price_per_image: None,
        }
    }

    fn offer(engine: &str, models: Vec<ModelInfo>) -> EngineOffer {
        EngineOffer {
            engine_key: engine.to_string(),
            models,
        }
    }

    fn choice(engine: &str, model: &str) -> Option<TaskModel> {
        Some(TaskModel {
            provider_key: engine.to_string(),
            model: model.to_string(),
        })
    }

    #[test]
    fn any_engine_with_the_capability_is_used_not_a_fixed_provider() {
        let offers = vec![
            offer("mistral", vec![model("mistral", "text-only", false, false)]),
            offer("acme-llm", vec![model("acme-llm", "seer", true, true)]),
        ];
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Vision, None),
            choice("acme-llm", "seer")
        );
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Text, None),
            choice("mistral", "text-only")
        );
    }

    #[test]
    fn capability_decides_between_vision_and_document_models() {
        let offers = vec![offer(
            "gemini",
            vec![
                model("gemini", "sees", true, false),
                model("gemini", "reads", false, true),
            ],
        )];
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Vision, None),
            choice("gemini", "sees")
        );
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Document, None),
            choice("gemini", "reads")
        );
        assert_eq!(
            TaskCapability::for_content_type("image/png"),
            TaskCapability::Vision
        );
        assert_eq!(
            TaskCapability::for_content_type("application/pdf"),
            TaskCapability::Document
        );
    }

    #[test]
    fn no_capable_model_means_no_choice() {
        let mut embedder = model("openai", "embed", true, true);
        embedder.model_type = ModelType::TextEmbedder;
        let offers = vec![offer(
            "openai",
            vec![embedder, model("openai", "plain", false, false)],
        )];
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Document, None),
            None
        );
        assert_eq!(pick_task_model(&[], TaskCapability::Text, None), None);
    }

    #[test]
    fn department_model_lists_are_respected() {
        let offers = vec![
            offer("first", vec![model("first", "a", true, true)]),
            offer("second", vec![model("second", "b", true, true)]),
        ];
        let allowed = AllowedModels::from([("second".to_string(), "b".to_string())]);
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Vision, Some(&allowed)),
            choice("second", "b")
        );
        let by_name = AllowedModels::from([("first".to_string(), "a display".to_string())]);
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Vision, Some(&by_name)),
            choice("first", "a")
        );
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Vision, Some(&AllowedModels::new())),
            None
        );
    }

    #[test]
    fn the_default_model_goes_first_only_when_whitelisted() {
        let whitelist = vec!["b".to_string(), "a".to_string(), "b".to_string()];
        assert_eq!(candidate_order("a", &whitelist), vec!["a", "b"]);
        assert_eq!(candidate_order("<empty>", &whitelist), vec!["b", "a"]);
        assert!(candidate_order("a", &[]).is_empty());
    }

    #[test]
    fn department_rules_match_the_same_model_across_funding_routes() {
        let allowed = AllowedModels::from([model_policy_key("openai", "gpt-5")]);
        let offers = vec![offer(
            "grengin",
            vec![model("grengin", "openai/gpt-5", true, true)],
        )];
        assert_eq!(
            pick_task_model(&offers, TaskCapability::Vision, Some(&allowed)),
            choice("grengin", "openai/gpt-5")
        );
        let forbidden = AllowedModels::from([model_policy_key("openai", "other")]);
        assert!(pick_task_model(&offers, TaskCapability::Vision, Some(&forbidden)).is_none());
    }

    #[test]
    fn summaries_keep_text_models_and_skip_image_generators() {
        assert!(writes_text(None));
        assert!(writes_text(Some(&model("openai", "chat", false, false))));
        let mut image = model("openai", "image", false, false);
        image.model_type = ModelType::ImageGenerator;
        assert!(!writes_text(Some(&image)));
    }
}
