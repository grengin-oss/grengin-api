use std::{convert::Infallible};
use axum::{Json, extract::{State}, response::{Sse, sse::{Event, KeepAlive}}};
use chrono::Utc;
use futures_util::StreamExt;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, prelude::Decimal};
use serde_json::json;
use uuid::Uuid;
use crate::{auth::claims::Claims, dto::{chat::{ChatInitRequest, ChatStream, File}, openai::OpenaiChatCompletionChunk}, error::AppError, llm::provider::OpenaiApis, models::{conversations, messages::{self, ChatRole}}, state::SharedState};
use reqwest_eventsource::{Event as ReqwestEvent};

#[utoipa::path(
    post,
    path = "/chat/stream",
    tag = "chat",
    request_body = ChatInitRequest,
    responses(
        (status = 200, content_type = "text/event-stream", body = ChatStream ),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
    )
)]
pub async fn handle_chat_stream(
  claims:Claims,
  State(app_state): State<SharedState>,
  Json(req):Json<ChatInitRequest>
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>,AppError>{
 let llm_provider_settings = match req.provider.to_lowercase().as_str() {
     "openai" => &app_state.settings.openai
                   .as_ref()
                   .ok_or(AppError::LlmProviderNotConfigured)?,
       _ => return Err(AppError::InvalidLlmProvider)
 };
 let mut files:Vec<File> = vec![];
 for attachment in req.message.files{
    let id = app_state
      .req_client
      .openai_upload_file(llm_provider_settings, &attachment)
      .await
      .ok();
    files.push(
      File {
        id,
        name:attachment.name,
        content_type:attachment.content_type,
        size:attachment.file.map(|v| v.len())
    });
 }
 let metadata = json!({
  "files":&files,
  "temperature":req.temperature,
  "webSearch":req.web_search,
  "selectedTools":req.selected_tools,
 });
 let new_conversation_id = Uuid::new_v4();
 let new_conversation = conversations::ActiveModel{ 
   id:Set(new_conversation_id.clone()),
   user_id:Set(claims.user_id),
   title: Set(None),
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
 let new_message_id = Uuid::new_v4();
 let new_message = messages::ActiveModel{ 
   id:Set(new_message_id),
   conversation_id:Set(new_conversation_id),
   previous_message_id:Set(None),
   role:Set(ChatRole::User),
   message_content:Set(req.message.content.clone()),
   model_provider:Set(req.provider.clone()),
   model_name:Set(req.model_name.clone()),
   request_tokens:Set(0),
   response_tokens:Set(0),
   tools_calls:Set(Vec::new()),
   tools_results:Set(Vec::new()),
   created_at:Set(Utc::now()),
   updated_at:Set(Utc::now()),
   total_tokens:Set(0),
   latency:Set(0),
   cost:Set(Decimal::from(0)),
   metadata:Set(Some(metadata)),
 };
 new_message
   .clone()
   .insert(&app_state.database)
   .await
   .map_err(|e| {
        eprintln!("{:?}", e);
        AppError::ServiceTemporarilyUnavailable})?;
 let mut event_source = app_state
     .req_client
     .openai_chat_stream(&llm_provider_settings,req.model_name,req.message.content,req.temperature, files)
     .await
     .map_err(|e|{
       eprintln!("event source loding error {} for llm provider {}",e,&req.provider);
       AppError::ServiceTemporarilyUnavailable
     })?;
 let sse_stream = async_stream::try_stream! {
    let mut message_content = String::new();
     while let Some(event) = event_source.next().await {
        match event {
            Ok(ReqwestEvent::Open) => {
                println!("SSE connection open");
            }
            Ok(ReqwestEvent::Message(msg)) => {
                if msg.data == "[DONE]" {
                    let new_llm_message = messages::ActiveModel{ 
                       id:Set(Uuid::new_v4()),
                       conversation_id:Set(new_conversation_id.clone()),
                       previous_message_id:Set(Some(new_message_id)),
                       role:Set(ChatRole::Assistant),
                       message_content:Set(message_content),
                       model_provider:new_message.model_provider,
                       model_name:new_message.model_name,
                       request_tokens:Set(0),
                       response_tokens:Set(0),
                       tools_calls:Set(Vec::new()),
                       tools_results:Set(Vec::new()),
                       created_at:new_message.created_at,
                       updated_at:new_message.updated_at,
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
                    let chat_stream = ChatStream{id:new_conversation_id.clone(),role:delta.role.clone(),content:delta.content.clone()};
                    yield Event::default().event("chunk").data(chat_stream.to_string());
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
