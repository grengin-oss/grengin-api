use std::convert::Infallible;
use axum::{Json, extract::{Path, State}, response::{Sse, sse::Event}};
use chrono::Utc;
use reqwest::StatusCode;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, sea_query};
use uuid::Uuid;
use crate::{auth::claims::Claims, dto::chat_stream::{ChatInitRequest, ChatStream}, error::AppError, handlers::chat_stream::handle_chat_stream, models::{conversations, messages}, state::SharedState};

#[utoipa::path(
    delete,
    path = "/chat/{chat_id}/message/{message_id}",
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
pub async fn delete_chat_message_by_id(
  claims:Claims,
  Path((chat_id,message_id)):Path<(Uuid,Uuid)>,
  State(app_state): State<SharedState>
) -> Result<StatusCode,AppError> {
  let (_,message) = conversations::Entity::find()
    .filter(conversations::Column::Id.eq(chat_id))
    .filter(conversations::Column::UserId.eq(claims.user_id))
    .inner_join(messages::Entity)
    .filter(messages::Column::Id.eq(message_id))
    .select_also(messages::Entity)
    .one(&app_state.database)
    .await
    .map_err(|e|{
      eprintln!("db error :{}",e);
      AppError::ServiceTemporarilyUnavailable
     })?
    .ok_or(AppError::ResourceNotFound)?;
  let mut active_model = message
    .ok_or(AppError::ResourceNotFound)?
    .into_active_model();
  active_model.deleted = Set(true);
  active_model.updated_at = Set(Utc::now());
  active_model
    .update(&app_state.database)
    .await
    .map_err(|e|{
      eprintln!("db error :{}",e);
      AppError::ServiceTemporarilyUnavailable
    })?;
 Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    patch,
    path = "/chat/{chat_id}/message/{message_id}/stream",
    tag = "chat",
    params(
        ("chat_id" = Uuid, Path, description = "Unique identifier for the conversation"),
        ("message_id" = Uuid, Path, description = "Unique identifier for the message"),
    ),
    request_body = ChatInitRequest,
    responses(
        (status = 200, content_type = "text/event-stream", body = ChatStream),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
    )
)]
pub async fn edit_chat_message_by_id_and_stream(
  claims:Claims,
  Path((chat_id,message_id)):Path<(Uuid,Uuid)>,
  State(app_state): State<SharedState>,
  Json(req):Json<ChatInitRequest>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>,AppError> {
     let message = conversations::Entity::find()
       .filter(conversations::Column::Id.eq(chat_id))
       .filter(conversations::Column::UserId.eq(claims.user_id.clone()))
       .inner_join(messages::Entity)
       .filter(messages::Column::Id.eq(message_id))
       .select_also(messages::Entity)
       .one(&app_state.database)
       .await
       .map_err(|e|{
          eprintln!("db error :{}",e);
          AppError::ServiceTemporarilyUnavailable
        })?
       .ok_or(AppError::ResourceNotFound)?
       .1
       .ok_or(AppError::ResourceNotFound)?;
     messages::Entity::update_many()
       .filter(messages::Column::ConversationId.eq(chat_id))
       .filter(messages::Column::Deleted.eq(false))
       .filter(messages::Column::CreatedAt.gte(message.created_at))
       .col_expr(messages::Column::Deleted,sea_query::Expr::value(true))
       .exec(&app_state.database)
       .await
          .map_err(|e|{
           eprintln!("db update many error :{}",e);
            AppError::ServiceTemporarilyUnavailable
        })?;
 Ok(handle_chat_stream(claims, Some(Path(chat_id)), State(app_state), Json(req)).await?)
}