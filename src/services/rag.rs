// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, IntoActiveModel, JoinType, QueryFilter, QueryOrder, QuerySelect, RelationTrait,
    Statement,
    sea_query::{Alias, BinOper, Expr, Order},
};
use uuid::Uuid;

use crate::{
    config::setting::EmbeddingSettings,
    dto::files::File,
    dto::prompt::{Prompt, PromptTextResponse},
    error::AppError,
    models::{
        conversation_summaries, message_embeddings, messages, messages::ChatRole,
        project_source_chunks, project_sources,
    },
    services::{
        embedders_cache::get_model_dimensions,
        provider_chat::{generate_provider_text, provider_error_class},
        provider_resolver::resolve_provider,
    },
    state::SharedState,
};

pub struct RecentMessages {
    pub prompts: Vec<Prompt>,
    pub boundary: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct EmbeddingTarget {
    pub message_id: Uuid,
    pub conversation_id: Uuid,
    pub role: ChatRole,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, sea_orm::FromQueryResult)]
struct RetrievedMessageRow {
    #[sea_orm(from_alias = "messageContent")]
    message_content: String,
    #[sea_orm(from_alias = "role")]
    role: String,
}

#[derive(Debug, sea_orm::FromQueryResult)]
struct ProjectChunkRow {
    #[sea_orm(from_alias = "content")]
    content: String,
    #[sea_orm(from_alias = "fileName")]
    file_name: String,
}

pub async fn load_recent_prompts(
    db: &DatabaseConnection,
    conversation_id: Uuid,
    recent_pairs: usize,
) -> Result<RecentMessages, AppError> {
    if recent_pairs == 0 {
        return Ok(RecentMessages {
            prompts: Vec::new(),
            boundary: None,
        });
    }
    let limit = (recent_pairs * 2) as u64;
    let mut recent_messages = messages::Entity::find()
        .filter(messages::Column::ConversationId.eq(conversation_id))
        .filter(messages::Column::Deleted.eq(false))
        .filter(messages::Column::Role.is_in(vec![ChatRole::User, ChatRole::Assistant]))
        .order_by_desc(messages::Column::CreatedAt)
        .limit(limit)
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("rag recent messages query error: {e}");
            AppError::DbTimeout
        })?;

    let boundary = if recent_messages.len() < limit as usize {
        None
    } else {
        recent_messages.last().map(|message| message.created_at)
    };

    recent_messages.reverse();
    let prompts = recent_messages
        .into_iter()
        .map(message_to_prompt)
        .collect::<Vec<Prompt>>();

    Ok(RecentMessages { prompts, boundary })
}

pub async fn load_summary(
    db: &DatabaseConnection,
    conversation_id: Uuid,
) -> Result<Option<conversation_summaries::Model>, AppError> {
    conversation_summaries::Entity::find()
        .filter(conversation_summaries::Column::ConversationId.eq(conversation_id))
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("rag summary query error: {e}");
            AppError::DbTimeout
        })
}

