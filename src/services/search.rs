use sea_orm::sea_query::{Alias, BinOper, Expr, Func, Order};
use sea_orm::{
    ColumnTrait, DatabaseBackend, EntityTrait, FromQueryResult, JoinType,
    QueryFilter, QueryOrder, QuerySelect, RelationTrait, Statement,
};
use serde_json::json;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    config::setting::EmbeddingSettings,
    dto::embeddings::{GeminiEmbeddingResponse, MistralEmbeddingResponse},
    error::AppError,
    llm::provider::OpenaiApis,
    llm::{gemini::GEMINI_API_URL, mistral::MISTRAL_API_URL},
    models::{conversations, message_embeddings, messages, messages::ChatRole},
    services::embedders_cache::get_model_dimensions,
    state::SharedState,
};

pub struct LexicalSearchPage {
    pub conversation_ids: Vec<Uuid>,
    pub total: u64,
    pub snippets: HashMap<Uuid, String>,
}

pub struct SemanticConversationPage {
    pub conversation_ids: Vec<Uuid>,
    pub total: u64,
    pub snippets: HashMap<Uuid, SemanticSnippet>,
}

pub struct SemanticSnippet {
    pub message_id: Uuid,
    pub snippet: String,
    pub distance: f64,
}

#[derive(Debug, FromQueryResult)]
struct SemanticConversationRow {
    #[sea_orm(from_alias = "conversationId")]
    conversation_id: Uuid,
    #[allow(dead_code)]
    #[sea_orm(from_alias = "distance")]
    distance: f64,
}

#[derive(Debug, FromQueryResult)]
struct SemanticSnippetRow {
    #[sea_orm(from_alias = "conversationId")]
    conversation_id: Uuid,
    #[sea_orm(from_alias = "messageId")]
    message_id: Uuid,
    #[sea_orm(from_alias = "snippet")]
    snippet: String,
    #[sea_orm(from_alias = "distance")]
    distance: f64,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    #[sea_orm(from_alias = "count")]
    count: i64,
}

#[derive(Debug, FromQueryResult)]
struct LexicalRow {
    #[sea_orm(from_alias = "conversationId")]
    conversation_id: Uuid,
    #[sea_orm(from_alias = "snippet")]
    snippet: String,
    #[allow(dead_code)]
    #[sea_orm(from_alias = "rank")]
    rank: f32,
}

// Splits a natural-language query into OR-joined keywords for websearch_to_tsquery.
// plainto_tsquery uses AND semantics, causing 0 results when query words span multiple messages.
fn build_fts_or_query(raw: &str) -> String {
    let terms: Vec<String> = raw
        .split(|c: char| !c.is_alphabetic())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 2)
        .collect();
    if terms.is_empty() {
        raw.trim().to_string()
    } else {
        terms.join(" or ")
    }
}

