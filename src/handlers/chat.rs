use crate::{
    auth::{claims::Claims, error::Error},
    dto::{
        chat::{
            ArchiveChatRequest, ArtifactMeta, ConversationResponse, MessageParts, MessageResponse,
            PaginatedConversations, SemanticResult, TokenUsage,
        },
        common::PaginationQuery,
        files::File,
    },
    error::{AppError, ErrorResponse},
    models::{
        conversations::{self, ConversationWithCount},
        messages::{self},
    },
    services::{chat_helpers::resolve_web_search_enabled, search},
    state::SharedState,
};
use axum::{
    Json,
    extract::{Path, Query, State},
};
use chrono::Utc;
use num_traits::cast::ToPrimitive;
use reqwest::StatusCode;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, Iterable,
    PaginatorTrait as _, QueryFilter, QueryOrder, QuerySelect,
};
use uuid::Uuid;


#[utoipa::path(
    get,
    path = "/chat",
    tag = "chat",
    params(
        ("limit" = Option<u64>, Query, description = "Number of items per page (default: 20, max: 100)"),
        ("offset" = Option<u64>, Query, description = "Number of items to skip (default: 0)"),
        ("archived" = Option<bool>, Query, description = "Filter by archived status. If not provided, returns only non-archived conversations"),
        ("search" = Option<String>, Query, description = "Search text (title match by default; use semantic=true to enable semantic search)"),
        ("semantic" = Option<bool>, Query, description = "Enable semantic search for query text (default: false)"),
    ),
    responses(
        (status = 200, body = PaginatedConversations),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_chats(
    claims: Claims,
    Query(query): Query<PaginationQuery>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<PaginatedConversations>), AppError> {
    let mut response = Vec::new();
    let limit = query.limit.unwrap_or(20).min(100); // Cap at 100
    let offset = query.offset.unwrap_or(0);
    let search = query
        .search
        .clone()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let enable_semantic = query.semantic.unwrap_or(false);
    let archived = query.archived.unwrap_or(false);

    let semantic_results_fallback = if enable_semantic && search.is_some() {
        Some(std::collections::HashMap::new())
    } else {
        None
    };

    if !enable_semantic {
        if let Some(search_text) = search.as_ref() {
            let page = search::lexical_conversation_search(
                &app_state.database,
                claims.user_id,
                search_text,
                archived,
                limit,
                offset,
            )
            .await?;

            if !page.conversation_ids.is_empty() {
                let mut select = conversations::Entity::find()
                    .select_only()
                    .columns(conversations::Column::iter())
                    .column_as(messages::Column::Id.count(), "messageCount")
                    .left_join(messages::Entity)
                    .filter(conversations::Column::UserId.eq(claims.user_id))
                    .filter(conversations::Column::Id.is_in(page.conversation_ids.clone()));

                if archived {
                    select = select.filter(conversations::Column::ArchivedAt.is_not_null());
                } else {
                    select = select.filter(conversations::Column::ArchivedAt.is_null());
                }

                let rows: Vec<ConversationWithCount> = select
                    .group_by(conversations::Column::Id)
                    .into_model::<ConversationWithCount>()
                    .all(&app_state.database)
                    .await
                    .map_err(|e| {
                        eprintln!("lexical search conversation fetch error: {e}");
                        AppError::DbTimeout
                    })?;

                let mut row_map = std::collections::HashMap::new();
                for row in rows {
                    row_map.insert(row.id, row);
                }

                for conversation_id in &page.conversation_ids {
                    let Some(conversation_with_count) = row_map.remove(conversation_id) else {
                        continue;
                    };
                    let web_search_enabled =
                        resolve_web_search_enabled(conversation_with_count.metadata.as_ref());
                    response.push(ConversationResponse {
                        id: conversation_with_count.id,
                        title: conversation_with_count.title,
                        web_search_enabled,
                        archived: conversation_with_count.archived_at.is_some(),
                        archived_at: conversation_with_count.archived_at,
                        model: conversation_with_count.model_name,
                        total_tokens: conversation_with_count.total_tokens,
                        total_cost: conversation_with_count.total_cost.to_f32().unwrap_or_default(),
                        created_at: conversation_with_count.created_at,
                        updated_at: conversation_with_count.updated_at,
                        last_message_at: conversation_with_count.last_message_at,
                        message_count: conversation_with_count.message_count.max(0) as u64,
                        messages: None,
                        search_score: page.scores.get(conversation_id).copied(),
                        search_snippet: page.snippets.get(conversation_id).cloned(),
                    });
                }
            }
            let payload = PaginatedConversations {
                total: page.total,
                limit,
                offset,
                conversations: response,
                semantic_results: semantic_results_fallback,
            };
            return Ok((StatusCode::OK, Json(payload)));
        }
    }

    if enable_semantic {
        if let Some(search_text) = search.as_ref() {
            if let Some(page) = search::semantic_conversation_search(
                &app_state,
                claims.user_id,
                search_text,
                archived,
                limit,
                offset,
            )
            .await?
            {
                if page.total > 0 {
                    let mut select = conversations::Entity::find()
                        .select_only()
                        .columns(conversations::Column::iter())
                        .column_as(messages::Column::Id.count(), "messageCount")
                        .left_join(messages::Entity)
                        .filter(conversations::Column::UserId.eq(claims.user_id))
                        .filter(conversations::Column::Id.is_in(page.conversation_ids.clone()));

                    if archived {
                        select = select.filter(conversations::Column::ArchivedAt.is_not_null());
                    } else {
                        select = select.filter(conversations::Column::ArchivedAt.is_null());
                    }

                    let rows: Vec<ConversationWithCount> = select
                        .group_by(conversations::Column::Id)
                        .into_model::<ConversationWithCount>()
                        .all(&app_state.database)
                        .await
                        .map_err(|e| {
                            eprintln!("conversation semantic query error -> {e}");
                            AppError::DbTimeout
                        })?;

                    let mut row_map = std::collections::HashMap::new();
                    for row in rows {
                        row_map.insert(row.id, row);
                    }

                    for conversation_id in page.conversation_ids {
                        let Some(conversation_with_count) = row_map.remove(&conversation_id) else {
                            continue;
                        };
                        let web_search_enabled =
                            resolve_web_search_enabled(conversation_with_count.metadata.as_ref());
                        let conversation_response = ConversationResponse {
                            id: conversation_with_count.id,
                            title: conversation_with_count.title,
                            web_search_enabled,
                            archived: conversation_with_count.archived_at.is_some(),
                            archived_at: conversation_with_count.archived_at,
                            model: conversation_with_count.model_name,
                            total_tokens: conversation_with_count.total_tokens,
                            total_cost: conversation_with_count
                                .total_cost
                                .to_f32()
                                .unwrap_or_default(),
                            created_at: conversation_with_count.created_at,
                            updated_at: conversation_with_count.updated_at,
                            last_message_at: conversation_with_count.last_message_at,
                            message_count: conversation_with_count.message_count.max(0) as u64,
                            messages: None,
                            search_score: None,
                            search_snippet: None,
                        };
                        response.push(conversation_response);
                    }

                    let semantic_results = Some(
                        page.snippets
                            .into_iter()
                            .map(|(conversation_id, snippet)| {
                                (
                                    conversation_id,
                                    SemanticResult {
                                        message_id: snippet.message_id,
                                        snippet: snippet.snippet,
                                        distance: snippet.distance,
                                    },
                                )
                            })
                            .collect(),
                    );
                    let payload = PaginatedConversations {
                        total: page.total,
                        limit,
                        offset,
                        conversations: response,
                        semantic_results,
                    };
                    return Ok((StatusCode::OK, Json(payload)));
                }
            }
        }
    }

    let mut count_query =
        conversations::Entity::find().filter(conversations::Column::UserId.eq(claims.user_id));
    if archived {
        count_query = count_query.filter(conversations::Column::ArchivedAt.is_not_null());
    } else {
        count_query = count_query.filter(conversations::Column::ArchivedAt.is_null());
    }
    let total = count_query.count(&app_state.database).await.map_err(|e| {
        eprintln!("conversation count query error -> {e}");
        AppError::DbTimeout
    })?;

    let mut select = conversations::Entity::find()
        .select_only()
        .columns(conversations::Column::iter())
        .column_as(messages::Column::Id.count(), "messageCount")
        .left_join(messages::Entity)
        .filter(conversations::Column::UserId.eq(claims.user_id));

    if archived {
        select = select.filter(conversations::Column::ArchivedAt.is_not_null());
    } else {
        select = select.filter(conversations::Column::ArchivedAt.is_null());
    }

    select = select
        .group_by(conversations::Column::Id)
        .order_by_desc(conversations::Column::UpdatedAt)
        .limit(limit)
        .offset(offset);

    // Run query into our projection struct
    let rows: Vec<ConversationWithCount> = select
        .into_model::<ConversationWithCount>()
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("conversation in count query error -> {e}");
            AppError::DbTimeout
        })?;
    for conversation_with_count in rows {
        let message_count = messages::Entity::find()
            .filter(messages::Column::ConversationId.eq(conversation_with_count.id.clone()))
            .count(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("conversation in count error {}", e);
                AppError::DbTimeout
            })?;
        let web_search_enabled =
            resolve_web_search_enabled(conversation_with_count.metadata.as_ref());
        let conversation_response = ConversationResponse {
            id: conversation_with_count.id,
            title: conversation_with_count.title,
            web_search_enabled,
            archived: conversation_with_count.archived_at.is_some(),
            archived_at: conversation_with_count.archived_at,
            model: conversation_with_count.model_name,
            total_tokens: conversation_with_count.total_tokens,
            total_cost: conversation_with_count
                .total_cost
                .to_f32()
                .unwrap_or_default(),
            created_at: conversation_with_count.created_at,
            updated_at: conversation_with_count.updated_at,
            last_message_at: conversation_with_count.last_message_at,
            message_count,
            messages: None,
            search_score: None,
            search_snippet: None,
        };
        response.push(conversation_response);
    }
    let payload = PaginatedConversations {
        total,
        limit,
        offset,
        conversations: response,
        semantic_results: semantic_results_fallback,
    };
    Ok((StatusCode::OK, Json(payload)))
}