pub async fn build_retrieval_prompt(
    app_state: &SharedState,
    conversation_id: Uuid,
    query_text: &str,
    boundary: Option<DateTime<Utc>>,
) -> Result<Option<String>, AppError> {
    if query_text.trim().is_empty() {
        return Ok(None);
    }
    if !app_state.settings.rag.enabled {
        return Ok(None);
    }
    if app_state.settings.rag.retrieval_top_k == 0 {
        return Ok(None);
    }
    let embedding_config = match app_state.settings.get_embedding_config().await {
        Some(config) if config.is_enabled => config,
        _ => return Ok(None),
    };
    let boundary = boundary.unwrap_or_else(Utc::now);
    let embedding = match generate_embedding(app_state, &embedding_config, query_text).await? {
        Some(embedding) => embedding,
        None => return Ok(None),
    };
    let vector = format_pgvector(&embedding);
    let distance_expr = Expr::col((
        message_embeddings::Entity,
        message_embeddings::Column::Embedding,
    ))
    .binary(
        BinOper::Custom("<=>".into()),
        Expr::val(vector).cast_as(Alias::new("vector")),
    );

    let rows = message_embeddings::Entity::find()
        .join(
            JoinType::InnerJoin,
            message_embeddings::Relation::Messages.def(),
        )
        .filter(message_embeddings::Column::ConversationId.eq(conversation_id))
        .filter(message_embeddings::Column::Provider.eq(embedding_config.provider.clone()))
        .filter(message_embeddings::Column::Model.eq(embedding_config.model.clone()))
        .filter(messages::Column::Deleted.eq(false))
        .filter(messages::Column::Role.is_in(vec![ChatRole::User, ChatRole::Assistant]))
        .filter(messages::Column::CreatedAt.lt(boundary))
        .select_only()
        .column_as(
            Expr::col((messages::Entity, messages::Column::MessageContent)),
            "messageContent",
        )
        .column_as(
            Expr::col((messages::Entity, messages::Column::Role)),
            "role",
        )
        .order_by(distance_expr, Order::Asc)
        .limit(app_state.settings.rag.retrieval_top_k as u64)
        .into_model::<RetrievedMessageRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("rag retrieval query error: {e}");
            AppError::DbTimeout
        })?;

    let messages = rows
        .into_iter()
        .map(|row| {
            let role_label = match row.role.as_str() {
                "assistant" => "Assistant",
                "user" => "User",
                _ => "User",
            };
            let snippet = truncate_text(&row.message_content, 500);
            format!("{role_label}: {snippet}")
        })
        .collect::<Vec<_>>();

    if messages.is_empty() {
        return Ok(None);
    }

    let text = format!(
        "Relevant previous messages (most similar):\n{}",
        messages.join("\n")
    );
    Ok(Some(text))
}

pub fn assemble_prompts_with_budget(
    summary: Option<Prompt>,
    retrieval: Option<Prompt>,
    project_retrieval: Option<Prompt>,
    mut recent: Vec<Prompt>,
    mut current: Vec<Prompt>,
    max_tokens: usize,
) -> Vec<Prompt> {
    // Tier 1: everything
    let mut prompts = Vec::new();
    if let Some(p) = summary.clone() {
        prompts.push(p);
    }
    if let Some(p) = retrieval.clone() {
        prompts.push(p);
    }
    if let Some(p) = project_retrieval.clone() {
        prompts.push(p);
    }
    prompts.extend(recent.clone());
    prompts.extend(current.clone());
    if estimate_tokens(&prompts) <= max_tokens {
        return prompts;
    }

    // Tier 2: drop conversation history (lowest value density)
    let mut prompts = Vec::new();
    if let Some(p) = summary.clone() {
        prompts.push(p);
    }
    if let Some(p) = project_retrieval.clone() {
        prompts.push(p);
    }
    prompts.extend(recent.clone());
    prompts.extend(current.clone());
    if estimate_tokens(&prompts) <= max_tokens {
        return prompts;
    }

    // Tier 3: drop project docs too
    let mut prompts = Vec::new();
    if let Some(p) = summary.clone() {
        prompts.push(p);
    }
    prompts.extend(recent.clone());
    prompts.extend(current.clone());
    if estimate_tokens(&prompts) <= max_tokens {
        return prompts;
    }

    // Tier 4: bare — recent + current only
    let mut prompts = Vec::new();
    prompts.append(&mut recent);
    prompts.append(&mut current);
    prompts
}

pub async fn embed_messages(
    app_state: &SharedState,
    targets: Vec<EmbeddingTarget>,
) -> Result<(), AppError> {
    if targets.is_empty() {
        return Ok(());
    }
    if !app_state.settings.rag.enabled {
        return Ok(());
    }
    let embedding_config = match app_state.settings.get_embedding_config().await {
        Some(config) if config.is_enabled => config,
        _ => return Ok(()),
    };

    let mut inputs = Vec::new();
    let mut filtered_targets = Vec::new();
    for target in targets {
        if target.content.trim().is_empty() {
            continue;
        }
        inputs.push(target.content.clone());
        filtered_targets.push(target);
    }
    if filtered_targets.is_empty() {
        return Ok(());
    }

    let embeddings = match generate_embeddings(app_state, &embedding_config, inputs).await {
        Err(e) => return Err(e),
        Ok(None) => return Ok(()),
        Ok(Some(v)) => v,
    };

    for (target, embedding) in filtered_targets.into_iter().zip(embeddings.into_iter()) {
        insert_message_embedding(app_state, &embedding_config, &target, &embedding).await?;
    }

    Ok(())
}

