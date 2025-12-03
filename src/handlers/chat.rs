use axum::{Json, extract::{Path, Query, State}};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect};
use uuid::Uuid;
use crate::{auth::claims::Claims, dto::{chat::{ArchiveChatRequest, Attachment, ConversationResponse, File, MessageParts, MessageResponse, TokenUsage}, common::PaginationQuery}, error::{AppError}, models::{conversations, messages}, state::SharedState};
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
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
    )
)]
pub async fn get_chats(
  claims:Claims,
  Query(query):Query<PaginationQuery>,
  State(app_state): State<SharedState>
) -> Result<(StatusCode,Json<Vec<ConversationResponse>>),AppError>{
    let mut response = Vec::new();
    let mut conversations_filter = conversations::Entity::find()
      .filter(conversations::Column::UserId.eq(claims.user_id))
      .order_by_asc(conversations::Column::CreatedAt)
      .limit(query.limit.unwrap_or(20))
      .offset(query.offset.unwrap_or(0));
    
    if let Some(title) = query.search {
        conversations_filter = conversations_filter.filter(conversations::Column::Title.eq(title));
    }
    if query.archived.unwrap_or(false){
        conversations_filter = conversations_filter.filter(conversations::Column::ArchivedAt.is_not_null());
    }else{
        conversations_filter = conversations_filter.filter(conversations::Column::ArchivedAt.is_null());
    }
    let conversations_models = conversations_filter
      .all(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("{}",e);
        AppError::ServiceTemporarilyUnavailable}
    )?;
    conversations_models
      .into_iter()
      .for_each(|conversation_model|{
        let conversation_response = ConversationResponse{ 
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
            messages:None 
        };
        response.push(conversation_response);
    });
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
    ),
    responses(
        (status = 200, body = Vec<ConversationResponse>),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
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
        AppError::ServiceTemporarilyUnavailable})?
      .ok_or(AppError::ResourceNotFound)?;
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
    };
    let messages_models = messages::Entity::find()
      .filter(messages::Column::ConversationId.eq(chat_id))
      .filter(conversations::Column::UserId.eq(claims.user_id))
      .order_by_asc(conversations::Column::CreatedAt)
      .limit(query.limit.unwrap_or(30))
      .offset(query.offset.unwrap_or(0))
      .all(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("{}",e);
        AppError::ServiceTemporarilyUnavailable})?;
    messages_models
      .into_iter()
      .for_each(|message_model|{
        let model_params = if let Some(metadata) = &message_model.metadata{
          metadata.get("params").cloned()
        }else{
            None
        };
        let files:Option<Vec<File>> = if let Some(metadata) = &message_model.metadata{
          let attactments:Option<Vec<Attachment>> = metadata.get("attachments")
            .cloned()
            .map(|value| serde_json::from_value::<Vec<Attachment>>(value).unwrap_or(Vec::new()));
          let files = attactments
            .map(|attachments|{
              attachments
               .into_iter()
               .map(|attachment|{
                 File {
                  id:None,
                  size: attachment.file.map(|f| f.len()),
                  name: attachment.name,
                  content_type: attachment.content_type 
                 }
               }).collect()
            });
         files
        }else{
            None
        };
        let message =  MessageResponse {
            id:message_model.id,
            role:message_model.role,
            cost:message_model.cost.to_f32().unwrap_or_default(),
            created_at: message_model.created_at,
            updated_at: message_model.updated_at,
            request_id:None,
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
 Ok((StatusCode::NO_CONTENT,Json(conversation_response)))
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
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
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
        AppError::ServiceTemporarilyUnavailable})?
      .ok_or(AppError::ResourceNotFound)?;
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
        AppError::ServiceTemporarilyUnavailable})?;
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
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
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
        AppError::ServiceTemporarilyUnavailable})?
      .ok_or(AppError::ResourceNotFound)?;
    conversation_model
      .into_active_model()
      .delete(&app_state.database)
      .await
      .map_err(|e|{
        eprintln!("{}",e);
        AppError::ServiceTemporarilyUnavailable})?;
 Ok(StatusCode::NO_CONTENT)
}