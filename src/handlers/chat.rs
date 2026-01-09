use axum::{Json, extract::{Path, Query, State}};
use chrono::Utc;
use migration::extension::postgres::PgExpr;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, Iterable, PaginatorTrait as _, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;
use crate::{auth::{claims::Claims, error::AuthErrorResponse}, dto::{chat::{ArchiveChatRequest, ConversationResponse, MessageParts, MessageResponse, TokenUsage}, common::PaginationQuery, files::File}, error::{AppError, ErrorResponse}, models::{conversations::{self, ConversationWithCount}, messages::{self}}, state::SharedState};
use num_traits::cast::ToPrimitive;

#[utoipa::path(
    get,
    path = "/chat",
    tag = "chat",
    params(
        ("limit" = Option<u64>, Query, description = "Default value : 20"),
        ("offset" = Option<u64>, Query, description = "Default value : 0"),
        ("archived" = Option<bool>, Query, description = "Default value : false"),
        ("search" = Option<String>, Query, description = "Search in conversation titles"),
    ),
    responses(
        (status = 200, body = Vec<ConversationResponse>),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_chats(
  claims:Claims,
  Query(query):Query<PaginationQuery>,
  State(app_state): State<SharedState>
) -> Result<(StatusCode,Json<Vec<ConversationResponse>>),AppError>{
    let mut response = Vec::new();
    let limit = query.limit.unwrap_or(20);
    let offset = query.offset.unwrap_or(0);

    let mut select = conversations::Entity::find()
        .select_only()
        .columns(conversations::Column::iter())
        .column_as(messages::Column::Id.count(), "messageCount")
        .left_join(messages::Entity)
        .filter(conversations::Column::UserId.eq(claims.user_id));

    if let Some(title) = query.search {
        select = select.filter(conversations::Column::Title.into_expr().ilike(format!("%{}%",title)));
    }
    if query.archived.unwrap_or(false){
        select = select.filter(conversations::Column::ArchivedAt.is_not_null());
    }else{
        select = select.filter(conversations::Column::ArchivedAt.is_null());
    }
    
    select = select
        .group_by(conversations::Column::Id)
        .order_by_desc(conversations::Column::CreatedAt)
        .limit(limit)
        .offset(offset);

    // Run query into our projection struct
    let rows:Vec<ConversationWithCount> = select
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
        .map_err(|e|{
          eprintln!("conversation in count error {}",e);
          AppError::DbTimeout}
       )?;
       let conversation_response = ConversationResponse{ 
            id: conversation_with_count.id,
            title: conversation_with_count.title,
            archived: conversation_with_count.archived_at.is_some(),
            archived_at: conversation_with_count.archived_at,
            model:conversation_with_count.model_name,
            total_tokens: conversation_with_count.total_tokens,
            total_cost: conversation_with_count.total_cost.to_f32().unwrap_or_default(),
            created_at: conversation_with_count.created_at,
            updated_at: conversation_with_count.updated_at,
            last_message_at: conversation_with_count.last_message_at,
            message_count,
            messages:None 
        };
        response.push(conversation_response);
     }
  Ok((StatusCode::OK,Json(response)))
}