pub async fn update_conversation_summary(
    app_state: &SharedState,
    conversation_id: Uuid,
    provider: &str,
    model_name: &str,
) -> Result<(), AppError> {
    if !app_state.settings.rag.enabled {
        return Ok(());
    }
    let recent_pairs = app_state.settings.rag.recent_message_pairs;
    let recent = load_recent_prompts(&app_state.database, conversation_id, recent_pairs).await?;
    let boundary = match recent.boundary {
        Some(boundary) => boundary,
        None => return Ok(()),
    };

    let summary_provider = app_state
        .settings
        .rag
        .summary_llm_provider
        .clone()
        .unwrap_or_else(|| provider.to_string())
        .to_lowercase();
    let summary_model = app_state
        .settings
        .rag
        .summary_llm_model
        .clone()
        .unwrap_or_else(|| model_name.to_string());
    if summary_provider != "openai" && summary_provider != "anthropic" {
        return Ok(());
    }

    let existing_summary = load_summary(&app_state.database, conversation_id).await?;
    let mut condition = Condition::all()
        .add(messages::Column::ConversationId.eq(conversation_id))
        .add(messages::Column::Deleted.eq(false))
        .add(messages::Column::Role.is_in(vec![ChatRole::User, ChatRole::Assistant]))
        .add(messages::Column::CreatedAt.lt(boundary));
    if let Some(last_message_at) = existing_summary.as_ref().and_then(|s| s.last_message_at) {
        condition = condition.add(messages::Column::CreatedAt.gt(last_message_at));
    }

    let mut new_messages = messages::Entity::find()
        .filter(condition)
        .order_by_asc(messages::Column::CreatedAt)
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("rag summary messages query error: {e}");
            AppError::DbTimeout
        })?;

    if new_messages.is_empty() {
        return Ok(());
    }

    let last_message = new_messages.last().cloned();
    let new_chunk = new_messages
        .drain(..)
        .map(|message| {
            let role = match message.role {
                ChatRole::Assistant => "Assistant",
                _ => "User",
            };
            format!("{role}: {}", truncate_text(&message.message_content, 800))
        })
        .collect::<Vec<String>>()
        .join("\n");

    let existing_text = existing_summary
        .as_ref()
        .map(|summary| summary.summary.clone())
        .unwrap_or_default();
    let summary_prompt = format!(
        "You are a summarization assistant. Update the conversation summary.\n\nCurrent summary:\n{}\n\nNew messages:\n{}\n\nReturn an updated summary that captures key topics, decisions, facts, names, numbers, user preferences, and open tasks. Keep it concise.",
        existing_text, new_chunk
    );

    let summary_response =
        generate_summary_text(app_state, &summary_provider, &summary_model, summary_prompt).await?;
    let summary_text = summary_response.text.trim().to_string();

    let now = Utc::now();
    if let Some(existing) = existing_summary {
        let mut active = existing.into_active_model();
        active.summary = sea_orm::ActiveValue::Set(summary_text);
        if let Some(last) = last_message {
            active.last_message_at = sea_orm::ActiveValue::Set(Some(last.created_at));
            active.last_message_id = sea_orm::ActiveValue::Set(Some(last.id));
        }
        active.updated_at = sea_orm::ActiveValue::Set(now);
        active.update(&app_state.database).await.map_err(|e| {
            eprintln!("rag summary update error: {e}");
            AppError::DbTimeout
        })?;
    } else {
        let (last_message_at, last_message_id) = last_message
            .as_ref()
            .map(|message| (Some(message.created_at), Some(message.id)))
            .unwrap_or((None, None));
        let summary = conversation_summaries::ActiveModel {
            id: sea_orm::ActiveValue::Set(Uuid::new_v4()),
            conversation_id: sea_orm::ActiveValue::Set(conversation_id),
            summary: sea_orm::ActiveValue::Set(summary_text),
            last_message_at: sea_orm::ActiveValue::Set(last_message_at),
            last_message_id: sea_orm::ActiveValue::Set(last_message_id),
            created_at: sea_orm::ActiveValue::Set(now),
            updated_at: sea_orm::ActiveValue::Set(now),
        };
        summary.insert(&app_state.database).await.map_err(|e| {
            eprintln!("rag summary insert error: {e}");
            AppError::DbTimeout
        })?;
    }

    Ok(())
}

