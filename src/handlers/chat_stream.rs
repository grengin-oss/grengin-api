use std::{collections::HashMap, convert::Infallible};
use axum::{Json, extract::{Path, State}, response::{Sse, sse::{Event, KeepAlive}}};
use chrono::Utc;
use futures_util::StreamExt;
use num_traits::ToPrimitive;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, prelude::Decimal};
use serde::Serialize;
use serde_json::json;
use tokio::time::Instant;
use uuid::Uuid;
use rust_decimal::prelude::FromPrimitive;
use crate::{
    auth::{claims::Claims, error::AuthErrorResponse},
    config::setting::{AnthropicSettings, OpenaiSettings},
    dto::{
        chat_stream::{
            ChatInitRequest,
            ChatStream,
            ChatStreamEvents,
            ChatStreamEvent,
            ChatStreamPayload,
            ChatStreamToolCall,
            ChatStreamToolResult,
            ChatStreamWebSearchAction,
            ChatStreamWebSearchResult,
            ChatStreamWebSearchResultItem,
            ChatToolKind,
        },
        files::File,
        llm::anthropic::ANTHROPIC_DEFAULT_MAX_TOKENS,
    },
    error::{AppError, ErrorResponse},
    handlers::models::get_model_info_cached,
    handlers::llm::{StreamParseResult, StreamParser, anthropic::AnthropicStreamParser, openai::OpenaiStreamParser},
    llm::{prompt::Prompt, provider::{AnthropicApis, OpenaiApis, get_title_generation_model}},
    models::{conversations, departments::ActionOnExceed, messages::{self, ChatRole}, users},
    services::budget_allocation::{get_department_budget_status, refresh_department_budget_available},
    state::SharedState,
};
use reqwest_eventsource::Event as ReqwestEvent;