// to_tsvector / @@ / websearch_to_tsquery / ts_rank / DISTINCT ON are not expressible in SeaORM.
pub async fn lexical_conversation_search(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    query: &str,
    archived: bool,
    limit: u64,
    offset: u64,
) -> Result<LexicalSearchPage, AppError> {
    let archived_filter = if archived {
        r#"AND c."archivedAt" IS NOT NULL"#
    } else {
        r#"AND c."archivedAt" IS NULL"#
    };

    let fts_query = build_fts_or_query(query);
    let count_sql = format!(
        r#"
        SELECT COUNT(DISTINCT c."id") as count
        FROM "conversations" c
        INNER JOIN "messages" m ON m."conversationId" = c."id"
            AND m."deleted" = false
            AND m."role" IN ('user', 'assistant')
        WHERE c."userId" = $1
          AND to_tsvector('english', coalesce(c."title", '') || ' ' || m."messageContent")
              @@ websearch_to_tsquery('english', $2)
          {archived_filter}
        "#
    );

    let count_row = CountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        &count_sql,
        vec![user_id.into(), fts_query.clone().into()],
    ))
    .one(db)
    .await
    .map_err(|e| {
        eprintln!("lexical search count error: {e}");
        AppError::DbTimeout
    })?;

    let total = count_row.map(|r| r.count).unwrap_or(0);
    if total == 0 {
        return Ok(LexicalSearchPage {
            conversation_ids: Vec::new(),
            total: 0,
            snippets: HashMap::new(),
        });
    }

    let data_sql = format!(
        r#"
        SELECT "conversationId", "snippet", "rank" FROM (
            SELECT DISTINCT ON (c."id")
                c."id"                       AS "conversationId",
                left(m."messageContent", 240) AS "snippet",
                ts_rank(
                    to_tsvector('english', coalesce(c."title", '') || ' ' || m."messageContent"),
                    websearch_to_tsquery('english', $1)
                )                            AS "rank"
            FROM "conversations" c
            INNER JOIN "messages" m ON m."conversationId" = c."id"
                AND m."deleted" = false
                AND m."role" IN ('user', 'assistant')
            WHERE c."userId" = $2
              AND to_tsvector('english', coalesce(c."title", '') || ' ' || m."messageContent")
                  @@ websearch_to_tsquery('english', $1)
              {archived_filter}
            ORDER BY c."id", "rank" DESC
        ) sub
        ORDER BY "rank" DESC
        LIMIT $3 OFFSET $4
        "#
    );

    let rows = LexicalRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        &data_sql,
        vec![
            fts_query.into(),
            user_id.into(),
            (limit as i64).into(),
            (offset as i64).into(),
        ],
    ))
    .all(db)
    .await
    .map_err(|e| {
        eprintln!("lexical search query error: {e}");
        AppError::DbTimeout
    })?;

    let mut conversation_ids = Vec::with_capacity(rows.len());
    let mut snippets = HashMap::new();
    for row in rows {
        snippets.insert(row.conversation_id, truncate_snippet(&row.snippet));
        conversation_ids.push(row.conversation_id);
    }

    Ok(LexicalSearchPage {
        conversation_ids,
        total: total as u64,
        snippets,
    })
}

pub async fn semantic_conversation_search(
    app_state: &SharedState,
    user_id: Uuid,
    query_text: &str,
    archived: bool,
    limit: u64,
    offset: u64,
) -> Result<Option<SemanticConversationPage>, AppError> {
    if query_text.trim().is_empty() {
        return Ok(None);
    }
    if !app_state.settings.rag.enabled {
        return Ok(None);
    }
    let embedding_config = match app_state.settings.get_embedding_config().await {
        Some(config) if config.is_enabled => config,
        _ => return Ok(None),
    };
    let embedding =
        match generate_search_embedding(app_state, &embedding_config, query_text).await? {
            Some(embedding) => embedding,
            None => return Ok(None),
        };

    let total =
        load_total_matches(&app_state.database, user_id, archived, &embedding_config).await?;

    if total == 0 {
        return Ok(Some(SemanticConversationPage {
            conversation_ids: Vec::new(),
            total: 0,
            snippets: HashMap::new(),
        }));
    }

    let vector = format_pgvector(&embedding);
    let distance_expr = Expr::col(message_embeddings::Column::Embedding).binary(
        BinOper::Custom("<=>".into()),
        Expr::val(vector).cast_as(Alias::new("vector")),
    );
    let distance_min: sea_orm::sea_query::SimpleExpr = Func::min(distance_expr.clone()).into();

    let rows: Vec<SemanticConversationRow> =
        build_semantic_base_query(user_id, archived, &embedding_config)
            .select_only()
            .column_as(
                Expr::col((
                    message_embeddings::Entity,
                    message_embeddings::Column::ConversationId,
                )),
                "conversationId",
            )
            .column_as(distance_min.clone(), "distance")
            .group_by(Expr::col((
                message_embeddings::Entity,
                message_embeddings::Column::ConversationId,
            )))
            .order_by(distance_min, Order::Asc)
            .limit(limit)
            .offset(offset)
            .into_model::<SemanticConversationRow>()
            .all(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("conversation semantic query error -> {e}");
                AppError::DbTimeout
            })?;

    let conversation_ids = rows
        .into_iter()
        .map(|row| row.conversation_id)
        .collect::<Vec<Uuid>>();

    let snippets = load_semantic_snippets(
        &app_state.database,
        user_id,
        archived,
        &embedding_config,
        &conversation_ids,
        &embedding,
    )
    .await?;

    Ok(Some(SemanticConversationPage {
        conversation_ids,
        total: total as u64,
        snippets,
    }))
}

