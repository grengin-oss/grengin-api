use crate::{
    auth::{claims::Claims, error::Error},
    config::setting::{AnthropicSettings, GeminiSettings, MistralSettings, OpenaiSettings},
    dto::{
        chat_stream::{
            BudgetWarningPayload, ChatInput, ChatStream, ChatStreamEvent,
            ChatStreamEvents, ChatStreamPayload, ChatStreamToolCall, ChatStreamToolResult,
            ChatStreamWebSearchAction, ChatToolKind,
        },
        files::File,
        llm::anthropic::{
            ANTHROPIC_DEFAULT_MAX_TOKENS, AnthropicContentBlock, AnthropicMessage, AnthropicRole,
            AnthropicTool, AnthropicToolUnion, AnthropicWebSearchTool,
        },
        llm::gemini::{normalize_gemini_parameters, prompts_to_gemini_payload},
        llm::mistral::{MistralMessage, MistralTool, MistralToolCall, MistralToolDefinition},
        llm::openai::{OpenaiFunctionCallOutput, OpenaiInputItem, OpenaiTool},
    },
    error::{AppError, ErrorDetailVariant, ErrorResponse},
    handlers::llm::{
        StreamParseResult, StreamParser, StreamWebSearchAction as ParsedWebSearchAction,
        StreamWebSearchState, ToolCall, ToolInput, anthropic::AnthropicStreamParser,
        gemini::GeminiStreamParser, mistral::MistralStreamParser,
        mistral_conversations::MistralConversationStreamParser, openai::OpenaiStreamParser,
        update_web_search_action_state, update_web_search_results_state,
    },
    handlers::models::get_model_info_cached,
    llm::{
        prompt::Prompt,
        provider::{
            AnthropicApis, GeminiApis, MistralApis, OpenaiApis, get_title_generation_model,
        },
        tooling::mcp_server_short_id,
    },
    models::{
        conversation_projects, conversations,
        departments::ActionOnExceed,
        mcp_access_policies::McpPermission,
        mcp_executions, mcp_oauth_states, mcp_servers,
        mcp_servers::McpTransportType,
        messages::{self, ChatRole},
        projects, users,
    },
    services::{
        artifacts::{extract_artifacts, filter_artifact_chunk},
        budget_allocation::{get_department_budget_status, refresh_department_budget_available},
        department_policies::check_model_allowed,
        mcp_client::build_authorization_url,
        mcp_helpers::{build_oauth_config, resolve_mcp_oauth_token},
        mcp_tools::{
            McpServerSummary, McpToolDescriptor, load_auto_mcp_server_ids, load_openai_mcp_tools,
        },
        notifications::emit_budget_alerts,
        rag::{
            EmbeddingTarget, assemble_prompts_with_budget, build_retrieval_prompt, embed_messages,
            load_recent_prompts, load_summary, update_conversation_summary,
        },
        system_prompts,
    },
    state::SharedState,
    utils::chat_stream::{
        to_chat_tool_input, to_chat_web_search_result, tool_input_to_value,
        tool_result_status_from_output,
    },
};
use axum::{
    Json,
    extract::{Path, State},
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use chrono::{Duration, Utc};
use futures_util::StreamExt;
use num_traits::ToPrimitive;
use reqwest::StatusCode;
use reqwest_eventsource::Event as ReqwestEvent;
use rust_decimal::prelude::FromPrimitive;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, QuerySelect, prelude::Decimal,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, convert::Infallible};
use tokio::time::Instant;
use uuid::Uuid;

/// Provider configuration enum for handling different LLM providers
enum LlmProviderConfig {
    OpenAI(OpenaiSettings),
    Anthropic(AnthropicSettings),
    Mistral(MistralSettings),
    Gemini(GeminiSettings),
}

#[derive(Clone)]
struct McpOauthPrompt {
    authorization_url: String,
    server_name: String,
}

#[derive(Serialize)]
struct McpOauthRequiredEvent {
    text: String,
    server_id: Uuid,
    server_name: String,
    authorization_url: String,
    tool_name: String,
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct McpOauthErrorPayload {
    error: String,
    is_error: bool,
    authorization_url: String,
    server_id: Uuid,
    server_name: String,
}


#[derive(Deserialize)]
struct LlmErrorObject {
    #[serde(rename = "type")]
    kind: Option<String>,
    code: Option<String>,
}

#[derive(Deserialize)]
struct LlmErrorEnvelope {
    #[serde(rename = "type")]
    kind: Option<String>,
    error: Option<LlmErrorObject>,
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
    let cost =
        (input_tokens as f64 * input_rate + output_tokens as f64 * output_rate) / 1_000_000.0;
    Decimal::from_f64(cost).unwrap_or_else(|| Decimal::from(0))
}

fn build_mcp_server_context(
    servers: &[McpServerSummary],
    tool_lookup: &HashMap<String, McpToolDescriptor>,
) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    lines.push("MCP servers selected for this request:".to_string());
    for server in servers {
        let short_id = mcp_server_short_id(&server.server_id);
        let mut line = format!(
            "- {} (id: {}, short: {})",
            server.name, server.server_id, short_id
        );
        if let Some(description) = server
            .description
            .as_ref()
            .map(|text| text.trim())
            .filter(|text| !text.is_empty())
        {
            line.push_str(&format!(" — {}", description));
        }
        lines.push(line);
    }
    lines.push(
        "Tools starting with mcp__<short>__ map to the corresponding server above.".to_string(),
    );
    let has_sqlx_query_tool = tool_lookup.values().any(|tool| {
        tool.original_name == "sql_query" && tool.server_name.to_ascii_lowercase().contains("sqlx")
    });
    if has_sqlx_query_tool {
        lines.push("SQL dialect hint: The sqlx MCP server uses PostgreSQL. Do not use sqlite_master, PRAGMA, or SHOW TABLES.".to_string());
        lines.push(
            "Use PostgreSQL catalog queries such as information_schema.tables when listing tables."
                .to_string(),
        );
    }
    Some(lines.join("\n"))
}

fn resolve_mcp_tool_descriptor<'a>(
    lookup: &'a HashMap<String, McpToolDescriptor>,
    tool_name: &str,
) -> Option<&'a McpToolDescriptor> {
    if let Some(found) = lookup.get(tool_name) {
        return Some(found);
    }
    let is_mcp_name = tool_name.starts_with("mcp__");
    let full_prefix = if is_mcp_name {
        Some(format!("{tool_name}__"))
    } else {
        None
    };
    let truncated_prefix = if is_mcp_name {
        tool_name
            .rsplit_once("__")
            .map(|(prefix, _)| format!("{prefix}__"))
    } else {
        None
    };

    let matches: Vec<&McpToolDescriptor> = lookup
        .iter()
        .filter_map(|(name, descriptor)| {
            if !is_mcp_name && descriptor.original_name == tool_name {
                return Some(descriptor);
            }
            if !is_mcp_name {
                return None;
            }

            // Providers may emit a non-canonical function name without the hash suffix,
            // or with a truncated hash. Match by stable MCP prefix in those cases.
            if full_prefix
                .as_ref()
                .map(|prefix| name.starts_with(prefix))
                .unwrap_or(false)
            {
                return Some(descriptor);
            }
            if truncated_prefix
                .as_ref()
                .map(|prefix| name.starts_with(prefix))
                .unwrap_or(false)
            {
                return Some(descriptor);
            }
            None
        })
        .collect();

    if matches.len() == 1 {
        matches.first().copied()
    } else {
        None
    }
}

fn is_rate_limit_error(body: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<LlmErrorEnvelope>(body) else {
        return false;
    };
    if parsed.kind.as_deref() == Some("rate_limit_error") {
        return true;
    }
    if let Some(error) = parsed.error {
        if error.kind.as_deref() == Some("rate_limit_error") {
            return true;
        }
        if error.code.as_deref() == Some("rate_limit_error") {
            return true;
        }
    }
    false
}

