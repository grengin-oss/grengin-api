use std::{convert::Infallible};
use axum::{Json, extract::{Path, State}, response::{Sse, sse::{Event, KeepAlive}}};
use chrono::Utc;
use futures_util::StreamExt;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait, IntoActiveModel, prelude::Decimal};
use serde_json::json;
use uuid::Uuid;
use crate::{auth::claims::Claims, dto::{chat_stream::{ChatInitRequest, ChatStream}, files::File, openai::OpenaiChatCompletionChunk}, error::AppError, llm::provider::OpenaiApis, models::{conversations, messages::{self, ChatRole}}, state::SharedState};
use reqwest_eventsource::{Event as ReqwestEvent};

#[utoipa::path(
    post,
    path = "/chat/stream/{chat_id}",
    tag = "chat",
    params(
        ("chat_id" = Option<Uuid>, Path, description = "Optional Chat id to stream messages for exiting chat"),
    ),
    request_body = ChatInitRequest,
    responses(
        (status = 200, content_type = "text/event-stream", body = ChatStream),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
    ),
)]
pub async fn handle_chat_stream_path_doc(){}

#[utoipa::path(
    post,
    path = "/chat/stream",
    tag = "chat",
    request_body = ChatInitRequest,
    responses(
        (status = 200, content_type = "text/event-stream", body = ChatStream),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
    ),
)]
pub async fn handle_chat_stream_doc(){}