fn build_semantic_base_query(
    user_id: Uuid,
    archived: bool,
    config: &EmbeddingSettings,
) -> sea_orm::Select<message_embeddings::Entity> {
    let mut query = message_embeddings::Entity::find()
        .join(
            JoinType::InnerJoin,
            message_embeddings::Relation::Messages.def(),
        )
        .join(
            JoinType::InnerJoin,
            message_embeddings::Relation::Conversations.def(),
        )
        .filter(conversations::Column::UserId.eq(user_id))
        .filter(message_embeddings::Column::Provider.eq(config.provider.clone()))
        .filter(message_embeddings::Column::Model.eq(config.model.clone()))
        .filter(messages::Column::Deleted.eq(false))
        .filter(messages::Column::Role.is_in(vec![ChatRole::User, ChatRole::Assistant]));

    if archived {
        query = query.filter(conversations::Column::ArchivedAt.is_not_null());
    } else {
        query = query.filter(conversations::Column::ArchivedAt.is_null());
    }

    query
}

async fn load_total_matches(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    archived: bool,
    config: &EmbeddingSettings,
) -> Result<i64, AppError> {
    let row = build_semantic_base_query(user_id, archived, config)
        .select_only()
        .column_as(
            Expr::col((
                message_embeddings::Entity,
                message_embeddings::Column::ConversationId,
            ))
            .count_distinct(),
            "count",
        )
        .into_model::<CountRow>()
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("conversation semantic count query error -> {e}");
            AppError::DbTimeout
        })?;

    Ok(row.map(|row| row.count).unwrap_or(0))
}

async fn load_semantic_snippets(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
    archived: bool,
    config: &EmbeddingSettings,
    conversation_ids: &[Uuid],
    embedding: &[f32],
) -> Result<HashMap<Uuid, SemanticSnippet>, AppError> {
    if conversation_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // DISTINCT ON is not expressible in Sea-ORM. Since conversation_ids is bounded by the
    // page limit (typically ≤20), N+1 queries with LIMIT 1 ordered by distance are fine.
    let vector = format_pgvector(embedding);
    let mut results = HashMap::new();

    for &conv_id in conversation_ids {
        let distance = || {
            Expr::col(message_embeddings::Column::Embedding).binary(
                BinOper::Custom("<=>".into()),
                Expr::val(vector.clone()).cast_as(Alias::new("vector")),
            )
        };

        let row = build_semantic_base_query(user_id, archived, config)
            .filter(message_embeddings::Column::ConversationId.eq(conv_id))
            .select_only()
            .column_as(
                Expr::col((
                    message_embeddings::Entity,
                    message_embeddings::Column::ConversationId,
                )),
                "conversationId",
            )
            .column_as(
                Expr::col((
                    message_embeddings::Entity,
                    message_embeddings::Column::MessageId,
                )),
                "messageId",
            )
            .column_as(
                Expr::col((messages::Entity, messages::Column::MessageContent)),
                "snippet",
            )
            .column_as(distance(), "distance")
            .order_by(distance(), Order::Asc)
            .limit(1)
            .into_model::<SemanticSnippetRow>()
            .one(db)
            .await
            .map_err(|e| {
                eprintln!("semantic snippet query error -> {e}");
                AppError::DbTimeout
            })?;

        if let Some(row) = row {
            results.insert(
                row.conversation_id,
                SemanticSnippet {
                    message_id: row.message_id,
                    snippet: truncate_snippet(&row.snippet),
                    distance: row.distance,
                },
            );
        }
    }

    Ok(results)
}