async fn build_mcp_oauth_prompt(
    state: &SharedState,
    server_id: Uuid,
    user_id: Uuid,
) -> Result<McpOauthPrompt, AppError> {
    let server = mcp_servers::Entity::find_by_id(server_id)
        .one(&state.database)
        .await
        .map_err(|e| {
            eprintln!("mcp server lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::McpServerNotFound)?;

    if !server.enabled {
        return Err(AppError::McpServerNotFound);
    }

    if !matches!(
        server.transport_type,
        McpTransportType::Http | McpTransportType::Sse
    ) {
        return Err(AppError::ServiceTemporarilyUnavailable);
    }

    let oauth_config = build_oauth_config(state, &server)?;
    let authorization = build_authorization_url(&oauth_config).map_err(|e| {
        eprintln!("mcp oauth authorize url error: {e}");
        AppError::ServiceTemporarilyUnavailable
    })?;

    let now = Utc::now();
    let expires_at = now + Duration::minutes(10);
    let model = mcp_oauth_states::ActiveModel {
        id: Set(Uuid::new_v4()),
        server_id: Set(server_id),
        user_id: Set(user_id),
        state: Set(authorization.state.clone()),
        pkce_verifier: Set(authorization.pkce_verifier.clone()),
        redirect_uri: Set(None),
        expires_at: Set(Some(expires_at)),
        created_at: Set(now),
    };
    model.insert(&state.database).await.map_err(|e| {
        eprintln!("mcp oauth state insert error: {e}");
        AppError::DbTimeout
    })?;

    Ok(McpOauthPrompt {
        authorization_url: authorization.authorization_url,
        server_name: server.name,
    })
}

#[utoipa::path(
    post,
    path = "/chat/stream/{chat_id}",
    tag = "chat",
    params(
        ("chat_id" = Option<Uuid>, Path, description = "Optional Chat id to stream messages for exiting chat"),
    ),
    request_body = ChatInput,
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
    ("artifact" = (
      description = "event:artifact — emitted when the response contains a standalone document (HTML page, Markdown file, etc.). Sent before event:done. Only fired when the conversation is linked to a project.",
      value = json!({
        "event": "artifact",
        "data": {
          "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
          "title": "Hello World HTML Page",
          "contentType": "text/html",
          "content": "<!DOCTYPE html><html><body><h1>Hello World</h1></body></html>"
        }
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
    (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
    (status = 400, content_type = "application/json", body = ErrorResponse, description = "Validation error (code=2002 empty messages)"),
    (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003) or budget exceeded (code=6001)"),
    (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found / DB not found (code=5003)"),
    (status = 503, content_type = "application/json", body = ErrorResponse, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),

    ),
)]
pub async fn handle_chat_stream_path_doc() {}

#[utoipa::path(
    post,
    path = "/chat/stream",
    tag = "chat",
    request_body = ChatInput,
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
    ("artifact" = (
      description = "event:artifact — emitted when the response contains a standalone document (HTML page, Markdown file, etc.). Sent before event:done. Only fired when the conversation is linked to a project.",
      value = json!({
        "event": "artifact",
        "data": {
          "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
          "title": "Hello World HTML Page",
          "contentType": "text/html",
          "content": "<!DOCTYPE html><html><body><h1>Hello World</h1></body></html>"
        }
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
    (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
    (status = 400, content_type = "application/json", body = ErrorResponse, description = "Validation error (code=2002 empty messages)"),
    (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003) or budget exceeded (code=6001)"),
    (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found / DB not found (code=5003)"),
    (status = 503, content_type = "application/json", body = ErrorResponse, description = "DB timeout/unavailable (code=5001/5000) or service temporarily unavailable (code=1000)"),
    ),
)]
pub async fn handle_chat_stream_doc() {}

#[utoipa::path(
    post,
    path = "/chat/stream/{message_id}/cancel",
    tag = "chat",
    params(
        ("message_id" = Uuid, Path, description = "Assistant message id to cancel")
    ),
    responses(
        (status = 202),
        (status = 204),
        (status = 401, content_type = "application/json", body = Error),
        (status = 404, content_type = "application/json", body = ErrorResponse),
        (status = 503, content_type = "application/json", body = ErrorResponse),
    )
)]
pub async fn cancel_chat_stream(
    claims: Claims,
    State(app_state): State<SharedState>,
    Path(message_id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    let message = messages::Entity::find_by_id(message_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("cancel stream message lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;

    let conversation = conversations::Entity::find_by_id(message.conversation_id)
        .one(&app_state.database)
        .await
        .map_err(|e| {
            eprintln!("cancel stream conversation lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;

    if conversation.user_id != claims.user_id {
        return Err(AppError::ResourceNotFound);
    }

    if app_state.cancel_stream(message_id).await {
        Ok(StatusCode::ACCEPTED)
    } else {
        Ok(StatusCode::NO_CONTENT)
    }
}

pub async fn handle_chat_stream(
    claims: Claims,
    mut chat_id: Option<Path<Uuid>>,
    State(app_state): State<SharedState>,
    Json(req): Json<ChatInput>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, AppError> {
    let start = Instant::now();
    let provider = req.provider.clone().unwrap_or_else(|| "openai".to_string());
    let selected_tools = req.selected_tools.clone().unwrap_or_default();
    let request_selected_mcp_servers = req.selected_mcp_servers.clone().unwrap_or_default();
    let mut selected_mcp_servers = request_selected_mcp_servers.clone();
    let web_search = req.web_search;
    let openai_settings = app_state.settings.openai.read().await.clone();
    let anthropic_settings = app_state.settings.anthropic.read().await.clone();
    let mistral_settings = app_state.settings.mistral.read().await.clone();
    let gemini_settings = app_state.settings.gemini.read().await.clone();
    // Select provider configuration and set default model
    let (provider_config, model_name) = match provider.to_lowercase().as_str() {
        "openai" => {
            let settings = openai_settings
                .clone()
                .ok_or(AppError::LlmProviderNotConfigured {
                    provider: provider.clone(),
                })?;
            if !settings.is_enabled {
                return Err(AppError::LlmProviderDisabledByAdmin {
                    provider: provider.clone(),
                });
            }
            let model = req
                .model_name
                .clone()
                .unwrap_or_else(|| "gpt-5.2".to_string());
            (LlmProviderConfig::OpenAI(settings), model)
        }
        "anthropic" => {
            let settings =
                anthropic_settings
                    .clone()
                    .ok_or(AppError::LlmProviderNotConfigured {
                        provider: provider.clone(),
                    })?;
            if !settings.is_enabled {
                return Err(AppError::LlmProviderDisabledByAdmin {
                    provider: provider.clone(),
                });
            }
            let model = req
                .model_name
                .clone()
                .unwrap_or_else(|| "claude-sonnet-4-5".to_string());
            (LlmProviderConfig::Anthropic(settings), model)
        }
        "mistral" => {
            let settings = mistral_settings
                .clone()
                .ok_or(AppError::LlmProviderNotConfigured {
                    provider: provider.clone(),
                })?;
            if !settings.is_enabled {
                return Err(AppError::LlmProviderDisabledByAdmin {
                    provider: provider.clone(),
                });
            }
            let model = req
                .model_name
                .clone()
                .unwrap_or_else(|| "mistral-small-latest".to_string());
            (LlmProviderConfig::Mistral(settings), model)
        }
        "gemini" => {
            let settings = gemini_settings
                .clone()
                .ok_or(AppError::LlmProviderNotConfigured {
                    provider: provider.clone(),
                })?;
            if !settings.is_enabled {
                return Err(AppError::LlmProviderDisabledByAdmin {
                    provider: provider.clone(),
                });
            }
            let model = req
                .model_name
                .clone()
                .unwrap_or_else(|| "gemini-2.5-flash".to_string());
            (LlmProviderConfig::Gemini(settings), model)
        }
        _ => {
            return Err(AppError::InvalidLlmProvider {
                provider: provider.clone(),
            });
        }
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
        let allowed = check_model_allowed(
            &app_state.database,
            user.department_id,
            &provider,
            &model_name,
        )
        .await
        .map_err(|e| {
            eprintln!("department model check error: {e}");
            AppError::DbTimeout
        })?;
        if !allowed {
            return Err(AppError::DepartmentModelNotAllowed {
                provider: provider.clone(),
                model: model_name.clone(),
            });
        }

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

    let (input_rate, output_rate) =
        match get_model_info_cached(&app_state.req_client, &model_name).await {
            Ok(Some(model)) => (model.input_token_rate, model.output_token_rate),
            Ok(None) => (None, None),
            Err(error) => {
                eprintln!("models cache error: {error}");
                (None, None)
            }
        };
    if let Some(conversation_id) = req.conversation_id {
        chat_id = Some(Path(conversation_id));
    }
    let mut metadata = json!({
       "webSearch":req.web_search,
       "selectedTools":selected_tools.clone()
    });
    let retrieval_query = req
        .messages
        .last()
        .map(|message| message.content.clone())
        .unwrap_or_default();
    let mut summary_prompt: Option<Prompt> = None;
    let mut retrieval_prompt: Option<Prompt> = None;
    let mut recent_prompts: Vec<Prompt> = Vec::new();
    let (conversation_id, title) = if let Some(Path(conversation_id)) = chat_id {
        let conversation = conversations::Entity::find_by_id(conversation_id.clone())
            .filter(conversations::Column::ArchivedAt.is_null())
            .one(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("DB get one error {:?}", e);
                AppError::DbTimeout
            })?
            .ok_or(AppError::DbNotFound)?;
        let mut conversation_active = conversation.clone().into_active_model();
        // if !selected_tools.is_empty(){
        // if let Some(json) =  conversation.metadata.as_mut() {
        //     // Update metadata TODO
        //  }
        // conversation_active
        //   .metadata
        //   .as_mut()
        //   .or(Some(&mut metadata));
        // }
        conversation_active.updated_at = Set(Utc::now());
        conversation_active.message_count =
            Set(conversation.message_count + req.messages.len() as i32);
        conversation_active.last_message_at = Set(Some(Utc::now()));
        conversation_active
            .update(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("Db update one error {:?}", e);
                AppError::DbTimeout
            })?;
        if app_state.settings.rag.enabled {
            let recent = load_recent_prompts(
                &app_state.database,
                conversation_id,
                app_state.settings.rag.recent_message_pairs,
            )
            .await?;
            let recent_boundary = recent.boundary;
            recent_prompts = recent.prompts;
            if let Some(summary) = load_summary(&app_state.database, conversation_id).await? {
                if !summary.summary.trim().is_empty() {
                    summary_prompt = Some(Prompt {
                        role: ChatRole::System,
                        text: format!("Conversation summary:\n{}", summary.summary),
                        files: Vec::new(),
                    });
                }
            }
            if let Some(retrieval_text) = build_retrieval_prompt(
                &app_state,
                conversation_id,
                &retrieval_query,
                recent_boundary,
            )
            .await?
            {
                retrieval_prompt = Some(Prompt {
                    role: ChatRole::System,
                    text: retrieval_text,
                    files: Vec::new(),
                });
            }
        } else {
            let previous_messages = messages::Entity::find()
                .filter(messages::Column::ConversationId.eq(conversation_id))
                .filter(messages::Column::Deleted.eq(false))
                .order_by_asc(messages::Column::CreatedAt)
                .all(&app_state.database)
                .await
                .map_err(|e| {
                    eprintln!("DB get messages error {:?}", e);
                    AppError::DbTimeout
                })?;
            recent_prompts = previous_messages
                .into_iter()
                .map(|message| Prompt {
                    text: message.message_content,
                    role: message.role,
                    files: message
                        .metadata
                        .and_then(|json| json.get("files").cloned())
                        .and_then(|files_val| serde_json::from_value::<Vec<File>>(files_val).ok())
                        .unwrap_or_default(),
                })
                .collect::<Vec<Prompt>>();
        }
        (conversation_id, None)
    } else {
        let first_prompt = req
            .messages
            .first()
            .map(|message| message.content.clone())
            .ok_or(AppError::ValidationEmptyField { field: "messages" })?;
        let new_conversation_id = Uuid::new_v4();
        let prompt_title_result = match &provider_config {
            LlmProviderConfig::OpenAI(settings) => {
                app_state
                    .req_client
                    .openai_get_title(settings, first_prompt)
                    .await
            }
            LlmProviderConfig::Anthropic(settings) => {
                app_state
                    .req_client
                    .anthropic_get_title(settings, first_prompt)
                    .await
            }
            LlmProviderConfig::Mistral(settings) => {
                let model = get_title_generation_model(&provider)
                    .unwrap_or("mistral-small-latest")
                    .to_string();
                app_state
                    .req_client
                    .mistral_get_title(settings, model, first_prompt)
                    .await
            }
            LlmProviderConfig::Gemini(settings) => {
                let model = get_title_generation_model(&provider)
                    .unwrap_or("gemini-2.5-flash")
                    .to_string();
                app_state
                    .req_client
                    .gemini_get_title(settings, model, first_prompt)
                    .await
            }
        };
        let mut new_metadata = metadata.clone();
        let mut generated_title: Option<String> = None;
        let is_rate_limit = |err: &anyhow::Error| {
            let msg = err.to_string();
            msg.contains("429") || msg.to_lowercase().contains("too many requests")
        };
        match prompt_title_result {
            Ok(prompt_title_response) => {
                let title_generation_usage = json!({
                    "model":get_title_generation_model(&provider),
                    "inputTokens":prompt_title_response.input_tokens,
                    "outputTokens":prompt_title_response.output_tokens,
                });
                new_metadata["titleGenerationUsage"] = title_generation_usage;
                generated_title = Some(prompt_title_response.title);
            }
            Err(e) => {
                if !is_rate_limit(&e) {
                    eprintln!("title generation error {:?}", e);
                }
            }
        }
        let new_conversation = conversations::ActiveModel {
            id: Set(new_conversation_id.clone()),
            user_id: Set(claims.user_id),
            title: Set(generated_title.clone()),
            model_provider: Set(provider.clone()),
            model_name: Set(model_name.clone()),
            created_at: Set(Utc::now()),
            updated_at: Set(Utc::now()),
            last_message_at: Set(Some(Utc::now())),
            archived_at: Set(None),
            message_count: Set(req.messages.len() as i32),
            total_tokens: Set(0),
            total_cost: Set(Decimal::from(0)),
            metadata: Set(Some(new_metadata)),
        };
        new_conversation
            .insert(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("Db insert one error {:?}", e);
                AppError::DbTimeout
            })?;
        (new_conversation_id, generated_title)
    };
    let mut previous_message_id = None;
    let mut embedding_targets: Vec<EmbeddingTarget> = Vec::new();
    for message in &req.messages {
        let new_message_id = Uuid::new_v4();
        let created_at = Utc::now();
        metadata["files"] = message
            .files
            .iter()
            .map(|f| serde_json::to_value(f).unwrap())
            .collect::<Vec<serde_json::Value>>()
            .into();
        let new_message = messages::ActiveModel {
            id: Set(new_message_id),
            conversation_id: Set(conversation_id),
            previous_message_id: Set(previous_message_id),
            role: Set(message.role),
            deleted: Set(false),
            message_content: Set(message.content.clone()),
            model_provider: Set(provider.clone()),
            model_name: Set(model_name.clone()),
            request_id: Set(None),
            request_tokens: Set(0),
            response_tokens: Set(0),
            tools_calls: Set(Vec::new()),
            tools_results: Set(Vec::new()),
            created_at: Set(created_at),
            updated_at: Set(created_at),
            total_tokens: Set(0),
            latency: Set(start.elapsed().as_millis() as i32),
            cost: Set(Decimal::from(0)),
            metadata: Set(Some(metadata.clone())),
        };
        previous_message_id = Some(new_message_id);
        new_message
            .clone()
            .insert(&app_state.database)
            .await
            .map_err(|e| {
                eprintln!("Db one insert error {:?}", e);
                AppError::DbTimeout
            })?;
        if matches!(message.role, ChatRole::User | ChatRole::Assistant) {
            embedding_targets.push(EmbeddingTarget {
                message_id: new_message_id,
                conversation_id,
                role: message.role,
                content: message.content.clone(),
                created_at,
            });
        }
    }

    let current_prompts: Vec<Prompt> = req
        .messages
        .into_iter()
        .map(|message| Prompt {
            text: message.content,
            role: message.role,
            files: message.files,
        })
        .collect();
    let mut previous_prompts = if app_state.settings.rag.enabled {
        assemble_prompts_with_budget(
            summary_prompt,
            retrieval_prompt,
            recent_prompts,
            current_prompts.clone(),
            app_state.settings.rag.max_context_tokens,
        )
    } else {
        let mut prompts = recent_prompts;
        prompts.extend(current_prompts.clone());
        prompts
    };
    if let Ok(Some(system_prompt)) =
        system_prompts::resolve_system_prompt(&app_state.database, claims.user_id).await
    {
        if !system_prompt.prompt_text.trim().is_empty() {
            previous_prompts.insert(
                0,
                Prompt {
                    role: ChatRole::System,
                    text: system_prompt.prompt_text,
                    files: Vec::new(),
                },
            );
        }
    }
    // Inject project instructions as the top-priority system prompt block.
    // Two-step query to avoid camelCase column aliasing issues in Sea-ORM JOINs.
    let project_ids: Vec<Uuid> = conversation_projects::Entity::find()
        .select_only()
        .column(conversation_projects::Column::ProjectId)
        .filter(conversation_projects::Column::ConversationId.eq(conversation_id))
        .into_tuple::<Uuid>()
        .all(&app_state.database)
        .await
        .unwrap_or_default();
    if !project_ids.is_empty() {
        let linked_projects = projects::Entity::find()
            .filter(projects::Column::Id.is_in(project_ids))
            .all(&app_state.database)
            .await
            .unwrap_or_default();
        let mut project_blocks: Vec<String> = linked_projects
            .into_iter()
            .filter_map(|p| {
                let instr = p.instructions?;
                let trimmed = instr.trim().to_string();
                if trimmed.is_empty() {
                    return None;
                }
                Some(format!("## Project: {}\n{}", p.name, trimmed))
            })
            .collect();
        if !project_blocks.is_empty() {
            project_blocks.sort();
            let project_text = format!(
                "You are working within the context of the following project(s). Follow their instructions carefully.\n\n{}",
                project_blocks.join("\n\n---\n\n")
            );
            // Prepend to the existing system prompt so providers that accept only one
            // system block (e.g. Anthropic) don't silently discard it.
            if let Some(existing) = previous_prompts.iter_mut().find(|p| p.role == ChatRole::System) {
                existing.text = format!("{}\n\n---\n\n{}", project_text, existing.text);
            } else {
                previous_prompts.insert(0, Prompt {
                    role: ChatRole::System,
                    text: project_text,
                    files: Vec::new(),
                });
            }
        }
    }
    const ARTIFACT_PROMPT: &str = "When your response includes a standalone document \
(complete HTML page, full Markdown file, or long code file) that is best viewed separately, \
wrap it in an artifact block:\n\n\
<artifact title=\"Descriptive title\" contentType=\"text/html\">\n\
...complete content here...\n\
</artifact>\n\n\
Valid contentType values: text/html, text/markdown\n\
Do not repeat the artifact content in your conversational reply.";
    if let Some(existing) = previous_prompts.iter_mut().find(|p| p.role == ChatRole::System) {
        existing.text.push_str(&format!("\n\n---\n\n{}", ARTIFACT_PROMPT));
    } else {
        previous_prompts.insert(0, Prompt {
            role: ChatRole::System,
            text: ARTIFACT_PROMPT.to_string(),
            files: Vec::new(),
        });
    }
    let provider_is_openai = provider.to_lowercase() == "openai";
    let provider_is_anthropic = provider.to_lowercase() == "anthropic";
    let provider_is_mistral = provider.to_lowercase() == "mistral";
    let provider_is_gemini = provider.to_lowercase() == "gemini";
    let gemini_web_search_only = provider_is_gemini
        && web_search
        && selected_tools.is_empty()
        && request_selected_mcp_servers.is_empty();
    let supports_mcp_tools =
        provider_is_openai || provider_is_anthropic || provider_is_mistral || provider_is_gemini;
    let should_auto_select_mcp =
        supports_mcp_tools && selected_mcp_servers.is_empty() && !gemini_web_search_only;
    if should_auto_select_mcp {
        selected_mcp_servers = load_auto_mcp_server_ids(&app_state).await?;
    }
    let (mcp_openai_tools, mcp_tool_lookup, mcp_server_summaries) =
        if supports_mcp_tools && !gemini_web_search_only {
            load_openai_mcp_tools(
                &app_state,
                claims.user_id,
                &selected_mcp_servers,
                &selected_tools,
            )
            .await?
        } else {
            (Vec::new(), HashMap::new(), Vec::new())
        };
    let mistral_has_function_tools = provider_is_mistral && !mcp_tool_lookup.is_empty();
    let mistral_use_conversations =
        provider_is_mistral && (web_search || mistral_has_function_tools);
    if std::env::var("MISTRAL_TOOL_DEBUG").as_deref() == Ok("1") && provider_is_mistral {
        println!(
            "mistral mcp debug: selected_tools={:?} selected_mcp_servers={:?} mcp_tools_loaded={:?}",
            selected_tools,
            selected_mcp_servers,
            mcp_tool_lookup.keys().cloned().collect::<Vec<_>>()
        );
    }
    if supports_mcp_tools {
        if let Some(context) = build_mcp_server_context(&mcp_server_summaries, &mcp_tool_lookup) {
            let insert_at = previous_prompts
                .iter()
                .position(|prompt| prompt.role != ChatRole::System)
                .unwrap_or(previous_prompts.len());
            previous_prompts.insert(
                insert_at,
                Prompt {
                    role: ChatRole::System,
                    text: context,
                    files: Vec::new(),
                },
            );
        }
    }
    let mut openai_tools = Vec::new();
    if web_search {
        openai_tools.push(OpenaiTool::web_search());
    }
    openai_tools.extend(mcp_openai_tools);
    let openai_tools = if openai_tools.is_empty() {
        None
    } else {
        Some(openai_tools)
    };
    let mut anthropic_tools = Vec::new();
    if web_search {
        anthropic_tools.push(AnthropicToolUnion::WebSearchTool(
            AnthropicWebSearchTool::new(Some(5)),
        ));
    }
    if supports_mcp_tools {
        for descriptor in mcp_tool_lookup.values() {
            let description = descriptor
                .description
                .clone()
                .unwrap_or_else(|| descriptor.original_name.clone());
            anthropic_tools.push(AnthropicToolUnion::ClientTool(AnthropicTool {
                name: descriptor.openai_name.clone(),
                description,
                input_schema: descriptor.input_schema.clone(),
            }));
        }
    }
    let anthropic_tools = if anthropic_tools.is_empty() {
        None
    } else {
        Some(anthropic_tools)
    };
    let mut mistral_tools = Vec::new();
    if !mistral_use_conversations && supports_mcp_tools {
        for descriptor in mcp_tool_lookup.values() {
            let description = descriptor
                .description
                .clone()
                .unwrap_or_else(|| descriptor.original_name.clone());
            mistral_tools.push(MistralTool::Function {
                function: MistralToolDefinition {
                    name: descriptor.openai_name.clone(),
                    description: Some(description),
                    parameters: descriptor.input_schema.clone(),
                },
            });
        }
    }
    let mistral_tools = if mistral_tools.is_empty() {
        None
    } else {
        Some(mistral_tools)
    };
    let mistral_conversation_tools = if mistral_use_conversations {
        let mut tools = Vec::new();
        if web_search {
            tools.push(MistralTool::WebSearch);
        }
        if supports_mcp_tools {
            for descriptor in mcp_tool_lookup.values() {
                let description = descriptor
                    .description
                    .clone()
                    .unwrap_or_else(|| descriptor.original_name.clone());
                tools.push(MistralTool::Function {
                    function: MistralToolDefinition {
                        name: descriptor.openai_name.clone(),
                        description: Some(description),
                        parameters: descriptor.input_schema.clone(),
                    },
                });
            }
        }
        if tools.is_empty() { None } else { Some(tools) }
    } else {
        None
    };
    let mistral_tool_choice = if mistral_tools.is_some() {
        if !selected_tools.is_empty() && mcp_tool_lookup.len() == 1 {
            let tool_name = mcp_tool_lookup.keys().next().cloned().unwrap_or_default();
            Some(json!({"type":"function","function":{"name": tool_name}}))
        } else {
            Some(json!("auto"))
        }
    } else {
        None
    };
    let gemini_tools = if provider_is_gemini {
        let mut tools: Vec<Value> = Vec::new();
        if web_search {
            tools.push(json!({ "google_search": {} }));
        }
        if supports_mcp_tools && !mcp_tool_lookup.is_empty() {
            let function_declarations = mcp_tool_lookup
                .values()
                .map(|descriptor| {
                    let description = descriptor
                        .description
                        .clone()
                        .unwrap_or_else(|| descriptor.original_name.clone());
                    json!({
                        "name": descriptor.openai_name.clone(),
                        "description": description,
                        "parameters": normalize_gemini_parameters(&descriptor.input_schema),
                    })
                })
                .collect::<Vec<Value>>();
            if !function_declarations.is_empty() {
                tools.push(json!({
                    "function_declarations": function_declarations
                }));
            }
        }
        if tools.is_empty() {
            None
        } else {
            Some(Value::Array(tools))
        }
    } else {
        None
    };
    let gemini_tool_config = if provider_is_gemini {
        let mut config = serde_json::Map::new();
        if web_search && !mcp_tool_lookup.is_empty() {
            config.insert(
                "include_server_side_tool_invocations".to_string(),
                json!(true),
            );
        }
        if !selected_tools.is_empty() && mcp_tool_lookup.len() == 1 {
            let tool_name = mcp_tool_lookup.keys().next().cloned().unwrap_or_default();
            config.insert(
                "function_calling_config".to_string(),
                json!({
                    "mode": "ANY",
                    "allowed_function_names": [tool_name],
                }),
            );
        } else if !mcp_tool_lookup.is_empty() {
            config.insert(
                "function_calling_config".to_string(),
                json!({
                    "mode": "AUTO",
                }),
            );
        }
        if config.is_empty() {
            None
        } else {
            Some(Value::Object(config))
        }
    } else {
        None
    };
    let (gemini_system_instruction, gemini_contents_seed) = if provider_is_gemini {
        prompts_to_gemini_payload(&previous_prompts)
    } else {
        (None, Vec::new())
    };
    let base_prompts = if (provider_is_anthropic || provider_is_mistral) && supports_mcp_tools {
        Some(previous_prompts.clone())
    } else {
        None
    };
    let (mistral_agent_instructions, mistral_inputs) = if mistral_use_conversations {
        let mut instructions = Vec::new();
        let entries = previous_prompts
            .iter()
            .filter_map(|prompt| match prompt.role {
                ChatRole::System => {
                    if !prompt.text.trim().is_empty() {
                        instructions.push(prompt.text.clone());
                    }
                    None
                }
                ChatRole::User | ChatRole::Assistant => Some(json!({
                    "object": "entry",
                    "type": "message.input",
                    "role": prompt.role,
                    "content": prompt.text,
                })),
                _ => None,
            })
            .collect::<Vec<Value>>();
        let instructions = instructions.join("\n\n");
        (instructions, Value::Array(entries))
    } else {
        (String::new(), Value::Null)
    };
    let mut mistral_completion_args_map = serde_json::Map::new();
    if let Some(temperature) = req.temperature {
        mistral_completion_args_map.insert("temperature".to_string(), json!(temperature));
    }
    if supports_mcp_tools && !selected_tools.is_empty() && mcp_tool_lookup.len() == 1 {
        if !mistral_use_conversations {
            if let Some(tool_name) = mcp_tool_lookup.keys().next() {
                mistral_completion_args_map.insert(
                    "tool_choice".to_string(),
                    json!({"type":"function","function":{"name": tool_name}}),
                );
            }
        }
    }
    let mistral_completion_args = if mistral_completion_args_map.is_empty() {
        None
    } else {
        Some(Value::Object(mistral_completion_args_map))
    };
    // Create event source based on provider
    let event_source = match &provider_config {
        LlmProviderConfig::OpenAI(settings) => {
            app_state
                .req_client
                .openai_chat_stream(
                    settings,
                    model_name.clone(),
                    req.temperature,
                    previous_prompts,
                    &claims.user_id,
                    openai_tools.clone(),
                    None,
                    None,
                    None,
                )
                .await
        }
        LlmProviderConfig::Anthropic(settings) => {
            app_state
                .req_client
                .anthropic_chat_stream(
                    settings,
                    model_name.clone(),
                    ANTHROPIC_DEFAULT_MAX_TOKENS,
                    req.temperature,
                    previous_prompts,
                    anthropic_tools.clone(),
                    &claims.user_id,
                )
                .await
        }
        LlmProviderConfig::Mistral(settings) => {
            if mistral_use_conversations {
                app_state
                    .req_client
                    .mistral_conversation_start_stream(
                        settings,
                        mistral_inputs.clone(),
                        mistral_conversation_tools.clone(),
                        mistral_completion_args.clone(),
                        Some(model_name.clone()),
                        None,
                        if mistral_agent_instructions.is_empty() {
                            None
                        } else {
                            Some(mistral_agent_instructions.clone())
                        },
                    )
                    .await
            } else {
                app_state
                    .req_client
                    .mistral_chat_stream(
                        settings,
                        model_name.clone(),
                        req.temperature,
                        previous_prompts,
                        mistral_tools.clone(),
                        mistral_tool_choice.clone(),
                    )
                    .await
            }
        }
        LlmProviderConfig::Gemini(settings) => {
            app_state
                .req_client
                .gemini_chat_stream_with_contents(
                    settings,
                    model_name.clone(),
                    req.temperature,
                    gemini_system_instruction.clone(),
                    Value::Array(gemini_contents_seed.clone()),
                    gemini_tools.clone(),
                    gemini_tool_config.clone(),
                )
                .await
        }
    }
    .map_err(|_| AppError::LlmProviderNotConfigured {
        provider: provider.clone(),
    })?;
    // Create stream parser based on provider
    let stream_parser: Box<dyn StreamParser> = match &provider_config {
        LlmProviderConfig::OpenAI(_) => Box::new(OpenaiStreamParser::new()),
        LlmProviderConfig::Anthropic(_) => Box::new(AnthropicStreamParser::new()),
        LlmProviderConfig::Mistral(_) => {
            if mistral_use_conversations {
                Box::new(MistralConversationStreamParser::new())
            } else {
                Box::new(MistralStreamParser::new())
            }
        }
        LlmProviderConfig::Gemini(_) => Box::new(GeminiStreamParser::new()),
    };

    let sse_stream = async_stream::try_stream! {
       let mut message_content = String::new();
       let mut stream_message_content = String::new();
       // Artifact delta filter state — strips <artifact>…</artifact> from the live delta stream.
       let mut artifact_filter_in_progress = false;
       let mut artifact_filter_buf = String::new();
       let mut request_tokens = 0;
       let mut response_tokens = 0;
       let mut total_tokens = 0;
       let mut request_id: Option<String> = None;
       let mut openai_response_id: Option<String> = None;
       let mut tool_calls: Vec<serde_json::Value> = Vec::new();
       let mut tool_results: Vec<serde_json::Value> = Vec::new();
       let mut seen_tool_call_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
       let mut tool_input_buffers: HashMap<String, String> = HashMap::new();
       let mut tool_inputs: HashMap<String, Value> = HashMap::new();
       let mut web_search_state: HashMap<String, StreamWebSearchState> = HashMap::new();
       let mut oauth_prompt_urls: HashMap<Uuid, McpOauthPrompt> = HashMap::new();
       let mut oauth_required_seen = false;
       let mut last_web_search_call_id: Option<String> = None;
       let mcp_tooling_enabled = supports_mcp_tools && !mcp_tool_lookup.is_empty();
       let openai_tooling_enabled = provider_is_openai && mcp_tooling_enabled;
       let anthropic_tooling_enabled = provider_is_anthropic && mcp_tooling_enabled;
       let mistral_tooling_enabled = provider_is_mistral && mcp_tooling_enabled;
       let gemini_tooling_enabled = provider_is_gemini && mcp_tooling_enabled;
       let mut openai_previous_response_id: Option<String> = None;
       let mut openai_next_input: Option<Vec<OpenaiInputItem>> = None;
       let mut tool_round: usize = 0;
       let max_tool_rounds: usize = 3;
       let mut pending_mcp_tool_calls: Vec<ToolCall> = Vec::new();
       let mut anthropic_messages: Option<Vec<AnthropicMessage>> = None;
       let mut anthropic_system_prompt: Option<String> = None;
       let mut mistral_messages: Option<Vec<MistralMessage>> = None;
       let mut mistral_conversation_id: Option<String> = None;
       let mut mistral_conversation_next_inputs: Option<Value> = None;
       let mut gemini_contents: Option<Vec<Value>> = if provider_is_gemini {
           Some(gemini_contents_seed.clone())
       } else {
           None
       };
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
        let assistant_created_at = Utc::now();
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
           created_at: Set(assistant_created_at),
           updated_at: Set(assistant_created_at),
           total_tokens: Set(total_tokens),
           latency: Set(latency),
           cost: Set(Decimal::from(0)),
           metadata: Set(Some(json!({"webSearch":req.web_search}))),
         };
        new_llm_message
             .clone()
             .insert(&app_state.database)
             .await
             .expect("failed to insert llm response in table messages");

       let cancel_handle = app_state.register_stream_cancel(new_message_id).await;

       let mut event_source = event_source;
       let mut final_message_cost = Decimal::from(0);
       loop {
           let mut stream_should_continue = false;
           let mut stream_finished = false;
           while let Some(event) = tokio::select! {
               _ = cancel_handle.cancelled() => {
                   let cancel_cost = calculate_cost_decimal(
                       request_tokens,
                       response_tokens,
                       input_rate,
                       output_rate,
                   );
                   final_message_cost = cancel_cost;
                   new_llm_message.updated_at = Set(Utc::now());
                   new_llm_message.request_tokens = Set(request_tokens);
                   new_llm_message.response_tokens = Set(response_tokens);
                   new_llm_message.total_tokens = Set(request_tokens + response_tokens);
                   new_llm_message.cost = Set(cancel_cost);
                   new_llm_message.message_content = Set(message_content.clone());
                   new_llm_message.metadata = Set(Some(json!({
                       "webSearch": req.web_search,
                       "cancelled": true,
                   })));
                   new_llm_message
                       .clone()
                       .update(&app_state.database)
                       .await
                       .expect("failed to update cancelled llm response");

                   let cancel_event = ChatStream {
                       id: None,
                       title: None,
                       message_id: Some(new_message_id),
                       is_new: None,
                       content: None,
                       input_tokens: Some(request_tokens),
                       output_tokens: Some(response_tokens),
                       latency_ms: Some(latency),
                       cost: cancel_cost.to_f32(),
                       event: None,
                       tool_call: None,
                       tool_result: None,
                   };
                   yield Event::default()
                       .event(ChatStreamEvents::Cancelled.to_string())
                       .data(cancel_event.to_string());
                   stream_finished = true;
                   None
               }
               ev = event_source.next() => ev,
           } {
           match event {
               Ok(ReqwestEvent::Open) => {}
               Ok(ReqwestEvent::Message(msg)) => {
                   if provider.to_lowercase() == "openai" && std::env::var("OPENAI_STREAM_DEBUG").as_deref() == Ok("1") {
                       println!("openai raw event: {}", msg.data);
                   }
                   if mistral_use_conversations && std::env::var("MISTRAL_STREAM_DEBUG").as_deref() == Ok("1") {
                       println!("mistral conversations raw event: event='{}' data={}", msg.event, msg.data);
                   }
                   let mut data_for_parse = msg.data.clone();
                   if mistral_use_conversations {
                       let parsed = serde_json::from_str::<Value>(&msg.data).unwrap_or(Value::String(msg.data.clone()));
                       if !msg.event.is_empty() {
                           let event_name = msg.event.as_str();
                           if event_name == "conversation.response.started" {
                               if let Some(conversation_id) = parsed.get("conversation_id").and_then(|v| v.as_str()) {
                                   mistral_conversation_id = Some(conversation_id.to_string());
                               }
                           }
                           data_for_parse = json!({
                               "event": event_name,
                               "data": parsed,
                           })
                           .to_string();
                       } else if let Some(event_type) = parsed.get("type").and_then(|v| v.as_str()) {
                           if event_type == "conversation.response.started" {
                               if let Some(conversation_id) = parsed.get("conversation_id").and_then(|v| v.as_str()) {
                                   mistral_conversation_id = Some(conversation_id.to_string());
                               }
                           }
                           data_for_parse = parsed.to_string();
                       }
                   }
                   let mut parse_results = vec![stream_parser.parse_event(&data_for_parse)];
                   loop {
                       let pending = stream_parser.parse_event("");
                       if matches!(pending, StreamParseResult::None) {
                           break;
                       }
                       parse_results.push(pending);
                   }

                   for parse_result in parse_results {
                   match &parse_result {
                       StreamParseResult::TextDelta { text, request_id: rid } => {
                           message_content.push_str(text);
                           stream_message_content.push_str(text);
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
                           let visible = filter_artifact_chunk(
                               text,
                               &mut artifact_filter_in_progress,
                               &mut artifact_filter_buf,
                               false,
                           );
                           if !visible.is_empty() {
                               let chat_stream = ChatStream {
                                   id:None,
                                   title:None,
                                   message_id:None,
                                   is_new:None,
                                   content: Some(visible),
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
                       }
                       StreamParseResult::TokenUsage{ request_id:req_id,input_tokens, output_tokens, total_tokens:t_tokens} => {
                          let accumulate_tokens = mcp_tooling_enabled && tool_round > 0;
                          if let Some(tokens) = input_tokens {
                            if accumulate_tokens {
                              request_tokens += tokens.clone() as i32;
                            } else {
                              request_tokens = tokens.clone() as i32;
                            }
                          }
                          if let Some(tokens) = output_tokens {
                            if accumulate_tokens {
                              response_tokens += tokens.clone() as i32;
                            } else {
                              response_tokens = tokens.clone() as i32;
                            }
                          }
                          if let Some(tokens) = t_tokens{
                            if accumulate_tokens {
                              total_tokens += tokens.clone() as i32;
                            } else {
                              total_tokens = tokens.clone() as i32;
                            }
                          }
                          if req_id.is_some() {
                            openai_response_id = req_id.clone();
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
                          let accumulate_tokens = mcp_tooling_enabled && tool_round > 0;
                          if let Some(tokens) = input_tokens {
                            if accumulate_tokens {
                              request_tokens += tokens.clone() as i32;
                            } else {
                              request_tokens = tokens.clone() as i32;
                            }
                          }
                          if let Some(tokens) = output_tokens {
                            if accumulate_tokens {
                              response_tokens += tokens.clone() as i32;
                            } else {
                              response_tokens = tokens.clone() as i32;
                            }
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
                          openai_response_id = Some(req_id.clone());
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
                       StreamParseResult::ToolInput(tool_input) => {
                           let resolved_name = tool_input
                               .tool_name
                               .clone()
                               .unwrap_or_else(|| "tool_call".to_string());
                           let is_web_search = tool_input.is_web_search();
                           let mut parsed_input: Option<Value> = None;
                           if let Some(tool_id) = tool_input.tool_id.as_ref() {
                               let buffer = tool_input_buffers.entry(tool_id.clone()).or_default();
                               buffer.push_str(&tool_input.partial_json);
                               if let Ok(value) = serde_json::from_str::<Value>(buffer) {
                                   tool_inputs.insert(tool_id.clone(), value.clone());
                                   parsed_input = Some(value);
                               }
                           }
                           if is_web_search {
                               let _ = update_web_search_action_state(
                                   &mut web_search_state,
                                   &mut last_web_search_call_id,
                                   tool_input.tool_id.clone(),
                                   tool_input.web_search.clone(),
                               );
                           }
                           let tool_call = ChatStreamToolCall {
                               tool_name: resolved_name.clone(),
                               tool_id: tool_input.tool_id.clone(),
                               input_text: Some(tool_input.partial_json.clone()),
                               input: None,
                               kind: Some(if is_web_search { ChatToolKind::WebSearch } else { ChatToolKind::Other }),
                               web_search: tool_input.web_search.clone().map(|action| ChatStreamWebSearchAction {
                                   query: action.query.clone(),
                                   queries: action.queries.clone(),
                               }),
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

                           if mcp_tooling_enabled
                               && resolve_mcp_tool_descriptor(&mcp_tool_lookup, &resolved_name).is_some()
                           {
                               if let (Some(tool_id), Some(input_value)) =
                                   (tool_input.tool_id.clone(), parsed_input.clone())
                               {
                                   if seen_tool_call_ids.insert(tool_id.clone()) {
                                       let call = ToolCall {
                                           tool_name: resolved_name.clone(),
                                           tool_id: Some(tool_id.clone()),
                                           input: Some(ToolInput::Json(input_value.clone())),
                                           index: tool_input.index,
                                           raw: None,
                                           web_search: tool_input.web_search.clone(),
                                       };
                                       pending_mcp_tool_calls.push(call);

                                       let resolved_call = ChatStreamToolCall {
                                           tool_name: resolved_name.clone(),
                                           tool_id: Some(tool_id),
                                           input_text: None,
                                           input: Some(to_chat_tool_input(&ToolInput::Json(input_value))),
                                           kind: Some(if is_web_search {
                                               ChatToolKind::WebSearch
                                           } else {
                                               ChatToolKind::Other
                                           }),
                                           web_search: tool_input
                                               .web_search
                                               .clone()
                                               .map(|action| ChatStreamWebSearchAction {
                                                   query: action.query.clone(),
                                                   queries: action.queries.clone(),
                                               }),
                                       };
                                       tool_calls.push(
                                           serde_json::to_value(&resolved_call)
                                               .unwrap_or_else(|_| json!({})),
                                       );
                                       new_llm_message.tools_calls = Set(tool_calls.clone());
                                       new_llm_message.updated_at = Set(Utc::now());
                                       new_llm_message
                                           .clone()
                                           .update(&app_state.database)
                                           .await
                                           .expect("failed to update resolved tool call in table messages");
                                       let resolved_stream = ChatStream {
                                           id: None,
                                           title: None,
                                           message_id: None,
                                           is_new: None,
                                           content: None,
                                           input_tokens: None,
                                           output_tokens: None,
                                           latency_ms: None,
                                           cost: None,
                                           event: None,
                                           tool_call: Some(resolved_call),
                                           tool_result: None,
                                       };
                                       yield Event::default()
                                           .event(ChatStreamEvents::ToolCall.to_string())
                                           .data(resolved_stream.to_string());
                                   }
                               }
                           }
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
                       StreamParseResult::ToolCall(call) => {
                           if let Some(id) = call.tool_id.as_ref() {
                               if !seen_tool_call_ids.insert(id.clone()) {
                                   continue;
                               }
                           }
                           if let Some(tool_id) = call.tool_id.as_ref() {
                               if let Some(input) = call.input.as_ref() {
                                   let value = tool_input_to_value(Some(input));
                                   if !value.is_null() {
                                       tool_inputs.insert(tool_id.clone(), value);
                                   }
                               }
                           }
                           let is_web_search = call.is_web_search();
                           if mcp_tooling_enabled
                               && resolve_mcp_tool_descriptor(&mcp_tool_lookup, &call.tool_name).is_some()
                           {
                               pending_mcp_tool_calls.push(call.clone());
                           }
                           if is_web_search {
                               let _ = update_web_search_action_state(
                                   &mut web_search_state,
                                   &mut last_web_search_call_id,
                                   call.tool_id.clone(),
                                   call.web_search.clone(),
                               );
                           }
                           let input = call.input.as_ref().map(to_chat_tool_input);
                           let tool_call = ChatStreamToolCall {
                               tool_name: call.tool_name.clone(),
                               tool_id: call.tool_id.clone(),
                               input_text: None,
                               input,
                               kind: Some(if is_web_search { ChatToolKind::WebSearch } else { ChatToolKind::Other }),
                               web_search: call.web_search.as_ref().map(|action| ChatStreamWebSearchAction {
                                   query: action.query.clone(),
                                   queries: action.queries.clone(),
                               }),
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
                       StreamParseResult::WebSearchAction { tool_name: _, tool_id, query, queries } => {
                           let entry = update_web_search_action_state(
                               &mut web_search_state,
                               &mut last_web_search_call_id,
                               tool_id.clone(),
                               Some(ParsedWebSearchAction { query:query.clone(), queries:queries.clone() }),
                           );
                           if let Some((resolved_id, entry)) = entry {
                               if !seen_tool_call_ids.insert(resolved_id.clone()) {
                                   continue;
                               }
                               let tool_call = ChatStreamToolCall {
                                   tool_name: "web_search_call".to_string(),
                                   tool_id: Some(resolved_id),
                                   input_text: None,
                                   input: None,
                                   kind: Some(ChatToolKind::WebSearch),
                                   web_search: Some(ChatStreamWebSearchAction {
                                       query: entry.query.clone(),
                                       queries: entry.queries.clone(),
                                   }),
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
                       }
                       StreamParseResult::WebSearchResult { tool_name: _, tool_id, results } => {
                           let entry = update_web_search_results_state(
                               &mut web_search_state,
                               &mut last_web_search_call_id,
                               tool_id.clone(),
                               results.clone(),
                           );
                           if let Some((resolved_id, entry)) = entry {
                               let tool_result = ChatStreamToolResult {
                                   tool_name: Some("web_search_call".to_string()),
                                   tool_id: Some(resolved_id),
                                   kind: Some(ChatToolKind::WebSearch),
                                   status: Some("success".to_string()),
                                   output: None,
                                   web_search: Some(to_chat_web_search_result(&entry)),
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
                       StreamParseResult::ToolResult(result) => {
                           let is_web_search = result.is_web_search();
                           let tool_result = ChatStreamToolResult {
                               tool_name: result.tool_name.clone(),
                               tool_id: result.tool_id.clone(),
                               kind: Some(if is_web_search { ChatToolKind::WebSearch } else { ChatToolKind::Other }),
                               status: tool_result_status_from_output(&result.output),
                               output: result.output.clone(),
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
                         final_message_cost = message_cost;
                         println!("Stream ended for provider: {} input tokens: {} output_tokens: {} total_tokens: {} latency in ms: {} cost: {}", &provider,request_tokens,response_tokens,total_tokens,latency,&message_cost);
                       new_llm_message.latency = Set(latency);
                       new_llm_message.request_tokens = Set(request_tokens);
                       new_llm_message.response_tokens = Set(response_tokens);
                       new_llm_message.total_tokens = Set(total_tokens);
                       new_llm_message.cost = Set(message_cost);
                       new_llm_message.tools_calls = Set(tool_calls.clone());
                       new_llm_message.tools_results = Set(tool_results.clone());
                       new_llm_message.updated_at = Set(Utc::now());
                       new_llm_message
                         .clone()
                           .update(&app_state.database)
                           .await
                           .expect("failed to update llm response in table messages");
                       // remove cancel handle once stream ends
                       app_state.clear_stream_cancel(new_message_id).await;
                       if mcp_tooling_enabled
                           && !pending_mcp_tool_calls.is_empty()
                           && tool_round < max_tool_rounds
                       {
                           let mut tool_outputs: Vec<OpenaiInputItem> = Vec::new();
                           let mut anthropic_tool_use_blocks: Vec<AnthropicContentBlock> = Vec::new();
                           let mut anthropic_tool_result_blocks: Vec<AnthropicContentBlock> = Vec::new();
                           let mut mistral_tool_calls: Vec<MistralToolCall> = Vec::new();
                           let mut mistral_tool_messages: Vec<MistralMessage> = Vec::new();
                           let mut mistral_function_results_entries: Vec<Value> = Vec::new();
                           let mut gemini_model_tool_messages: Vec<Value> = Vec::new();
                           let mut gemini_function_response_messages: Vec<Value> = Vec::new();

                           if anthropic_tooling_enabled && !stream_message_content.trim().is_empty() {
                               anthropic_tool_use_blocks.push(AnthropicContentBlock::Text {
                                   text: stream_message_content.clone(),
                               });
                           }

                           for call in pending_mcp_tool_calls.drain(..) {
                               let Some(tool_ref) = resolve_mcp_tool_descriptor(&mcp_tool_lookup, &call.tool_name).cloned() else {
                                   continue;
                               };
                               let mut args_value = tool_input_to_value(call.input.as_ref());
                               if let Some(tool_id) = call.tool_id.as_ref() {
                                   let use_buffered = matches!(args_value, Value::Null)
                                       || matches!(args_value, Value::Object(ref map) if map.is_empty());
                                   if use_buffered {
                                       if let Some(buffered) = tool_inputs.get(tool_id) {
                                           args_value = buffered.clone();
                                       }
                                   }
                               }
                               let args_for_call = match args_value {
                                   Value::Object(_) => args_value.clone(),
                                   Value::Null => json!({}),
                                   other => json!({ "value": other }),
                               };

                               let access_error = match tool_ref.permission {
                                   McpPermission::Denied => Some("mcp access denied".to_string()),
                                   McpPermission::ReadOnly if !tool_ref.is_read_only => {
                                       Some("mcp access read_only: tool is not read-only".to_string())
                                   }
                                   _ => None,
                               };
                               let mut oauth_prompt: Option<McpOauthPrompt> = None;
                               let mut oauth_prompt_event: Option<McpOauthPrompt> = None;
                               let exec_start = Instant::now();
                               let call_result = if let Some(error) = access_error {
                                   Err(error)
                               } else {
                                   let (client, requires_oauth) = {
                                       let clients = app_state.mcp_clients.read().await;
                                       match clients.get(&tool_ref.server_id) {
                                           Some(client) => {
                                               let requires_oauth = matches!(
                                                   client.transport_type,
                                                   McpTransportType::Http | McpTransportType::Sse
                                               )
                                                   && client.connection_config.get("oauth").is_some();
                                               (Some(client.clone()), requires_oauth)
                                           }
                                           None => (None, false),
                                       }
                                   };
                                   let auth_token = if requires_oauth {
                                       match resolve_mcp_oauth_token(&app_state, claims.user_id, tool_ref.server_id)
                                           .await
                                       {
                                           Ok(token) => token,
                                           Err(err) => {
                                               eprintln!("mcp oauth token error: {:?}", err);
                                               None
                                           }
                                       }
                                   } else {
                                       None
                                   };
                                   match client {
                                       Some(client) => {
                                           if requires_oauth && auth_token.is_none() {
                                               let prompt = if let Some(existing) =
                                                   oauth_prompt_urls.get(&tool_ref.server_id)
                                               {
                                                   Some(existing.clone())
                                               } else {
                                                   match build_mcp_oauth_prompt(
                                                       &app_state,
                                                       tool_ref.server_id,
                                                       claims.user_id,
                                                   )
                                                   .await
                                                   {
                                                       Ok(prompt) => {
                                                           oauth_prompt_urls
                                                               .insert(tool_ref.server_id, prompt.clone());
                                                           oauth_prompt_event = Some(prompt.clone());
                                                           Some(prompt)
                                                       }
                                                       Err(err) => {
                                                           eprintln!("mcp oauth prompt error: {:?}", err);
                                                           None
                                                       }
                                                   }
                                               };
                                               if let Some(prompt) = prompt.clone() {
                                                   oauth_prompt = Some(prompt);
                                                   oauth_required_seen = true;
                                               }
                                               if let Some(prompt) = oauth_prompt_event.clone() {
                                                   let payload = McpOauthRequiredEvent {
                                                       text: format!(
                                                           "Connect {} account to continue.",
                                                           prompt.server_name
                                                       ),
                                                       server_id: tool_ref.server_id,
                                                       server_name: prompt.server_name.clone(),
                                                       authorization_url: prompt.authorization_url.clone(),
                                                       tool_name: tool_ref.original_name.clone(),
                                                       tool_call_id: call.tool_id.clone(),
                                                   };
                                                   let data = serde_json::to_string(&payload)
                                                       .unwrap_or_else(|_| "{}".to_string());
                                                   yield Event::default()
                                                       .event("mcp_oauth_required")
                                                       .data(data);
                                               }
                                               Err("mcp oauth connection required".to_string())
                                           } else {
                                               client
                                                   .call_tool(
                                                       &tool_ref.original_name,
                                                       args_for_call.clone(),
                                                       auth_token,
                                                       Some(claims.user_id),
                                                   )
                                                   .await
                                                   .map_err(|e| e.to_string())
                                           }
                                       }
                                       None => Err(format!(
                                           "mcp server {} not connected",
                                           tool_ref.server_id
                                       )),
                                   }
                               };

                               let duration_ms = exec_start.elapsed().as_millis() as i32;
                               let (output_payload, result_value, is_error) = match call_result {
                                   Ok(result) => {
                                       let result_value =
                                           serde_json::to_value(&result).unwrap_or_else(|_| json!({}));
                                       let output_payload = result
                                           .structured_content
                                           .clone()
                                           .unwrap_or_else(|| result_value.clone());
                                       let is_error = result.is_error.unwrap_or(false);
                                       (output_payload, result_value, is_error)
                                   }
                                   Err(error) => {
                                       let error_payload = if let Some(prompt) = oauth_prompt.clone() {
                                           let payload = McpOauthErrorPayload {
                                               error,
                                               is_error: true,
                                               authorization_url: prompt.authorization_url,
                                               server_id: tool_ref.server_id,
                                               server_name: prompt.server_name,
                                           };
                                           serde_json::to_value(payload)
                                               .unwrap_or_else(|_| json!({"error":"mcp oauth connection required","is_error":true}))
                                       } else {
                                           json!({
                                               "error": error,
                                               "is_error": true
                                           })
                                       };
                                       (error_payload.clone(), error_payload, true)
                                   }
                               };

                               let execution = mcp_executions::ActiveModel {
                                   id: Set(Uuid::new_v4()),
                                   server_id: Set(tool_ref.server_id),
                                   server_name: Set(tool_ref.server_name.clone()),
                                   tool_name: Set(tool_ref.original_name.clone()),
                                   conversation_id: Set(Some(conversation_id)),
                                   user_id: Set(Some(claims.user_id)),
                                   user_email: Set(Some(claims.sub.clone())),
                                   arguments: Set(Some(args_for_call.clone())),
                                   result: Set(Some(result_value.clone())),
                                   is_error: Set(is_error),
                                   duration_ms: Set(Some(duration_ms)),
                                   executed_at: Set(Utc::now()),
                                   created_at: Set(Utc::now()),
                               };
                               if let Err(e) = execution.insert(&app_state.database).await {
                                   eprintln!("mcp execution insert error: {e}");
                               }

                               let tool_result = ChatStreamToolResult {
                                   tool_name: Some(call.tool_name.clone()),
                                   tool_id: call.tool_id.clone(),
                                   kind: Some(ChatToolKind::Other),
                                   status: Some(if is_error { "error" } else { "success" }.to_string()),
                                   output: Some(output_payload.clone()),
                                   web_search: None,
                               };
                               tool_results.push(
                                   serde_json::to_value(&tool_result).unwrap_or_else(|_| json!({})),
                               );
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

                               if gemini_tooling_enabled {
                                   let gemini_call_id = call
                                       .tool_id
                                       .clone()
                                       .unwrap_or_else(|| format!("gemini_call_{}", Uuid::new_v4()));
                                   let thought_signature = call
                                       .raw
                                       .as_ref()
                                       .and_then(|raw| {
                                           raw.get("thoughtSignature")
                                               .cloned()
                                               .or_else(|| raw.get("thought_signature").cloned())
                                       });
                                   let mut part = serde_json::Map::new();
                                   part.insert(
                                       "functionCall".to_string(),
                                       json!({
                                           "id": gemini_call_id.clone(),
                                           "name": call.tool_name.clone(),
                                           "args": args_for_call.clone(),
                                       }),
                                   );
                                   if let Some(signature) = thought_signature {
                                       part.insert("thoughtSignature".to_string(), signature);
                                   }
                                   gemini_model_tool_messages.push(json!({
                                       "role": "model",
                                       "parts": [Value::Object(part)]
                                   }));
                                   gemini_function_response_messages.push(json!({
                                       "role": "user",
                                       "parts": [{
                                           "functionResponse": {
                                               "id": gemini_call_id,
                                               "name": call.tool_name.clone(),
                                               "response": {
                                                   "output": output_payload.clone(),
                                               }
                                           }
                                       }]
                                   }));
                               }

                               if let Some(call_id) = call.tool_id.clone() {
                                   if openai_tooling_enabled {
                                       let output_text = serde_json::to_string(&output_payload)
                                           .unwrap_or_else(|_| "{}".to_string());
                                       tool_outputs.push(OpenaiInputItem::FunctionCallOutput(
                                           OpenaiFunctionCallOutput {
                                               item_type: "function_call_output".to_string(),
                                               call_id: call_id.clone(),
                                               output: output_text,
                                           },
                                       ));
                                   }
                                   if anthropic_tooling_enabled {
                                       anthropic_tool_use_blocks.push(AnthropicContentBlock::ToolUse {
                                           id: call_id.clone(),
                                           name: call.tool_name.clone(),
                                           input: args_for_call.clone(),
                                       });
                                       let output_text = serde_json::to_string(&output_payload)
                                           .unwrap_or_else(|_| "{}".to_string());
                                       anthropic_tool_result_blocks.push(
                                           AnthropicContentBlock::ToolResult {
                                               tool_use_id: call_id.clone(),
                                               content: output_text,
                                               is_error: Some(is_error),
                                           },
                                       );
                                   }
                                   if mistral_use_conversations {
                                       let output_text = serde_json::to_string(&output_payload)
                                           .unwrap_or_else(|_| "{}".to_string());
                                       mistral_function_results_entries.push(json!({
                                           "object": "entry",
                                           "type": "function.result",
                                           "tool_call_id": call_id.clone(),
                                           "result": output_text,
                                       }));
                                   } else if mistral_tooling_enabled {
                                       let arguments_text = serde_json::to_string(&args_for_call)
                                           .unwrap_or_else(|_| "{}".to_string());
                                       mistral_tool_calls.push(MistralToolCall {
                                           id: call_id.clone(),
                                           call_type: "function".to_string(),
                                           function: crate::dto::llm::mistral::MistralToolFunction {
                                               name: call.tool_name.clone(),
                                               arguments: arguments_text,
                                           },
                                       });
                                       let output_text = serde_json::to_string(&output_payload)
                                           .unwrap_or_else(|_| "{}".to_string());
                                       mistral_tool_messages.push(MistralMessage::tool_response(
                                           call.tool_name.clone(),
                                           call_id,
                                           output_text,
                                       ));
                                   }
                               }
                           }

                           if oauth_required_seen {
                               stream_finished = true;
                           } else if openai_tooling_enabled {
                               if tool_outputs.is_empty() || openai_response_id.is_none() {
                                   stream_finished = true;
                               } else {
                                   openai_previous_response_id = openai_response_id.clone();
                                   openai_next_input = Some(tool_outputs);
                                   tool_round += 1;
                                   stream_should_continue = true;
                               }
                           } else if mistral_use_conversations {
                               if mistral_function_results_entries.is_empty()
                                   || mistral_conversation_id.is_none()
                               {
                                   stream_finished = true;
                               } else {
                                   mistral_conversation_next_inputs =
                                       Some(Value::Array(mistral_function_results_entries));
                                   tool_round += 1;
                                   stream_should_continue = true;
                               }
                           } else if mistral_tooling_enabled {
                               if mistral_tool_messages.is_empty() {
                                   stream_finished = true;
                               } else {
                                   let mut messages = if let Some(existing) = mistral_messages.take() {
                                       existing
                                   } else {
                                       let base = base_prompts.clone().unwrap_or_default();
                                       MistralMessage::from_prompts(base)
                                   };
                                   if !mistral_tool_calls.is_empty() {
                                       let content = if stream_message_content.trim().is_empty() {
                                           None
                                       } else {
                                           Some(stream_message_content.clone())
                                       };
                                       messages.push(MistralMessage::assistant_with_tool_calls(
                                           content,
                                           mistral_tool_calls,
                                       ));
                                   }
                                   messages.extend(mistral_tool_messages);
                                   mistral_messages = Some(messages);
                                   tool_round += 1;
                                   stream_should_continue = true;
                               }
                           } else if anthropic_tooling_enabled {
                               if anthropic_tool_result_blocks.is_empty() {
                                   stream_finished = true;
                               } else {
                                   let mut messages = if let Some(existing) = anthropic_messages.take() {
                                       existing
                                   } else {
                                       let base = base_prompts.clone().unwrap_or_default();
                                       let (messages, system_prompt) =
                                           AnthropicMessage::from_prompts(base);
                                       anthropic_system_prompt = system_prompt;
                                       messages
                                   };
                                   if !anthropic_tool_use_blocks.is_empty() {
                                       messages.push(AnthropicMessage::with_blocks(
                                           AnthropicRole::Assistant,
                                           anthropic_tool_use_blocks,
                                       ));
                                   }
                                   messages.push(AnthropicMessage::with_blocks(
                                       AnthropicRole::User,
                                       anthropic_tool_result_blocks,
                                   ));
                                   anthropic_messages = Some(messages);
                                   tool_round += 1;
                                   stream_should_continue = true;
                               }
                           } else if gemini_tooling_enabled {
                               if gemini_function_response_messages.is_empty() {
                                   stream_finished = true;
                               } else {
                                   let mut contents = gemini_contents.take().unwrap_or_default();
                                   contents.extend(gemini_model_tool_messages);
                                   contents.extend(gemini_function_response_messages);
                                   gemini_contents = Some(contents);
                                   tool_round += 1;
                                   stream_should_continue = true;
                               }
                           } else {
                               stream_finished = true;
                           }
                       } else {
                           stream_finished = true;
                       }
                       break;
                       },
                       reqwest_eventsource::Error::InvalidStatusCode(status, response) => {
                           let body = response
                               .text()
                               .await
                               .unwrap_or_else(|_| "<failed to read response body>".to_string());
                           if status == StatusCode::TOO_MANY_REQUESTS
                               && is_rate_limit_error(&body)
                           {
                               let (_, detail) = AppError::LlmTokenExhausted {
                                   provider: provider.clone(),
                               }
                               .to_detail();
                               let payload = ErrorResponse {
                                   detail: ErrorDetailVariant::Rich(detail),
                               };
                               let data = serde_json::to_string(&payload)
                                   .unwrap_or_else(|_| "{}".to_string());
                               yield Event::default()
                                   .event(ChatStreamEvents::LlmTokenExhausted.to_string())
                                   .data(data);
                           }
                           eprintln!(
                               "Streaming error for provider:{} status:{} body:{}",
                               provider, status, body
                           );
                           stream_finished = true;
                           break;
                       }
                       _ => {
                           println!("Streaming error for provider:{} error:{}",provider,e.to_string());
                           stream_finished = true;
                           break;
                       }
                   };
               }
           }
           }
           if stream_should_continue {
               if let LlmProviderConfig::OpenAI(settings) = &provider_config {
                   let prev_id = openai_previous_response_id.clone();
                   if prev_id.is_none() {
                       stream_finished = true;
                   } else if let Some(next_input) = openai_next_input.take() {
                       match app_state.req_client
                           .openai_chat_stream(
                               settings,
                               model_name.clone(),
                               req.temperature,
                               Vec::new(),
                               &claims.user_id,
                               openai_tools.clone(),
                               None,
                               prev_id,
                               Some(next_input),
                           )
                           .await
                       {
                           Ok(es) => {
                               event_source = es;
                               stream_message_content.clear();
                               continue;
                           }
                           Err(e) => {
                               eprintln!("openai continuation error: {e}");
                               stream_finished = true;
                           }
                       }
                   } else {
                       stream_finished = true;
                   }
               } else if let LlmProviderConfig::Anthropic(settings) = &provider_config {
                   let next_messages = anthropic_messages.take();
                   if let Some(messages) = next_messages {
                       let system_prompt = anthropic_system_prompt.clone();
                       match app_state.req_client
                           .anthropic_chat_stream_with_messages(
                               settings,
                               model_name.clone(),
                               ANTHROPIC_DEFAULT_MAX_TOKENS,
                               req.temperature,
                               messages,
                               system_prompt,
                               anthropic_tools.clone(),
                           )
                           .await
                       {
                           Ok(es) => {
                               event_source = es;
                               stream_message_content.clear();
                               continue;
                           }
                           Err(e) => {
                               eprintln!("anthropic continuation error: {e}");
                               stream_finished = true;
                           }
                       }
                   } else {
                       stream_finished = true;
                   }
               } else if let LlmProviderConfig::Mistral(settings) = &provider_config {
                   if mistral_use_conversations {
                       let next_inputs = mistral_conversation_next_inputs.take();
                       let conversation_id = mistral_conversation_id.clone();
                       if let (Some(inputs), Some(conversation_id)) = (next_inputs, conversation_id) {
                           match app_state.req_client
                               .mistral_conversation_append_stream(
                                   settings,
                                   conversation_id,
                                   inputs,
                                   None,
                                   None,
                               )
                               .await
                           {
                               Ok(es) => {
                                   event_source = es;
                                   stream_message_content.clear();
                                   continue;
                               }
                               Err(e) => {
                                   eprintln!("mistral conversation continuation error: {e}");
                                   stream_finished = true;
                               }
                           }
                       } else {
                           stream_finished = true;
                       }
                   } else {
                       let next_messages = mistral_messages.take();
                       if let Some(messages) = next_messages {
                           match app_state.req_client
                               .mistral_chat_stream_with_messages(
                                   settings,
                                   model_name.clone(),
                                   req.temperature,
                                   messages,
                                   mistral_tools.clone(),
                                   mistral_tool_choice.clone(),
                               )
                               .await
                           {
                               Ok(es) => {
                                   event_source = es;
                                   stream_message_content.clear();
                                   continue;
                               }
                               Err(e) => {
                                   eprintln!("mistral continuation error: {e}");
                                   stream_finished = true;
                               }
                           }
                       } else {
                           stream_finished = true;
                       }
                   }
               } else if let LlmProviderConfig::Gemini(settings) = &provider_config {
                   let next_contents = gemini_contents.clone();
                   if let Some(contents) = next_contents {
                       match app_state
                           .req_client
                           .gemini_chat_stream_with_contents(
                               settings,
                               model_name.clone(),
                               req.temperature,
                               gemini_system_instruction.clone(),
                               Value::Array(contents),
                               gemini_tools.clone(),
                               gemini_tool_config.clone(),
                           )
                           .await
                       {
                           Ok(es) => {
                               event_source = es;
                               stream_message_content.clear();
                               continue;
                           }
                           Err(e) => {
                               eprintln!("gemini continuation error: {e}");
                               stream_finished = true;
                           }
                       }
                   } else {
                       stream_finished = true;
                   }
               } else {
                   stream_finished = true;
               }
           }

           if !stream_should_continue && !stream_finished {
               stream_finished = true;
           }

           if stream_finished {
               app_state.clear_stream_cancel(new_message_id).await;
               if total_tokens == 0 {
                   total_tokens = request_tokens + response_tokens;
               }
               let message_cost = if final_message_cost > Decimal::from(0) {
                   final_message_cost
               } else {
                   calculate_cost_decimal(
                       request_tokens,
                       response_tokens,
                       input_rate,
                       output_rate,
                   )
               };
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
                       } else if let Err(e) = emit_budget_alerts(&app_state, department_id).await {
                           eprintln!("emit budget alert error: {:?}", e);
                       }
                   }
               }
               if !message_content.trim().is_empty() {
                   embedding_targets.push(EmbeddingTarget {
                       message_id: new_message_id,
                       conversation_id,
                       role: ChatRole::Assistant,
                       content: message_content.clone(),
                       created_at: assistant_created_at,
                   });
               }
               if app_state.settings.rag.enabled {
                   let targets = std::mem::take(&mut embedding_targets);
                   let state = app_state.clone();
                   let provider_clone = provider.clone();
                   let model_clone = model_name.clone();
                   tokio::spawn(async move {
                       let _ = embed_messages(&state, targets).await;
                       let _ = update_conversation_summary(&state, conversation_id, &provider_clone, &model_clone).await;
                   });
               }
               // Flush any remaining look-ahead buffer as visible text.
               let flushed = filter_artifact_chunk(
                   "",
                   &mut artifact_filter_in_progress,
                   &mut artifact_filter_buf,
                   true,
               );
               if !flushed.is_empty() {
                   let chat_stream = ChatStream {
                       id: None, title: None, message_id: None, is_new: None,
                       content: Some(flushed), input_tokens: None, output_tokens: None,
                       latency_ms: None, cost: None, event: None, tool_call: None, tool_result: None,
                   };
                   yield Event::default().event(ChatStreamEvents::Delta.to_string()).data(chat_stream.to_string());
               }
               let artifacts = extract_artifacts(&message_content);
               for artifact in artifacts {
                   let data = serde_json::to_string(&artifact).unwrap_or_else(|_| "{}".to_string());
                   yield Event::default().event(ChatStreamEvents::Artifact.to_string()).data(data);
               }
               yield Event::default().event(ChatStreamEvents::Done.to_string()).data("{}");
               break;
           }
       }
    };
    let sse_response = Sse::new(sse_stream).keep_alive(KeepAlive::new());
    Ok(sse_response)
}