fn message_to_prompt(message: messages::Model) -> Prompt {
    let files = message
        .metadata
        .as_ref()
        .and_then(|json| json.get("files").cloned())
        .and_then(|files_val| serde_json::from_value::<Vec<File>>(files_val).ok())
        .unwrap_or_default();
    Prompt {
        text: message.message_content,
        role: message.role,
        files,
    }
}

fn estimate_tokens(prompts: &[Prompt]) -> usize {
    prompts
        .iter()
        .map(|prompt| estimate_tokens_for_text(&prompt.text))
        .sum()
}

fn estimate_tokens_for_text(text: &str) -> usize {
    let chars = text.chars().count();
    (chars + 3) / 4
}

fn truncate_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}…")
}

async fn generate_embedding(
    app_state: &SharedState,
    config: &EmbeddingSettings,
    text: &str,
) -> Result<Option<Vec<f32>>, AppError> {
    let embeddings = generate_embeddings(app_state, config, vec![text.to_string()]).await?;
    Ok(embeddings.and_then(|mut vecs| vecs.pop()))
}

pub async fn generate_embeddings(
    app_state: &SharedState,
    config: &EmbeddingSettings,
    inputs: Vec<String>,
) -> Result<Option<Vec<Vec<f32>>>, AppError> {
    let provider = config.provider.to_lowercase();
    let target_dim = match config.dimensions {
        Some(d) => Some(d as usize),
        None => get_model_dimensions(&app_state.req_client, &provider, &config.model).await,
    };

    let plugin = match resolve_provider(app_state, &provider).await {
        Ok(plugin) => plugin,
        Err(_) => return Ok(None),
    };
    let Some(embedder) = plugin.embeddings() else {
        return Ok(None);
    };
    let response = embedder
        .embed(llm_plugin::EmbeddingRequest {
            model: llm_plugin::ModelId::new(config.model.clone()),
            inputs,
            dimensions: config
                .dimensions
                .and_then(|value| u32::try_from(value).ok()),
            options: serde_json::Value::Null,
        })
        .await
        .map_err(|error| {
            eprintln!(
                "provider embedding failed: {}",
                provider_error_class(&error)
            );
            AppError::LlmProviderNotConfigured {
                provider: provider.clone(),
            }
        })?;
    Ok(Some(
        response
            .vectors
            .into_iter()
            .map(|embedding| normalize_to_target(embedding, target_dim))
            .collect(),
    ))
}

fn normalize_to_target(mut embedding: Vec<f32>, target_dim: Option<usize>) -> Vec<f32> {
    let Some(target) = target_dim else {
        return embedding;
    };
    match embedding.len().cmp(&target) {
        std::cmp::Ordering::Equal => embedding,
        std::cmp::Ordering::Greater => {
            embedding.truncate(target);
            embedding
        }
        std::cmp::Ordering::Less => {
            embedding.resize(target, 0.0);
            embedding
        }
    }
}

async fn generate_summary_text(
    app_state: &SharedState,
    provider: &str,
    model: &str,
    prompt: String,
) -> Result<PromptTextResponse, AppError> {
    let provider_key = provider.to_lowercase();
    let plugin = resolve_provider(app_state, &provider_key)
        .await
        .map_err(|_| AppError::LlmProviderNotConfigured {
            provider: provider.to_string(),
        })?;
    generate_provider_text(plugin.as_ref(), model, None, prompt, Some(512))
        .await
        .map_err(|error| {
            eprintln!("summary provider error: {}", provider_error_class(&error));
            AppError::LlmProviderNotConfigured {
                provider: provider.to_string(),
            }
        })
}

