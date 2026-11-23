// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use llm_plugin::ProviderPlugin;
use rust_decimal::Decimal;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, sea_query::Expr};
use serde_json::json;
use uuid::Uuid;

use crate::{
    dto::conversation_titles::NewConversation,
    error::AppError,
    models::conversations,
    services::{
        chat_attachments::hydrate_files,
        llm_usage_gate::{ModelRates, TokenCounts, UsageCharge, UsagePurpose, UsageSubject},
        provider_chat::{generate_provider_title, provider_error_class},
    },
    state::SharedState,
};

pub async fn create_conversation(
    state: &SharedState,
    provider: &dyn ProviderPlugin,
    mut input: NewConversation,
) -> Result<(Uuid, Option<String>), AppError> {
    let id = Uuid::new_v4();
    let fallback: String = input.prompt.chars().take(80).collect();
    let charge = UsageCharge::pre_authorized(
        UsageSubject {
            user_id: input.user_id,
            purpose: UsagePurpose::TitleGeneration,
            conversation_id: Some(id),
        },
        input.department_id,
        &input.provider,
        &input.model,
        ModelRates {
            input: input.input_rate,
            output: input.output_rate,
        },
    );
    let response = if input.is_image_gen || input.via_gateway {
        None
    } else {
        if hydrate_files(&mut input.files, input.user_id, state)
            .await
            .is_err()
        {
            input.files.clear();
        }
        generate_provider_title(provider, input.model.clone(), input.prompt, input.files)
            .await
            .inspect_err(|error| {
                let class = provider_error_class(error);
                if class != "quota_exhausted" {
                    eprintln!("optional title generation failed: {class}");
                }
            })
            .ok()
    };
    input.metadata["titleGenerationUsage"] = json!({"model":null,"inputTokens":0,"outputTokens":0});
    let now = Utc::now();
    conversations::ActiveModel {
        id: Set(id),
        user_id: Set(input.user_id),
        title: Set(Some(fallback.clone())),
        model_provider: Set(input.provider),
        model_name: Set(input.model.clone()),
        created_at: Set(now),
        updated_at: Set(now),
        last_message_at: Set(Some(now)),
        archived_at: Set(None),
        pinned: Set(false),
        edit_requests: Set(0),
        edit_claim: Set(None),
        message_count: Set(0),
        total_tokens: Set(0),
        total_cost: Set(Decimal::ZERO),
        metadata: Set(Some(input.metadata.clone())),
    }
    .insert(&state.database)
    .await
    .map_err(|_| AppError::DbTimeout)?;

    let Some(response) = response else {
        return Ok((id, Some(fallback)));
    };
    let tokens = TokenCounts::for_text_reply(
        response.input_tokens,
        response.output_tokens,
        response.estimated_input_tokens,
        &response.title,
    );
    // Optional model output is discarded if its charge cannot be recorded; the user's chat stays.
    if charge.record(state, tokens).await.is_err() {
        return Ok((id, Some(fallback)));
    }
    input.metadata["titleGenerationUsage"] = json!({
        "model":input.model,"inputTokens":tokens.input,"outputTokens":tokens.output,
    });
    let published = conversations::Entity::update_many()
        .col_expr(
            conversations::Column::Title,
            Expr::value(response.title.clone()),
        )
        .col_expr(conversations::Column::Metadata, Expr::value(input.metadata))
        .filter(conversations::Column::Id.eq(id))
        .exec(&state.database)
        .await;
    match published {
        Ok(result) if result.rows_affected == 1 => Ok((id, Some(response.title))),
        _ => {
            eprintln!("optional title publication failed; retaining fallback title");
            Ok((id, Some(fallback)))
        }
    }
}