/// Provider configuration enum for handling different LLM providers
enum LlmProviderConfig<'a> {
    OpenAI(&'a OpenaiSettings),
    Anthropic(&'a AnthropicSettings),
}

#[derive(Debug, Serialize)]
struct BudgetWarningPayload {
    department_id: Uuid,
    budget_available: String,
    action: &'static str,
    message: String,
}

fn calculate_cost_decimal(
    input_tokens: i32,
    output_tokens: i32,
    input_rate: Option<f64>,
    output_rate: Option<f64>,
) -> Decimal {
    let input_rate = input_rate.unwrap_or(0.0);
    let output_rate = output_rate.unwrap_or(0.0);
    if input_rate == 0.0 && output_rate == 0.0 {
        return Decimal::from(0);
    }
    let cost = (input_tokens as f64 * input_rate + output_tokens as f64 * output_rate) / 1_000_000.0;
    Decimal::from_f64(cost).unwrap_or_else(|| Decimal::from(0))
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
    (status = 200, content_type = "text/event-stream", body = ChatStream,
      examples(
    ("conversation" = (
      description = "event:conversation",
      value = json!({
        "event": "conversation",
        "data": { "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "title": "...", "is_new": true }
      })
    )),
    ("message_start" = (
      description = "event:message_start",
      value = json!({
        "event": "message_start",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6" }
      })
    )),
    ("delta_1" = (
      description = "event:delta",
      value = json!({
        "event": "delta",
        "data": { "text": "Hello" }
      })
    )),
    ("delta_2" = (
      description = "event:delta",
      value = json!({
        "event": "delta",
        "data": { "text": " world" }
      })
    )),
    ("message_end" = (
      description = "event:message_end",
      value = json!({
        "event": "message_end",
        "data": { "input_tokens": 100, "output_tokens": 25, "latency_ms": 450 }
      })
    )),
    ("event" = (
      description = "event:event",
      value = json!({
        "event": "event",
        "data": { "event": { "event_type": "thinking_delta", "text": "Considering options..." } }
      })
    )),
    ("tool_call" = (
      description = "event:tool_call",
      value = json!({
        "event": "tool_call",
        "data": { "tool_call": { "tool_name": "web_search_call", "tool_id": "ws_123", "kind": "web_search", "web_search": { "query": "latest rust release" } } }
      })
    )),
    ("tool_result" = (
      description = "event:tool_result",
      value = json!({
        "event": "tool_result",
        "data": { "tool_result": { "tool_name": "web_search_call", "tool_id": "ws_123", "kind": "web_search", "web_search": { "query": "latest rust release", "results": [{ "title": "...", "url": "https://example.com" }] } } }
      })
    )),
    ("done" = (
      description = "event:done",
      value = json!({
        "event": "done",
        "data": {}
      })
    ))
   )
   ),
    (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
    (status = 400, content_type = "application/json", body = ErrorResponse, description = "Validation error (code=2002 empty messages)"),
    (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003) or budget exceeded (code=6001)"),
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
    (status = 200, content_type = "text/event-stream", body = ChatStream,
      examples(
    ("conversation" = (
      description = "event:conversation",
      value = json!({
        "event": "conversation",
        "data": { "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "title": "...", "is_new": true }
      })
    )),
    ("message_start" = (
      description = "event:message_start",
      value = json!({
        "event": "message_start",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6" }
      })
    )),
    ("delta_1" = (
      description = "event:delta",
      value = json!({
        "event": "delta",
        "data": { "text": "Hello" }
      })
    )),
    ("delta_2" = (
      description = "event:delta",
      value = json!({
        "event": "delta",
        "data": { "text": " world" }
      })
    )),
    ("message_end" = (
      description = "event:message_end",
      value = json!({
        "event": "message_end",
        "data": { "input_tokens": 100, "output_tokens": 25, "latency_ms": 450 }
      })
    )),
    ("event" = (
      description = "event:event",
      value = json!({
        "event": "event",
        "data": { "event": { "event_type": "thinking_delta", "text": "Considering options..." } }
      })
    )),
    ("tool_call" = (
      description = "event:tool_call",
      value = json!({
        "event": "tool_call",
        "data": { "tool_call": { "tool_name": "web_search_call", "tool_id": "ws_123", "kind": "web_search", "web_search": { "query": "latest rust release" } } }
      })
    )),
    ("tool_result" = (
      description = "event:tool_result",
      value = json!({
        "event": "tool_result",
        "data": { "tool_result": { "tool_name": "web_search_call", "tool_id": "ws_123", "kind": "web_search", "web_search": { "query": "latest rust release", "results": [{ "title": "...", "url": "https://example.com" }] } } }
      })
    )),
    ("done" = (
      description = "event:done",
      value = json!({
        "event": "done",
        "data": {}
      })
    ))
   )
   ),
    (status = 401, content_type = "application/json", body = AuthErrorResponse, description = "Invalid/expired token (code=6103)"),
    (status = 400, content_type = "application/json", body = ErrorResponse, description = "Validation error (code=2002 empty messages)"),
    (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003) or budget exceeded (code=6001)"),
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
 let start = Instant::now();
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
 let mut budget_warning: Option<BudgetWarningPayload> = None;
 let user = users::Entity::find_by_id(claims.user_id)
    .one(&app_state.database)
    .await
    .map_err(|e| {
        eprintln!("find user error: {e}");
        AppError::DbTimeout
    })?;
 if let Some(user) = user {
    if let Some(department_id) = user.department_id {
        let (budget_available, action_on_exceed) =
            get_department_budget_status(&app_state.database, department_id)
                .await
                .map_err(|e| {
                    eprintln!("get department budget status error: {e}");
                    AppError::DbTimeout
                })?;
        if budget_available.to_f32() <= Some(0_f32) {
           
            match action_on_exceed {
                ActionOnExceed::Block => {
                    return Err(AppError::DepartmentBudgetExceeded);
                }
                ActionOnExceed::Warn => {
                    budget_warning = Some(BudgetWarningPayload {
                        department_id,
                        budget_available: budget_available.to_string(),
                        action: "warn",
                        message:
                            "Department budget is exhausted for the current period. The chat will proceed, but usage may exceed the budget."
                                .to_string(),
                    });
                }
            }
        }
    }
 }

 let (input_rate, output_rate) = match get_model_info_cached(&app_state.req_client, &model_name).await {
    Ok(Some(model)) => (model.input_token_rate, model.output_token_rate),
    Ok(None) => (None, None),
    Err(error) => {
        eprintln!("models cache error: {error}");
        (None, None)
    }
 };
 if let Some(conversation_id) = req.conversation_id{
    chat_id = Some(Path(conversation_id));
 }
 let mut metadata = json!({
    "webSearch":req.web_search,
    "selectedTools":selected_tools.clone()
 });
 let (conversation_id,mut previous_prompts,title) = if let Some(Path(conversation_id)) = chat_id {
    let (mut conversation, previous_messages) = conversations::Entity::find_by_id(conversation_id.clone())
       .filter(conversations::Column::ArchivedAt.is_null())
       .find_with_related(messages::Entity)
       .order_by_asc(messages::Column::CreatedAt)
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
  (conversation_id,previous_prompts,None)
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
      AppError::LlmProviderNotConfigured { provider:provider.clone() }
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
    title: Set(Some(prompt_title_response.title.clone())),
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
    (new_conversation_id,Vec::new(),Some(prompt_title_response.title))
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
     latency:Set(start.elapsed().as_millis() as i32),
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
     AppError::LlmProviderNotConfigured { provider:provider.clone() }
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
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    let mut tool_results: Vec<serde_json::Value> = Vec::new();
    let mut tool_call_by_index: HashMap<u32, (String, Option<String>)> = HashMap::new();
    let mut seen_tool_call_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut web_search_state: HashMap<String, ChatStreamWebSearchResult> = HashMap::new();
    let mut last_web_search_call_id: Option<String> = None;
    let latency = start
      .elapsed()
      .as_millis() as i32;
     let first_chat_stream = ChatStream{
       id: Some(conversation_id.clone()),
       title:title.clone(),
       message_id:None,
       is_new:Some(title.is_some()),
       content:None,
       input_tokens:None,
       output_tokens:None,
       latency_ms:None,
       cost:None,
       event:None,
       tool_call:None,
       tool_result:None,
     };
     yield Event::default().event(ChatStreamEvents::Conversation.to_string()).data(first_chat_stream.to_string());
     let mut budget_warning = budget_warning;
     if let Some(payload) = budget_warning.take() {
        let data = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
        yield Event::default().event(ChatStreamEvents::DepartmentBudgetWarning.to_string()).data(data);
     }
     let new_message_id = Uuid::new_v4();
     let mut new_llm_message = messages::ActiveModel {
        id: Set(new_message_id.clone()),
        conversation_id: Set(conversation_id.clone()),
        previous_message_id: Set(previous_message_id),
        deleted: Set(false),
        role: Set(ChatRole::Assistant),
        message_content: Set(message_content.clone()),
        model_provider: Set(provider.clone()),
        model_name: Set(model_name.clone()),
        request_id: Set(request_id.clone()),
        request_tokens: Set(request_tokens),
        response_tokens: Set(response_tokens),
        tools_calls: Set(Vec::new()),
        tools_results: Set(Vec::new()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
        total_tokens: Set(total_tokens),
        latency: Set(latency),
        cost: Set(Decimal::from(0)),
        metadata: Set(None),
      };
     new_llm_message
          .clone()
          .insert(&app_state.database)
          .await
          .expect("failed to insert llm response in table messages");

    while let Some(event) = event_source.next().await {
        match event {
            Ok(ReqwestEvent::Open) => {
                println!("SSE connection open for provider: {}", &provider);
            }
            Ok(ReqwestEvent::Message(msg)) => {
                if provider.to_lowercase() == "openai" && std::env::var("OPENAI_STREAM_DEBUG").as_deref() == Ok("1") {
                    println!("openai raw event: {}", msg.data);
                }
                let parse_result = stream_parser.parse_event(&msg.data);
                match &parse_result {
                    StreamParseResult::TextDelta { text, request_id: rid } => {
                        message_content.push_str(text);
                        request_id = rid.clone();
                        new_llm_message.request_id = Set(request_id);
                        new_llm_message.updated_at = Set(Utc::now());
                        new_llm_message.message_content = Set(message_content.clone());
                        new_llm_message.request_tokens = Set(request_tokens);
                        new_llm_message.response_tokens = Set(response_tokens);
                        new_llm_message.total_tokens = Set(request_tokens + response_tokens);
                        new_llm_message.cost = Set(calculate_cost_decimal(
                            request_tokens,
                            response_tokens,
                            input_rate,
                            output_rate,
                        ));
                        new_llm_message
                          .clone()
                          .update(&app_state.database)
                          .await
                          .expect("failed to update in new llm response in table messages");
                        let chat_stream = ChatStream {
                            id:None,
                            title:None,
                            message_id:None,
                            is_new:None,
                            content: Some(text.clone()),
                            input_tokens:None,
                            output_tokens:None,
                            latency_ms:None,
                            cost:None,
                            event:None,
                            tool_call:None,
                            tool_result:None,
                        };
                        yield Event::default().event(ChatStreamEvents::Delta.to_string()).data(chat_stream.to_string());
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
                       let cost = calculate_cost_decimal(
                           request_tokens,
                           response_tokens,
                           input_rate,
                           output_rate,
                       );
                       new_llm_message.request_id = Set(request_id);
                       new_llm_message.updated_at = Set(Utc::now());
                       new_llm_message.request_tokens = Set(request_tokens);
                       new_llm_message.response_tokens = Set(response_tokens);
                       new_llm_message.total_tokens = Set(request_tokens + response_tokens);
                       new_llm_message.cost = Set(cost);
                       new_llm_message
                         .clone()
                         .update(&app_state.database)
                         .await
                         .expect("failed to update in new llm response in table messages");
                       let message_end = ChatStream {
                            id: None,
                            title:None,
                            message_id:None,
                            is_new:None,
                            content:None,
                            input_tokens:Some(request_tokens),
                            output_tokens:Some(response_tokens),
                            latency_ms:Some(latency),
                            cost:cost.to_f32(),
                            event:None,
                            tool_call:None,
                            tool_result:None,
                      };
                      yield Event::default().event(ChatStreamEvents::MessageEnd.to_string()).data(message_end.to_string());
                    }
                    StreamParseResult::MessageStart { request_id:req_id,input_tokens,output_tokens} => {
                       if let Some(tokens) = input_tokens {
                         request_tokens = tokens.clone() as i32;
                       }
                       if let Some(tokens) = output_tokens {
                         response_tokens = tokens.clone() as i32;
                       }
                       let message_start = ChatStream{
                         id:None,
                         title:None,
                         message_id:Some(new_message_id),
                         is_new:None,
                         content:None,
                         input_tokens:Some(request_tokens),
                         output_tokens:None,
                         latency_ms:None,
                         cost:None,
                         event:None,
                         tool_call:None,
                         tool_result:None,
                       };
                       yield Event::default().event(ChatStreamEvents::MessageStart.to_string()).data(message_start.to_string());
                       request_id = Some(req_id.clone());
                       new_llm_message.request_id = Set(request_id);
                       new_llm_message.updated_at = Set(Utc::now());
                       new_llm_message.request_tokens = Set(request_tokens);
                       new_llm_message.response_tokens = Set(response_tokens);
                       new_llm_message.total_tokens = Set(request_tokens + response_tokens);
                       new_llm_message.cost = Set(calculate_cost_decimal(
                           request_tokens,
                           response_tokens,
                           input_rate,
                           output_rate,
                       ));
                       new_llm_message
                         .clone()
                         .update(&app_state.database)
                         .await
                         .expect("failed to update in new llm response in table messages");
                    }
                    StreamParseResult::ToolInput { partial_json, index } => {
                        let mut tool_name = None;
                        let mut tool_id = None;
                        if let Some(idx) = index {
                            if let Some((name, id)) = tool_call_by_index.get(&idx) {
                                tool_name = Some(name.clone());
                                tool_id = id.clone();
                            }
                        }
                        let resolved_name = tool_name.clone().unwrap_or_else(|| "tool_call".to_string());
                        let is_web_search = resolved_name.contains("web_search");
                        let tool_call = ChatStreamToolCall {
                            tool_name: resolved_name,
                            tool_id: tool_id.clone(),
                            input_text: Some(partial_json.clone()),
                            kind: Some(if is_web_search { ChatToolKind::WebSearch } else { ChatToolKind::Other }),
                            web_search: None,
                        };
                        tool_calls.push(serde_json::to_value(&tool_call).unwrap_or_else(|_| json!({})));
                        new_llm_message.tools_calls = Set(tool_calls.clone());
                        new_llm_message.updated_at = Set(Utc::now());
                        new_llm_message
                          .clone()
                          .update(&app_state.database)
                          .await
                          .expect("failed to update tool input in new llm response in table messages");
                        let chat_stream = ChatStream {
                            id:None,
                            title:None,
                            message_id:None,
                            is_new:None,
                            content:None,
                            input_tokens:None,
                            output_tokens:None,
                            latency_ms:None,
                            cost:None,
                            event:None,
                            tool_call:Some(tool_call),
                            tool_result:None,
                        };
                        yield Event::default().event(ChatStreamEvents::ToolCall.to_string()).data(chat_stream.to_string());
                    }
                    StreamParseResult::EventLog { event_type, message, data } => {
                        let event = ChatStreamEvent {
                            event_type:event_type.clone(),
                            text: message.clone(),
                            data: data.as_ref().map(|value| ChatStreamPayload { value:value.clone() }),
                        };
                        let chat_stream = ChatStream {
                            id:None,
                            title:None,
                            message_id:None,
                            is_new:None,
                            content:None,
                            input_tokens:None,
                            output_tokens:None,
                            latency_ms:None,
                            cost:None,
                            event:Some(event),
                            tool_call:None,
                            tool_result:None,
                        };
                        yield Event::default().event(ChatStreamEvents::Event.to_string()).data(chat_stream.to_string());
                    }
                    StreamParseResult::ToolCall { tool_name, tool_id, input, index, .. } => {
                        if let Some(idx) = index {
                            tool_call_by_index.insert(*idx, (tool_name.clone(), tool_id.clone()));
                        }
                        if let Some(id) = tool_id.as_ref() {
                            if !seen_tool_call_ids.insert(id.clone()) {
                                continue;
                            }
                        }
                        let is_web_search = tool_name.contains("web_search");
                        if is_web_search {
                            last_web_search_call_id = tool_id.clone().or_else(|| last_web_search_call_id.clone());
                        }
                        let web_search = if is_web_search {
                            input.as_ref().and_then(|value| {
                                let query = value
                                    .get("query")
                                    .and_then(|q| q.as_str())
                                    .map(|s| s.to_string());
                                let queries = value
                                    .get("queries")
                                    .and_then(|q| q.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                            .collect::<Vec<String>>()
                                    });
                                if query.is_none() && queries.is_none() {
                                    return None;
                                }
                                Some(ChatStreamWebSearchAction { query, queries })
                            })
                        } else {
                            None
                        };
                        let tool_call = ChatStreamToolCall {
                            tool_name: tool_name.clone(),
                            tool_id: tool_id.clone(),
                            input_text: None,
                            kind: Some(if is_web_search { ChatToolKind::WebSearch } else { ChatToolKind::Other }),
                            web_search,
                        };
                        tool_calls.push(serde_json::to_value(&tool_call).unwrap_or_else(|_| json!({})));
                        new_llm_message.tools_calls = Set(tool_calls.clone());
                        new_llm_message.updated_at = Set(Utc::now());
                        new_llm_message
                          .clone()
                          .update(&app_state.database)
                          .await
                          .expect("failed to update tool calls in new llm response in table messages");
                        let chat_stream = ChatStream {
                            id:None,
                            title:None,
                            message_id:None,
                            is_new:None,
                            content:None,
                            input_tokens:None,
                            output_tokens:None,
                            latency_ms:None,
                            cost:None,
                            event:None,
                            tool_call:Some(tool_call),
                            tool_result:None,
                        };
                        yield Event::default().event(ChatStreamEvents::ToolCall.to_string()).data(chat_stream.to_string());
                    }
                    StreamParseResult::WebSearchAction { tool_name, tool_id, query, queries } => {
                        let call_id = tool_id.clone().or_else(|| last_web_search_call_id.clone());
                        if let Some(id) = call_id.clone() {
                            last_web_search_call_id = Some(id.clone());
                            let entry = web_search_state.entry(id.clone()).or_insert_with(|| ChatStreamWebSearchResult {
                                query: None,
                                queries: None,
                                results: Vec::new(),
                            });
                            if query.is_some() {
                                entry.query = query.clone();
                            }
                            if queries.is_some() {
                                entry.queries = queries.clone();
                            }
                            let tool_result = ChatStreamToolResult {
                                tool_name: Some("web_search_call".to_string()),
                                tool_id: Some(id.clone()),
                                kind: Some(ChatToolKind::WebSearch),
                                web_search: Some(entry.clone()),
                            };
                            tool_results.push(serde_json::to_value(&tool_result).unwrap_or_else(|_| json!({})));
                            new_llm_message.tools_results = Set(tool_results.clone());
                            new_llm_message.updated_at = Set(Utc::now());
                            new_llm_message
                              .clone()
                              .update(&app_state.database)
                              .await
                              .expect("failed to update tool results in new llm response in table messages");
                            let chat_stream = ChatStream {
                                id:None,
                                title:None,
                                message_id:None,
                                is_new:None,
                                content:None,
                                input_tokens:None,
                                output_tokens:None,
                                latency_ms:None,
                                cost:None,
                                event:None,
                                tool_call:None,
                                tool_result:Some(tool_result),
                            };
                            yield Event::default().event(ChatStreamEvents::ToolResult.to_string()).data(chat_stream.to_string());
                        }
                    }
                    StreamParseResult::WebSearchResult { tool_name: _, tool_id, results } => {
                        let call_id = tool_id.clone().or_else(|| last_web_search_call_id.clone());
                        if let Some(id) = call_id.clone() {
                            last_web_search_call_id = Some(id.clone());
                            let entry = web_search_state.entry(id.clone()).or_insert_with(|| ChatStreamWebSearchResult {
                                query: None,
                                queries: None,
                                results: Vec::new(),
                            });
                            for result in results {
                                entry.results.push(ChatStreamWebSearchResultItem {
                                    title: result.title.clone(),
                                    url: result.url.clone(),
                                    source: result.source.clone(),
                                    page_age: result.page_age.clone(),
                                    snippet: result.snippet.clone(),
                                });
                            }
                            let tool_result = ChatStreamToolResult {
                                tool_name: Some("web_search_call".to_string()),
                                tool_id: Some(id.clone()),
                                kind: Some(ChatToolKind::WebSearch),
                                web_search: Some(entry.clone()),
                            };
                            tool_results.push(serde_json::to_value(&tool_result).unwrap_or_else(|_| json!({})));
                            new_llm_message.tools_results = Set(tool_results.clone());
                            new_llm_message.updated_at = Set(Utc::now());
                            new_llm_message
                              .clone()
                              .update(&app_state.database)
                              .await
                              .expect("failed to update tool results in new llm response in table messages");
                            let chat_stream = ChatStream {
                                id:None,
                                title:None,
                                message_id:None,
                                is_new:None,
                                content:None,
                                input_tokens:None,
                                output_tokens:None,
                                latency_ms:None,
                                cost:None,
                                event:None,
                                tool_call:None,
                                tool_result:Some(tool_result),
                            };
                            yield Event::default().event(ChatStreamEvents::ToolResult.to_string()).data(chat_stream.to_string());
                        }
                    }
                    StreamParseResult::ToolResult { tool_name, tool_id, .. } => {
                        let is_web_search = tool_name
                            .as_ref()
                            .map(|name| name.contains("web_search"))
                            .unwrap_or(false);
                        let tool_result = ChatStreamToolResult {
                            tool_name: tool_name.clone(),
                            tool_id: tool_id.clone(),
                            kind: Some(if is_web_search { ChatToolKind::WebSearch } else { ChatToolKind::Other }),
                            web_search: None,
                        };
                        tool_results.push(serde_json::to_value(&tool_result).unwrap_or_else(|_| json!({})));
                        new_llm_message.tools_results = Set(tool_results.clone());
                        new_llm_message.updated_at = Set(Utc::now());
                        new_llm_message
                          .clone()
                          .update(&app_state.database)
                          .await
                          .expect("failed to update tool results in new llm response in table messages");
                        let chat_stream = ChatStream {
                            id:None,
                            title:None,
                            message_id:None,
                            is_new:None,
                            content:None,
                            input_tokens:None,
                            output_tokens:None,
                            latency_ms:None,
                            cost:None,
                            event:None,
                            tool_call:None,
                            tool_result:Some(tool_result),
                        };
                        yield Event::default().event(ChatStreamEvents::ToolResult.to_string()).data(chat_stream.to_string());
                    }
                    StreamParseResult::Error { error_type, message } => {
                      new_llm_message.message_content = Set(message.clone());
                      new_llm_message.role = Set(ChatRole::System);
                      new_llm_message.updated_at = Set(Utc::now());
                      new_llm_message
                        .clone()
                        .update(&app_state.database)
                        .await
                        .expect("failed to update in new llm response in table messages");
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
                      let message_cost = calculate_cost_decimal(
                        request_tokens,
                        response_tokens,
                        input_rate,
                        output_rate,
                      );
                      println!("Stream ended for provider: {} input tokens: {} output_tokens: {} total_tokens: {} latency in ms: {} cost: {}", &provider,request_tokens,response_tokens,total_tokens,latency,&message_cost);
                    new_llm_message.latency = Set(latency);
                    new_llm_message.request_tokens = Set(request_tokens);
                    new_llm_message.response_tokens = Set(response_tokens);
                    new_llm_message.total_tokens = Set(total_tokens);
                    new_llm_message.cost = Set(message_cost);
                    new_llm_message.updated_at = Set(Utc::now());
                    new_llm_message
                      .clone()
                      .update(&app_state.database)
                      .await
                      .expect("failed to update llm response in table messages");
                    if let Ok(Some(conversation)) = conversations::Entity::find_by_id(conversation_id.clone())
                      .one(&app_state.database)
                      .await
                     {
                      let mut active_conversation = conversation.clone().into_active_model();
                      active_conversation.total_tokens = Set(conversation.total_tokens + total_tokens as i64);
                      active_conversation.total_cost = Set(conversation.total_cost + message_cost);
                      active_conversation.updated_at = Set(Utc::now());
                      let _ = active_conversation.update(&app_state.database).await;
                    }
                    if let Ok(Some(user)) = users::Entity::find_by_id(claims.user_id)
                        .one(&app_state.database)
                        .await
                    {
                        if let Some(department_id) = user.department_id {
                            if let Err(e) =
                                refresh_department_budget_available(&app_state.database, department_id).await
                            {
                                eprintln!("refresh budget available error: {e}");
                            }
                        }
                    }
                    yield Event::default().event(ChatStreamEvents::Done.to_string()).data("{}");
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
