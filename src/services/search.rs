use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, JoinType, QueryFilter, QueryOrder, QuerySelect,
    RelationTrait,
};
use sea_orm::sea_query::{Alias, BinOper, Expr, Func, Order};
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    config::setting::EmbeddingSettings,
    error::AppError,
    llm::provider::OpenaiApis,
    models::{conversations, message_embeddings, messages, messages::ChatRole},
    state::SharedState,
};

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
    let embedding = match generate_search_embedding(app_state, &embedding_config, query_text).await? {
        Some(embedding) => embedding,
        None => return Ok(None),
    };

    let total = load_total_matches(
        &app_state.database,
        user_id,
        archived,
        &embedding_config,
    )
    .await?;

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
    let distance_min: sea_orm::sea_query::SimpleExpr =
        Func::min(distance_expr.clone()).into();

    let rows: Vec<SemanticConversationRow> = build_semantic_base_query(
        user_id,
        archived,
        &embedding_config,
    )
    .select_only()
    .column_as(
        Expr::col((message_embeddings::Entity, message_embeddings::Column::ConversationId)),
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
        .join(JoinType::InnerJoin, message_embeddings::Relation::Messages.def())
        .join(JoinType::InnerJoin, message_embeddings::Relation::Conversations.def())
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
            Expr::col((message_embeddings::Entity, message_embeddings::Column::ConversationId))
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

    let vector = format_pgvector(embedding);
    let distance_expr = Expr::col(message_embeddings::Column::Embedding).binary(
        BinOper::Custom("<=>".into()),
        Expr::val(vector).cast_as(Alias::new("vector")),
    );

    let mut results = HashMap::new();
    for conversation_id in conversation_ids {
        let row = build_semantic_base_query(user_id, archived, config)
            .select_only()
            .column_as(
                Expr::col((message_embeddings::Entity, message_embeddings::Column::ConversationId)),
                "conversationId",
            )
            .column_as(
                Expr::col((message_embeddings::Entity, message_embeddings::Column::MessageId)),
                "messageId",
            )
            .column_as(
                Expr::col((messages::Entity, messages::Column::MessageContent)),
                "snippet",
            )
            .column_as(distance_expr.clone(), "distance")
            .filter(message_embeddings::Column::ConversationId.eq(*conversation_id))
            .order_by(distance_expr.clone(), Order::Asc)
            .limit(1)
            .into_model::<SemanticSnippetRow>()
            .one(db)
            .await
            .map_err(|e| {
                eprintln!("conversation semantic snippet query error -> {e}");
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
    match config.provider.to_lowercase().as_str() {
        "openai" => {
            let openai_settings = match app_state.settings.openai.read().await.clone() {
                Some(settings) if settings.is_enabled => settings,
                _ => return Ok(None),
            };
            let response = app_state
                .req_client
                .openai_create_embedding(&openai_settings, config.model.clone(), vec![text.to_string()])
                .await
                .map_err(|e| {
                    eprintln!("embedding request error: {e}");
                    AppError::LlmProviderNotConfigured {
                        provider: "openai".to_string(),
                    }
                })?;
            let mut data = response.data;
            data.sort_by_key(|item| item.index);
            Ok(data.into_iter().next().map(|item| item.embedding))
        }
        _ => Ok(None),
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