#[utoipa::path(
    get,
    path = "/chat/{chat_id}",
    tag = "chat",
    params(
        ("limit" = Option<u64>, Query, description = "Default value : 30"),
        ("offset" = Option<u64>, Query, description = "Default value : 0"),
        ("archived" = Option<bool>, Query, description = "Default value : false"),
        ("search" = Option<String>, Query, description = "Search in conversation titles"),
        ("ascending" = Option<bool>, Query, description = "Order of message list default true"),
    ),
    responses(
        (status = 200, body = ConversationResponse),
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found (code=1001)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    )
)]
pub async fn get_chat_by_id(
  claims:Claims,
  Path(chat_id):Path<Uuid>,
  Query(query):Query<PaginationQuery>,
  State(app_state): State<SharedState>
) -> Result<(StatusCode,Json<ConversationResponse>),AppError> {
    let conversation_model = conversations::Entity::find_by_id(chat_id)
      .filter(conversations::Column::UserId.eq(claims.user_id))
      .one(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("{}",e);
        AppError::DbTimeout})?
      .ok_or(AppError::DbNotFound)?; 

    let limit = query.limit.unwrap_or(30);
    let offset = query.offset.unwrap_or(0);
    let page = offset / limit;

    let paginator = messages::Entity::find()
      .filter(messages::Column::ConversationId.eq(chat_id))
      .filter(messages::Column::Deleted.eq(false))
      .order_by(
        messages::Column::CreatedAt,
        if query.ascending.unwrap_or(true) {
            sea_orm::Order::Asc
        } else {
            sea_orm::Order::Desc
        },
       )
       .paginate(&app_state.database, limit);

    let message_count = paginator.num_items()
      .await
      .map_err(|e|{
        eprintln!("{}",e);
        AppError::DbTimeout})?;

    let mut conversation_response = ConversationResponse{
        id: conversation_model.id,
        title: conversation_model.title,
        archived: conversation_model.archived_at.is_some(),
        archived_at: conversation_model.archived_at,
        model:conversation_model.model_name,
        total_tokens: conversation_model.total_tokens,
        total_cost: conversation_model.total_cost.to_f32().unwrap_or_default(),
        created_at: conversation_model.created_at,
        updated_at: conversation_model.updated_at,
        last_message_at: conversation_model.last_message_at,
        messages:Some(Vec::new()),
        message_count 
    };

    let messages_models = paginator.fetch_page(page)
     .await
     .map_err(|e|{
        eprintln!("{}",e);
        AppError::DbTimeout})?;
    messages_models
      .into_iter()
      .for_each(|message_model|{
        let metadata = message_model.metadata.as_ref();
        let model_params = if let Some(metadata) = metadata{
          metadata.get("params").cloned()
        }else{
            None
        };
        let files:Option<Vec<File>> = if let Some(metadata) = metadata{
          metadata.get("files")
           .cloned()
           .map(|value| serde_json::from_value::<Vec<File>>(value).unwrap_or(Vec::new()))
        }else{
            None
        };
        let message =  MessageResponse {
            id:message_model.id,
            role:message_model.role,
            cost:message_model.cost.to_f32().unwrap_or_default(),
            created_at: message_model.created_at,
            updated_at: message_model.updated_at,
            request_id:message_model.request_id,
            model:message_model.model_name,
            model_params: model_params,
            tool_calls: message_model.tools_calls,
            tools_results:message_model.tools_results,
            parts:MessageParts{ text: message_model.message_content, files}, 
            usage:TokenUsage{input_tokens:message_model.request_tokens,output_tokens:message_model.response_tokens,total_tokens:message_model.total_tokens} 
        };
        conversation_response.messages
          .as_mut()
          .unwrap()
          .push(message);
      });
  Ok((StatusCode::OK,Json(conversation_response)))
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
        (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
        (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found in database (code=5003)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn update_chat_by_id(
  claims:Claims,
  Path(chat_id):Path<Uuid>,
  State(app_state): State<SharedState>,
  Json(req):Json<ArchiveChatRequest>,
) -> Result<(StatusCode,Json<ConversationResponse>),AppError> {
    let utc_now = Utc::now();
    let conversation_model = conversations::Entity::find_by_id(chat_id)
       .filter(conversations::Column::UserId.eq(claims.user_id))
       .one(&app_state.database)
       .await
       .map_err(|e|{
        eprintln!("{}",e);
        AppError::DbTimeout})?
      .ok_or(AppError::DbNotFound)?;
    let message_count = messages::Entity::find()
       .filter(messages::Column::ConversationId.eq(conversation_model.id.clone()))
       .count(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("{}",e);
          AppError::DbTimeout}
       )?;
    let mut active_model = conversation_model
       .clone()
       .into_active_model();
    active_model.archived_at = if req.archived{
      Set(Some(utc_now))
    }else {
      Set(None)
    };
    active_model.title = Set(Some(req.title));
    active_model.update(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("{}",e);
          AppError::DbTimeout})?;
    let response = ConversationResponse{
        id: conversation_model.id,
        title: conversation_model.title,
        archived:req.archived,
        archived_at:Some(utc_now),
        model:conversation_model.model_name,
        total_tokens: conversation_model.total_tokens,
        total_cost: conversation_model.total_cost.to_f32().unwrap_or_default(),
        created_at: conversation_model.created_at,
        updated_at: conversation_model.updated_at,
        last_message_at: conversation_model.last_message_at,
        messages:None,
        message_count
    };
 Ok((StatusCode::OK,Json(response)))
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
       (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
       (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found in database (code=5003)"),
       (status = 503, content_type = "application/json", body = ErrorResponse, description = "Database timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn delete_chat_by_id(
  claims:Claims,
  Path(chat_id):Path<Uuid>,
  State(app_state): State<SharedState>
) -> Result<StatusCode,AppError> {
    let conversation_model = conversations::Entity::find_by_id(chat_id)
      .filter(conversations::Column::UserId.eq(claims.user_id))
      .one(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("{}",e);
        AppError::DbTimeout})?
      .ok_or(AppError::DbNotFound)?;
    conversation_model
      .into_active_model()
      .delete(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("{}",e);
        AppError::DbTimeout})?;
 Ok(StatusCode::NO_CONTENT)
}