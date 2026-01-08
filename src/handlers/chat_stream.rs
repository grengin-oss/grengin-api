use std::convert::Infallible;
use axum::{Json, extract::{Path, State}, response::{Sse, sse::{Event, KeepAlive}}};
use chrono::Utc;
use futures_util::StreamExt;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, prelude::Decimal};
use serde_json::json;
use uuid::Uuid;
use crate::{
    auth::{claims::Claims, error::AuthErrorResponse},
    config::setting::{AnthropicSettings, OpenaiSettings},
    dto::{
        chat_stream::{ChatInitRequest, ChatStream},
        files::File,
        llm::anthropic::ANTHROPIC_DEFAULT_MAX_TOKENS,
    },
    error::{AppError, ErrorResponse},
    handlers::llm::{StreamParseResult, StreamParser, anthropic::AnthropicStreamParser, openai::OpenaiStreamParser},
    llm::{prompt::Prompt, provider::{AnthropicApis, OpenaiApis, get_title_generation_model}},
    models::{conversations, messages::{self, ChatRole}},
    state::SharedState,
};
use reqwest_eventsource::Event as ReqwestEvent;

/// Provider configuration enum for handling different LLM providers
enum LlmProviderConfig<'a> {
    OpenAI(&'a OpenaiSettings),
    Anthropic(&'a AnthropicSettings),
}

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
    (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
    (status = 400, content_type = "application/json", body = ErrorResponse, description = "Validation error (code=2002 empty messages)"),
    (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003)"),
    (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found / DB not found (code=5003)"),
    (status = 503, content_type = "application/json", body = ErrorResponse, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),

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
    (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
    (status = 400, content_type = "application/json", body = ErrorResponse, description = "Validation error (code=2002 empty messages)"),
    (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003)"),
    (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found / DB not found (code=5003)"),
    (status = 503, content_type = "application/json", body = ErrorResponse, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    ),
)]
pub async fn handle_chat_stream_doc(){}

pub async fn handle_chat_stream(
  claims:Claims,
  mut chat_id:Option<Path<Uuid>>,
  State(app_state): State<SharedState>,
  Json(req):Json<ChatInitRequest>
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>,AppError>{
 let provider = req.provider.clone().unwrap_or_else(|| "openai".to_string());
 let selected_tools = req.selected_tools.clone().unwrap_or_default();
 let web_search = req.web_search;
 let openai_settings = app_state
    .settings
    .openai
    .read()
    .await
    .clone();
 let anthropic_settings = app_state
    .settings
    .anthropic
    .read()
    .await
    .clone();
 // Select provider configuration and set default model
 let (provider_config, model_name) = match provider.to_lowercase().as_str() {
     "openai" => {
         let settings = openai_settings
             .as_ref()
             .ok_or(AppError::LlmProviderNotConfigured { provider:provider.clone() })?;
         if !settings.is_enabled{
            return Err(AppError::LlmProviderDisabledByAdmin {provider:provider.clone()});
         }
         let model = req.model_name.clone().unwrap_or_else(|| "gpt-5.2".to_string());
         (LlmProviderConfig::OpenAI(&settings), model)
     },
     "anthropic" => {
         let settings = anthropic_settings
           .as_ref()
           .ok_or(AppError::LlmProviderNotConfigured { provider:provider.clone() })?;
         if !settings.is_enabled{
            return Err(AppError::LlmProviderDisabledByAdmin {provider:provider.clone()});
         }
         let model = req.model_name.clone().unwrap_or_else(|| "claude-sonnet-4-5".to_string());
         (LlmProviderConfig::Anthropic(&settings), model)
     },
     _ => return Err(AppError::InvalidLlmProvider{provider:provider.clone()})
 };
 if let Some(conversation_id) = req.conversation_id{
    chat_id = Some(Path(conversation_id));
 }
 let mut metadata = json!({
    "webSearch":req.web_search,
    "selectedTools":selected_tools.clone()
 });
 let (conversation_id,mut previous_prompts) = if let Some(Path(conversation_id)) = chat_id {
    let (mut conversation, previous_messages) = conversations::Entity::find_by_id(conversation_id.clone())
       .filter(conversations::Column::ArchivedAt.is_null())
       .find_with_related(messages::Entity)
       .order_by_desc(messages::Column::CreatedAt)
       .filter(messages::Column::Deleted.eq(false))
       .all(&app_state.database)
       .await
       .map_err(|e| {
          eprintln!("DB get one with many error {:?}", e);
          AppError::DbTimeout})?
       .into_iter()
       .next()
       .ok_or(AppError::DbNotFound)?;
    if !selected_tools.is_empty(){
      if let Some(json) =  conversation.metadata.as_mut() {
          // Update metadata TODO
       }
      conversation
        .metadata
        .as_mut()
        .or(Some(&mut metadata));
      conversation.updated_at = Utc::now();
      conversation.message_count += req.messages.len() as i32;
    }
    conversation.last_message_at = Some(Utc::now());
    conversation
      .into_active_model()
      .update(&app_state.database)
      .await
      .map_err(|e| {
          eprintln!("Db update one error {:?}", e);
          AppError::DbTimeout})?;
   let previous_prompts = previous_messages
     .into_iter()
     .map(|message| Prompt {
        text: message.message_content,
        role: message.role,
        files: message
            .metadata
            .and_then(|json| json.get("files").cloned())
            .and_then(|files_val| serde_json::from_value::<Vec<File>>(files_val).ok())
            .unwrap_or_default(), // Vec::new()
    })
    .collect::<Vec<Prompt>>();
  (conversation_id,previous_prompts)
 }else{
  let first_prompt = req.messages
    .first()
    .map(|message| message.content.clone())
    .ok_or(AppError::ValidationEmptyField { field: "messages" })?;
  let new_conversation_id = Uuid::new_v4();
  let prompt_title_response = match &provider_config {
      LlmProviderConfig::OpenAI(settings) => {
          app_state.req_client
              .openai_get_title(settings, first_prompt)
              .await
      },
      LlmProviderConfig::Anthropic(settings) => {
          app_state.req_client
              .anthropic_get_title(settings, first_prompt)
              .await
      },
  }.map_err(|e| {
      eprintln!("title generation error {:?}", e);
      AppError::DbTimeout
  })?;
   let title_generation_usage  = json!({
       "model":get_title_generation_model(&provider),
       "inputTokens":prompt_title_response.input_tokens,
       "outputTokens":prompt_title_response.output_tokens,
  });
  let mut new_metadata = metadata.clone();
  new_metadata["titleGenerationUsage"] = title_generation_usage;
  let new_conversation = conversations::ActiveModel{ 
    id:Set(new_conversation_id.clone()),
    user_id:Set(claims.user_id),
    title: Set(Some(prompt_title_response.title)),
    model_provider:Set(provider.clone()),
    model_name:Set(model_name.clone()),
    created_at:Set(Utc::now()),
    updated_at: Set(Utc::now()),
    last_message_at:Set(Some(Utc::now())),
    archived_at:Set(None),
    message_count:Set(req.messages.len() as i32),
    total_tokens: Set(0),
    total_cost:Set(Decimal::from(0)),
    metadata:Set(Some(new_metadata))
   };
  new_conversation
    .insert(&app_state.database)
    .await
    .map_err(|e| {
       eprintln!("Db insert one error {:?}", e);
       AppError::DbTimeout})?;
    (new_conversation_id,Vec::new())
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
     deleted:Set(false),
     message_content:Set(message.content.clone()),
     model_provider:Set(provider.clone()),
     model_name:Set(model_name.clone()),
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
        eprintln!("Db one insert error {:?}", e);
        AppError::DbTimeout})?;
 }
 
 let current_prompts:Vec<Prompt> = req.messages
   .into_iter()
   .map(|message| 
      Prompt { text:message.content, role:message.role, files:message.files
    })
   .collect();
 previous_prompts.extend(current_prompts);
 // Create event source based on provider
 let mut event_source = match &provider_config {
     LlmProviderConfig::OpenAI(settings) => {
         app_state.req_client
             .openai_chat_stream(
                settings, model_name.clone(),
                req.temperature,
                previous_prompts,
                &claims.user_id,
                req.web_search
              )
             .await
     },
     LlmProviderConfig::Anthropic(settings) => {
         app_state.req_client
             .anthropic_chat_stream(
                 settings,
                 model_name.clone(),
                 ANTHROPIC_DEFAULT_MAX_TOKENS,
                 req.temperature,
                 previous_prompts,
                 web_search,
                 &claims.user_id,
              )
              .await
     },
 }.map_err(|e| {
     eprintln!("event source loading error {} for llm provider {}", e, &provider);
     AppError::ServiceTemporarilyUnavailable
 })?;
 // Create stream parser based on provider
 let stream_parser: Box<dyn StreamParser> = match provider_config {
     LlmProviderConfig::OpenAI(_) => Box::new(OpenaiStreamParser::new()),
     LlmProviderConfig::Anthropic(_) => Box::new(AnthropicStreamParser::new()),
 };

 let sse_stream = async_stream::try_stream! {
    let mut message_content = String::new();
    let mut request_tokens = 0;
    let mut response_tokens = 0;
    let mut total_tokens = 0;
    let mut request_id: Option<String> = None;

    while let Some(event) = event_source.next().await {
        match event {
            Ok(ReqwestEvent::Open) => {
                println!("SSE connection open for provider: {}", &provider);
            }
            Ok(ReqwestEvent::Message(msg)) => {
                let parse_result = stream_parser.parse_event(&msg.data);

                match &parse_result {
                    StreamParseResult::TextDelta { text, request_id: rid } => {
                        message_content.push_str(text);
                        if let Some(id) = rid {
                            request_id = Some(id.clone());
                        }
                        let chat_stream = ChatStream {
                            id: conversation_id.clone(),
                            role: None,
                            content: Some(text.clone()),
                        };
                        yield Event::default().event("chunk").data(chat_stream.to_string());
                    }
                    StreamParseResult::TokenUsage{ request_id:req_id,input_tokens, output_tokens, total_tokens:t_tokens} => {
                       if let Some(tokens) = input_tokens {
                         request_tokens = tokens.clone() as i32;
                       }
                       if let Some(tokens) = output_tokens {
                         response_tokens = tokens.clone() as i32;
                       }
                       if let Some(tokens) = t_tokens{
                         total_tokens = tokens.clone() as i32;
                       }
                       request_id = req_id.clone();
                    }
                    StreamParseResult::MessageStart { request_id:req_id,input_tokens,output_tokens} => {
                       if let Some(tokens) = input_tokens {
                         request_tokens = tokens.clone() as i32;
                       }
                       if let Some(tokens) = output_tokens {
                         response_tokens = tokens.clone() as i32;
                       }
                      request_id = Some(req_id.clone());
                    }
                    StreamParseResult::ToolInput { .. } => {
                        // Tool input streaming - accumulate for tool calls (future support)
                    }
                    StreamParseResult::Error { error_type, message } => {
                        eprintln!("Stream error: {} - {}", error_type, message);
                    }
                    StreamParseResult::None => {}
                }
            }
            Err(e) => {
                match e {
                  reqwest_eventsource::Error::StreamEnded => {
                      if total_tokens == 0 {
                        total_tokens = request_tokens + response_tokens;
                      }
                      println!("Stream ended for provider: {} input tokens: {} output_tokens: {} total_tokens: {}", &provider,request_tokens,response_tokens,total_tokens);
                      let new_llm_message = messages::ActiveModel {
                         id: Set(Uuid::new_v4()),
                         conversation_id: Set(conversation_id.clone()),
                         previous_message_id: Set(previous_message_id),
                         deleted: Set(false),
                         role: Set(ChatRole::Assistant),
                         message_content: Set(message_content),
                         model_provider: Set(provider.clone()),
                         model_name: Set(model_name.clone()),
                         request_id: Set(request_id),
                         request_tokens: Set(request_tokens),
                         response_tokens: Set(response_tokens),
                         tools_calls: Set(Vec::new()),
                         tools_results: Set(Vec::new()),
                         created_at: Set(Utc::now()),
                         updated_at: Set(Utc::now()),
                         total_tokens: Set(total_tokens),
                         latency: Set(0),
                         cost: Set(Decimal::from(0)),
                         metadata: Set(None),
                    };
                    new_llm_message
                        .insert(&app_state.database)
                        .await
                        .expect("failed to insert llm response in table messages");
                    yield Event::default().event("chunk").data("[DONE]");
                    break;
                    },
                    _ => {
                        println!("Streaming error for provider:{} error:{}",provider,e.to_string());
                        break;
                    }
                };
            }
        }
    }
 };
 let sse_response = Sse::new(sse_stream).keep_alive(KeepAlive::new());
 Ok(sse_response)
}