async fn insert_message_embedding(
    app_state: &SharedState,
    config: &EmbeddingSettings,
    target: &EmbeddingTarget,
    embedding: &[f32],
) -> Result<(), AppError> {
    let vector = format_pgvector(embedding);
    let sql = r#"
        INSERT INTO "message_embeddings"
            ("id", "messageId", "conversationId", "role", "provider", "model", "dimensions", "embedding", "createdAt")
        VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8::vector, $9)
        ON CONFLICT ("messageId", "provider", "model")
        DO NOTHING
    "#;
    let values = vec![
        Uuid::new_v4().into(),
        target.message_id.into(),
        target.conversation_id.into(),
        target.role.to_string().into(),
        config.provider.clone().into(),
        config.model.clone().into(),
        Some(embedding.len() as i32).into(),
        vector.into(),
        target.created_at.into(),
    ];
    app_state
        .database
        .execute(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            values,
        ))
        .await
        .map_err(|e| {
            // FK violation (23503) means the message was deleted before this
            // background task ran — expected race, not worth logging.
            if !e.to_string().contains("foreign key") {
                eprintln!("embedding insert error: {e}");
            }
            AppError::DbTimeout
        })?;
    Ok(())
}

pub fn format_pgvector(values: &[f32]) -> String {
    let mut out = String::from("[");
    for (idx, value) in values.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&format!("{:.6}", value));
    }
    out.push(']');
    out
}

pub async fn build_project_retrieval_prompt(
    app_state: &SharedState,
    project_id: uuid::Uuid,
    query: &str,
    top_k: usize,
) -> Result<Option<String>, AppError> {
    if !app_state.settings.rag.enabled {
        return Ok(None);
    }
    let embedding_config = match app_state.settings.get_embedding_config().await {
        Some(c) if c.is_enabled => c,
        _ => return Ok(None),
    };

    let embeddings =
        match generate_embeddings(app_state, &embedding_config, vec![query.to_string()]).await? {
            Some(v) if !v.is_empty() => v,
            _ => return Ok(None),
        };
    let query_vector = format_pgvector(&embeddings[0]);

    // <=> cosine operator and ::vector cast have no SeaORM integration.
    let distance_expr = Expr::col((
        project_source_chunks::Entity,
        project_source_chunks::Column::Embedding,
    ))
    .binary(
        BinOper::Custom("<=>".into()),
        Expr::val(query_vector).cast_as(Alias::new("vector")),
    );

    let rows = project_source_chunks::Entity::find()
        .join(
            JoinType::InnerJoin,
            project_source_chunks::Relation::ProjectSource.def(),
        )
        .filter(project_source_chunks::Column::ProjectId.eq(project_id))
        .filter(project_source_chunks::Column::Provider.eq(embedding_config.provider.clone()))
        .filter(project_source_chunks::Column::Model.eq(embedding_config.model.clone()))
        .filter(project_sources::Column::ProcessingStatus.eq("ready"))
        .select_only()
        .column_as(
            Expr::col((
                project_source_chunks::Entity,
                project_source_chunks::Column::Content,
            )),
            "content",
        )
        .column_as(
            Expr::col((project_sources::Entity, project_sources::Column::FileName)),
            "fileName",
        )
        .order_by(distance_expr, Order::Asc)
        .limit(top_k as u64)
        .into_model::<ProjectChunkRow>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("project chunk retrieval error: {e}");
            AppError::DbTimeout
        })?;

    if rows.is_empty() {
        return Ok(None);
    }

    let mut context = String::from("Relevant project documents:\n\n");
    for row in rows {
        context.push_str(&format!("— {}\n{}\n\n", row.file_name, row.content));
    }

    Ok(Some(context.trim_end().to_string()))
}

impl ChatRole {
    fn to_string(&self) -> String {
        match self {
            ChatRole::User => "user".to_string(),
            ChatRole::Assistant => "assistant".to_string(),
            ChatRole::System => "system".to_string(),
            ChatRole::Tool => "tool".to_string(),
        }
    }
}