#[utoipa::path(
    get,
    path = "/chat/{chat_id}",
    tag = "chat",
    params(
        ("chat_id" = Uuid, Path, description = "Unique identifier for the conversation"),
    ),
    responses(
        (status = 200, body = ConversationResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
        (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found (code=1001)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_chat_by_id(
    claims: Claims,
    Path(chat_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<(StatusCode, Json<ConversationResponse>), AppError> {
    let conversation_model = conversations::Entity::find_by_id(chat_id)
        .filter(conversations::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("{}", e);
            AppError::DbTimeout
        })?
        .ok_or(AppError::DbNotFound)?;

    let messages_models = messages::Entity::find()
        .filter(messages::Column::ConversationId.eq(chat_id))
        .filter(messages::Column::Deleted.eq(false))
        .order_by_asc(messages::Column::CreatedAt)
        .all(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("{}", e);
            AppError::DbTimeout
        })?;

    let message_count = messages_models.len() as u64;

    let web_search_enabled = resolve_web_search_enabled(conversation_model.metadata.as_ref());
    let mut conversation_response = ConversationResponse {
        id: conversation_model.id,
        title: conversation_model.title,
        web_search_enabled,
        archived: conversation_model.archived_at.is_some(),
        archived_at: conversation_model.archived_at,
        model: conversation_model.model_name,
        total_tokens: conversation_model.total_tokens,
        total_cost: conversation_model.total_cost.to_f32().unwrap_or_default(),
        created_at: conversation_model.created_at,
        updated_at: conversation_model.updated_at,
        last_message_at: conversation_model.last_message_at,
        messages: Some(Vec::new()),
        message_count,
        search_score: None,
        search_snippet: None,
    };

    messages_models.into_iter().for_each(|message_model| {
        let metadata = message_model.metadata.as_ref();
        let model_params = if let Some(metadata) = metadata {
            metadata.get("params").cloned()
        } else {
            None
        };
        let files: Option<Vec<File>> = if let Some(metadata) = metadata {
            metadata
                .get("files")
                .cloned()
                .map(|value| serde_json::from_value::<Vec<File>>(value).unwrap_or(Vec::new()))
        } else {
            None
        };
        let artifacts: Option<Vec<ArtifactMeta>> = metadata.and_then(|m| {
            m.get("artifacts")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
        });
        let message = MessageResponse {
            id: message_model.id,
            role: message_model.role,
            cost: message_model.cost.to_f32().unwrap_or_default(),
            created_at: message_model.created_at,
            updated_at: message_model.updated_at,
            request_id: message_model.request_id,
            model: message_model.model_name,
            model_params: model_params,
            tool_calls: message_model.tools_calls,
            tools_results: message_model.tools_results,
            parts: MessageParts {
                text: message_model.message_content,
                files,
                artifacts,
            },
            usage: TokenUsage {
                input_tokens: message_model.request_tokens,
                output_tokens: message_model.response_tokens,
                total_tokens: message_model.total_tokens,
            },
        };
        conversation_response
            .messages
            .as_mut()
            .unwrap()
            .push(message);
    });
    Ok((StatusCode::OK, Json(conversation_response)))
}

#[utoipa::path(
    put,
    path = "/chat/{chat_id}",
    tag = "chat",
    params(
        ("chat_id" = Uuid, Path, description = "Unique identifier for the conversation"),
    ),
    responses(
        (status = 200, body = ConversationResponse),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
        (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found in database (code=5003)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn update_chat_by_id(
    claims: Claims,
    Path(chat_id): Path<Uuid>,
    State(app_state): State<SharedState>,
    Json(req): Json<ArchiveChatRequest>,
) -> Result<(StatusCode, Json<ConversationResponse>), AppError> {
    let utc_now = Utc::now();
    let conversation_model = conversations::Entity::find_by_id(chat_id)
        .filter(conversations::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("{}", e);
            AppError::DbTimeout
        })?
        .ok_or(AppError::DbNotFound)?;
    let message_count = messages::Entity::find()
        .filter(messages::Column::ConversationId.eq(conversation_model.id.clone()))
        .count(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("{}", e);
            AppError::DbTimeout
        })?;
    let mut active_model = conversation_model.clone().into_active_model();
    active_model.archived_at = if req.archived {
        Set(Some(utc_now))
    } else {
        Set(None)
    };
    active_model.title = Set(Some(req.title));
    active_model
        .update(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("{}", e);
            AppError::DbTimeout
        })?;
    let web_search_enabled = resolve_web_search_enabled(conversation_model.metadata.as_ref());
    let response = ConversationResponse {
        id: conversation_model.id,
        title: conversation_model.title,
        web_search_enabled,
        archived: req.archived,
        archived_at: Some(utc_now),
        model: conversation_model.model_name,
        total_tokens: conversation_model.total_tokens,
        total_cost: conversation_model.total_cost.to_f32().unwrap_or_default(),
        created_at: conversation_model.created_at,
        updated_at: conversation_model.updated_at,
        last_message_at: conversation_model.last_message_at,
        messages: None,
        message_count,
        search_score: None,
        search_snippet: None,
    };
    Ok((StatusCode::OK, Json(response)))
}

#[utoipa::path(
    delete,
    path = "/chat/{chat_id}",
    tag = "chat",
    params(
        ("chat_id" = Uuid, Path, description = "Unique identifier for the conversation"),
    ),
    responses(
       (status = 204, description = "Deleted successfully"),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found in database (code=5003)"),
       (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn delete_chat_by_id(
    claims: Claims,
    Path(chat_id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AppError> {
    let conversation_model = conversations::Entity::find_by_id(chat_id)
        .filter(conversations::Column::UserId.eq(claims.user_id))
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("{}", e);
            AppError::DbTimeout
        })?
        .ok_or(AppError::DbNotFound)?;
    conversation_model
        .into_active_model()
        .delete(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("{}", e);
            AppError::DbTimeout
        })?;
    Ok(StatusCode::NO_CONTENT)
}
