use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, IntoActiveModel, JoinType, QueryFilter, QueryOrder, QuerySelect, RelationTrait,
    Statement,
    sea_query::{Alias, BinOper, Expr, Order},
};
use serde_json::json;
use uuid::Uuid;

use crate::{
    config::setting::EmbeddingSettings,
    dto::embeddings::{GeminiEmbeddingResponse, MistralEmbeddingResponse},
    dto::files::File,
    error::AppError,
    llm::{
        gemini::GEMINI_API_URL,
        mistral::MISTRAL_API_URL,
        prompt::{Prompt, PromptTextResponse},
        provider::{AnthropicApis, OpenaiApis},
    },
    models::{conversation_summaries, message_embeddings, messages, messages::ChatRole},
    services::embedders_cache::get_model_dimensions,
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
    let boundary = match boundary {
        Some(boundary) => boundary,
        None => return Ok(None),
    };
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
    mut recent: Vec<Prompt>,
    mut current: Vec<Prompt>,
    max_tokens: usize,
) -> Vec<Prompt> {
    let mut prompts = Vec::new();
    if let Some(summary_prompt) = summary.clone() {
        prompts.push(summary_prompt);
    }
    if let Some(retrieval_prompt) = retrieval.clone() {
        prompts.push(retrieval_prompt);
    }
    prompts.append(&mut recent.clone());
    prompts.append(&mut current.clone());
    if estimate_tokens(&prompts) <= max_tokens {
        return prompts;
    }

    let mut prompts = Vec::new();
    if let Some(summary_prompt) = summary {
        prompts.push(summary_prompt);
    }
    prompts.append(&mut recent.clone());
    prompts.append(&mut current.clone());
    if estimate_tokens(&prompts) <= max_tokens {
        return prompts;
    }

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

    match provider.as_str() {
        "openai" => {
            let openai_settings = match app_state.settings.openai.read().await.clone() {
                Some(settings) if settings.is_enabled => settings,
                _ => return Ok(None),
            };
            let response = app_state
                .req_client
                .openai_create_embedding(&openai_settings, config.model.clone(), inputs, config.dimensions)
                .await
                .map_err(|e| {
                    eprintln!("embedding request error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "openai".to_string(),
                    }
                })?;
            let mut data = response.data;
            data.sort_by_key(|item| item.index);
            Ok(Some(
                data.into_iter()
                    .map(|item| normalize_to_target(item.embedding, target_dim))
                    .collect(),
            ))
        }
        "mistral" => {
            let mistral_settings = match app_state.settings.mistral.read().await.clone() {
                Some(settings) if settings.is_enabled => settings,
                _ => return Ok(None),
            };
            let response: MistralEmbeddingResponse = app_state
                .req_client
                .post(format!("{MISTRAL_API_URL}/v1/embeddings"))
                .bearer_auth(mistral_settings.api_key)
                .header("content-type", "application/json")
                .json(&json!({
                    "model": config.model,
                    "input": inputs,
                }))
                .send()
                .await
                .map_err(|e| {
                    eprintln!("mistral embedding request error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "mistral".to_string(),
                    }
                })?
                .error_for_status()
                .map_err(|e| {
                    eprintln!("mistral embedding status error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "mistral".to_string(),
                    }
                })?
                .json()
                .await
                .map_err(|e| {
                    eprintln!("mistral embedding decode error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "mistral".to_string(),
                    }
                })?;
            let mut data = response.data;
            data.sort_by_key(|item| item.index);
            Ok(Some(
                data.into_iter()
                    .map(|item| normalize_to_target(item.embedding, target_dim))
                    .collect(),
            ))
        }
        "gemini" => {
            let gemini_settings = match app_state.settings.gemini.read().await.clone() {
                Some(settings) if settings.is_enabled => settings,
                _ => return Ok(None),
            };
            let mut embeddings = Vec::with_capacity(inputs.len());
            for input in inputs {
                let mut body = serde_json::Map::new();
                body.insert(
                    "content".to_string(),
                    json!({
                        "parts": [{"text": input}]
                    }),
                );
                if let Some(dimensions) = config.dimensions {
                    body.insert("outputDimensionality".to_string(), json!(dimensions));
                }
                let response = app_state
                    .req_client
                    .post(format!(
                        "{GEMINI_API_URL}/v1beta/models/{}:embedContent",
                        config.model
                    ))
                    .header("x-goog-api-key", gemini_settings.api_key.clone())
                    .header("content-type", "application/json")
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| {
                        eprintln!("gemini embedding request error: {e}");
                        AppError::LlmProviderNotConfigured {
                            provider: "gemini".to_string(),
                        }
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let body = response.text().await.unwrap_or_default();
                    eprintln!("gemini embedding status error: {status} body: {body}");
                    return Err(AppError::LlmProviderNotConfigured {
                        provider: "gemini".to_string(),
                    });
                }

                let parsed: GeminiEmbeddingResponse = response.json().await.map_err(|e| {
                    eprintln!("gemini embedding decode error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "gemini".to_string(),
                    }
                })?;
                if let Some(embedding) = parsed.embedding {
                    embeddings.push(normalize_to_target(embedding.values, target_dim));
                }
            }
            if embeddings.is_empty() {
                return Ok(None);
            }
            Ok(Some(embeddings))
        }
        _ => Ok(None),
    }
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
    match provider.to_lowercase().as_str() {
        "openai" => {
            let openai_settings = app_state.settings.openai.read().await.clone().ok_or(
                AppError::LlmProviderNotConfigured {
                    provider: "openai".to_string(),
                },
            )?;
            let messages = vec![crate::dto::llm::openai::OpenaiMessage {
                role: ChatRole::User,
                content: vec![crate::dto::llm::openai::OpenaiContent {
                    content_type: crate::dto::llm::openai::OpenaiContentType::Text,
                    text: Some(prompt),
                    file_id: None,
                }],
            }];
            app_state
                .req_client
                .openai_generate_text(&openai_settings, model.to_string(), messages, None)
                .await
                .map_err(|e| {
                    eprintln!("summary openai error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "openai".to_string(),
                    }
                })
        }
        "anthropic" => {
            let anthropic_settings = app_state.settings.anthropic.read().await.clone().ok_or(
                AppError::LlmProviderNotConfigured {
                    provider: "anthropic".to_string(),
                },
            )?;
            let messages = vec![crate::dto::llm::anthropic::AnthropicMessage::from_text(
                crate::dto::llm::anthropic::AnthropicRole::User,
                prompt,
            )];
            app_state
                .req_client
                .anthropic_generate_text(
                    &anthropic_settings,
                    model.to_string(),
                    512,
                    messages,
                    None,
                    None,
                )
                .await
                .map_err(|e| {
                    eprintln!("summary anthropic error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "anthropic".to_string(),
                    }
                })
        }
        _ => Err(AppError::LlmProviderNotConfigured {
            provider: provider.to_string(),
        }),
    }
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
            eprintln!("embedding insert error: {e}");
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

    let embeddings = match generate_embeddings(app_state, &embedding_config, vec![query.to_string()]).await? {
        Some(v) if !v.is_empty() => v,
        _ => return Ok(None),
    };
    let query_vector = format_pgvector(&embeddings[0]);

    let sql = format!(
        r#"
        SELECT psc."content",
               ps."fileName"
        FROM "project_source_chunks" psc
        JOIN "project_sources" ps ON ps."id" = psc."projectSourceId"
        WHERE psc."projectId" = $1
          AND ps."processingStatus" = 'ready'
        ORDER BY psc."embedding" <=> '{}'::vector
        LIMIT $2
        "#,
        query_vector
    );

    let rows = app_state
        .database
        .query_all(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            &sql,
            vec![project_id.into(), (top_k as i64).into()],
        ))
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
        let content: String = row.try_get("", "content").unwrap_or_default();
        let file_name: String = row.try_get("", "fileName").unwrap_or_default();
        context.push_str(&format!("— {file_name}\n{content}\n\n"));
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