async fn generate_search_embedding(
    app_state: &SharedState,
    config: &EmbeddingSettings,
    text: &str,
) -> Result<Option<Vec<f32>>, AppError> {
    let provider = config.provider.to_lowercase();
    // Prefer the admin-configured dimensions; fall back to the model's native dimensions from cache.
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
                .openai_create_embedding(
                    &openai_settings,
                    config.model.clone(),
                    vec![text.to_string()],
                    config.dimensions,
                )
                .await
                .map_err(|e| {
                    eprintln!("embedding request error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "openai".to_string(),
                    }
                })?;
            let mut data = response.data;
            data.sort_by_key(|item| item.index);
            Ok(data
                .into_iter()
                .next()
                .map(|item| normalize_to_target(item.embedding, target_dim)))
        }
        "mistral" => generate_mistral_search_embedding(app_state, config, text, target_dim).await,
        "gemini" => generate_gemini_search_embedding(app_state, config, text, target_dim).await,
        _ => Ok(None),
    }
}

async fn generate_mistral_search_embedding(
    app_state: &SharedState,
    config: &EmbeddingSettings,
    text: &str,
    target_dim: Option<usize>,
) -> Result<Option<Vec<f32>>, AppError> {
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
            "input": [text],
        }))
        .send()
        .await
        .map_err(|e| {
            eprintln!("mistral search embedding request error: {e}");
            AppError::LlmProviderNotConfigured {
                provider: "mistral".to_string(),
            }
        })?
        .error_for_status()
        .map_err(|e| {
            eprintln!("mistral search embedding status error: {e}");
            AppError::LlmProviderNotConfigured {
                provider: "mistral".to_string(),
            }
        })?
        .json()
        .await
        .map_err(|e| {
            eprintln!("mistral search embedding decode error: {e}");
            AppError::LlmProviderNotConfigured {
                provider: "mistral".to_string(),
            }
        })?;
    let mut data = response.data;
    data.sort_by_key(|item| item.index);
    Ok(data
        .into_iter()
        .next()
        .map(|item| normalize_to_target(item.embedding, target_dim)))
}

async fn generate_gemini_search_embedding(
    app_state: &SharedState,
    config: &EmbeddingSettings,
    text: &str,
    target_dim: Option<usize>,
) -> Result<Option<Vec<f32>>, AppError> {
    let gemini_settings = match app_state.settings.gemini.read().await.clone() {
        Some(settings) if settings.is_enabled => settings,
        _ => return Ok(None),
    };
    let mut body = serde_json::Map::new();
    body.insert(
        "content".to_string(),
        json!({
            "parts": [{"text": text}]
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
            eprintln!("gemini search embedding request error: {e}");
            AppError::LlmProviderNotConfigured {
                provider: "gemini".to_string(),
            }
        })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        eprintln!("gemini search embedding status error: {status} body: {body}");
        return Err(AppError::LlmProviderNotConfigured {
            provider: "gemini".to_string(),
        });
    }

    let parsed: GeminiEmbeddingResponse = response.json().await.map_err(|e| {
        eprintln!("gemini search embedding decode error: {e}");
        AppError::LlmProviderNotConfigured {
            provider: "gemini".to_string(),
        }
    })?;

    Ok(parsed
        .embedding
        .map(|e| normalize_to_target(e.values, target_dim)))
}

// Normalize only when the API returns a different length than expected (e.g. Mistral's
// fixed 1024-dim output used against a 1536-dim stored index). When target_dim is None
// the embedding is returned as-is.
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

fn format_pgvector(values: &[f32]) -> String {
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

fn truncate_snippet(value: &str) -> String {
    const MAX_LEN: usize = 240;
    let trimmed = value.trim();
    if trimmed.len() <= MAX_LEN {
        return trimmed.to_string();
    }
    let mut out = trimmed[..MAX_LEN].to_string();
    out.push_str("...");
    out
}