pub async fn handle_chat_stream(
  claims:Claims,
  mut chat_id:Option<Path<Uuid>>,
  State(app_state): State<SharedState>,
  Json(req):Json<ChatInitRequest>
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>,AppError>{
 let llm_provider_settings = match req.provider.to_lowercase().as_str() {
     "openai" => app_state
                   .settings
                   .openai
                   .as_ref()
                   .ok_or(AppError::LlmProviderNotConfigured)?,
       _ => return Err(AppError::InvalidLlmProvider)
 };
 let mut metadata = json!({
  "temperature":req.temperature,
  "webSearch":req.web_search,
  "selectedTools":req.selected_tools,
 });
 if let Some(conversation_id) = req.conversation_id{
    chat_id = Some(Path(conversation_id));
 }
 let conversation_id = if let Some(Path(conversation_id)) = chat_id {
    let mut conversation = conversations::Entity::find_by_id(conversation_id.clone())
       .one(&app_state.database)
       .await
       .map_err(|e| {
          eprintln!("{:?}", e);
          AppError::ServiceTemporarilyUnavailable})?
       .ok_or(AppError::ResourceNotFound)?;
    if !req.selected_tools.is_empty(){
      if let Some(json) = conversation.metadata.as_mut() {
          // Update metadata TODO
       }
      conversation
        .metadata
        .as_mut()
        .or(Some(&mut metadata));
      conversation.updated_at = Utc::now();
    }
    conversation.last_message_at = Some(Utc::now());
    conversation
      .into_active_model()
      .update(&app_state.database)
      .await
      .map_err(|e| {
          eprintln!("{:?}", e);
          AppError::ServiceTemporarilyUnavailable})?;
     conversation_id
 }else{
  let first_prompt = req.messages
    .first()
    .map(|message| message.content.clone())
    .ok_or(AppError::NoMessageInRequest)?;
  let new_conversation_id = Uuid::new_v4();
  let title = app_state.req_client
     .openai_get_title(&llm_provider_settings,first_prompt)
     .await
     .map_err(|e| {
        eprintln!("{:?}", e);
        AppError::ServiceTemporarilyUnavailable})?;
  let new_conversation = conversations::ActiveModel{ 
    id:Set(new_conversation_id.clone()),
    user_id:Set(claims.user_id),
    title: Set(Some(title)),
    model_provider:Set(req.provider.clone()),
    model_name:Set(req.model_name.clone()),
    created_at:Set(Utc::now()),
    updated_at: Set(Utc::now()),
    last_message_at:Set(Some(Utc::now())),
    archived_at:Set(None),
    message_count:Set(1),
    total_tokens: Set(0),
    total_cost:Set(Decimal::from(0)),
    metadata:Set(Some(metadata.clone()))
   };
  new_conversation
    .insert(&app_state.database)
    .await
    .map_err(|e| {
       eprintln!("{:?}", e);
       AppError::ServiceTemporarilyUnavailable})?;
    new_conversation_id
 };
 let mut previous_message_id = None;
 for message in &req.messages {
   let new_message_id = Uuid::new_v4();
   metadata["files"] = message.files
     .iter()
     .map(|f| serde_json::to_value(f).unwrap()).collect::<Vec<serde_json::Value>>().into();
   let new_message = messages::ActiveModel{ 
     id:Set(new_message_id),
     conversation_id:Set(conversation_id),
     previous_message_id:Set(previous_message_id),
     role:Set(message.role),
     message_content:Set(message.content.clone()),
     model_provider:Set(req.provider.clone()),
     model_name:Set(req.model_name.clone()),
     request_id:Set(None),
     request_tokens:Set(0),
     response_tokens:Set(0),
     tools_calls:Set(Vec::new()),
     tools_results:Set(Vec::new()),
     created_at:Set(Utc::now()),
     updated_at:Set(Utc::now()),
     total_tokens:Set(0),
     latency:Set(0),
     cost:Set(Decimal::from(0)),
     metadata:Set(Some(metadata.clone())),
  };
  previous_message_id = Some(new_message_id);
  new_message
   .clone()
   .insert(&app_state.database)
   .await
   .map_err(|e| {
        eprintln!("{:?}", e);
        AppError::ServiceTemporarilyUnavailable})?;
 }
 let prompts = req.messages
   .iter()
   .map(|message| message.content.clone())
   .collect();
 let files:Vec<File> = req.messages
   .into_iter()
   .map(|message| message.files)
   .collect::<Vec<Vec<File>>>()
   .into_iter()
   .flatten()
   .collect();
 let mut event_source = app_state
   .req_client
   .openai_chat_stream(&llm_provider_settings,req.model_name.clone(),prompts,req.temperature,files)
   .await
   .map_err(|e|{
      eprintln!("event source loding error {} for llm provider {}",e,&req.provider);
      AppError::ServiceTemporarilyUnavailable})?;
 let sse_stream = async_stream::try_stream! {
    let mut message_content = String::new();
    let mut request_id = None;
     while let Some(event) = event_source.next().await {
        match event {
            Ok(ReqwestEvent::Open) => {
                println!("SSE connection open");
            }
            Ok(ReqwestEvent::Message(msg)) => {
                if msg.data == "[DONE]" {
                    let new_llm_message = messages::ActiveModel{ 
                       id:Set(Uuid::new_v4()),
                       conversation_id:Set(conversation_id.clone()),
                       previous_message_id:Set(previous_message_id),
                       role:Set(ChatRole::Assistant),
                       message_content:Set(message_content),
                       model_provider:Set(req.provider.clone()),
                       model_name:Set(req.model_name.clone()),
                       request_id:Set(request_id),
                       request_tokens:Set(0),
                       response_tokens:Set(0),
                       tools_calls:Set(Vec::new()),
                       tools_results:Set(Vec::new()),
                       created_at:Set(Utc::now()),
                       updated_at:Set(Utc::now()),
                       total_tokens:Set(0),
                       latency:Set(0),
                       cost:Set(Decimal::from(0)),
                       metadata:Set(None),
                   };
                   new_llm_message
                    .insert(&app_state.database)
                    .await
                    .expect("failed to insert llm response in table messages");
                  yield Event::default().event("chunk").data("[DONE]");
                  break;
                }
                if let Ok(chunk) = serde_json::from_str::<OpenaiChatCompletionChunk>(&msg.data){
                  if let Some(delta) = chunk.choices.first().map(|c| &c.delta){
                    let chat_stream = ChatStream{id:conversation_id.clone(),role:delta.role.clone(),content:delta.content.clone()};
                    yield Event::default().event("chunk").data(chat_stream.to_string());
                    request_id = Some(chunk.id);
                    message_content = format!("{}{}",message_content,delta.content.clone().unwrap_or(String::new()));
                 }
               }
             }
            Err(e) => {
                eprintln!("chat stream error: {e}");
                break;
            }
        }
      };
 };
 let sse_response = Sse::new(sse_stream).keep_alive(KeepAlive::new());
 Ok(sse_response)
}

