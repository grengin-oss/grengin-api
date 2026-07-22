use crate::{
    auth::{claims::Claims, error::Error},
    dto::{
        chat_stream::{
            ActiveSkillInfo, ArtifactSavedPayload, BudgetWarningPayload, ChatInput, ChatStream,
            ChatStreamEvent, ChatStreamEvents, ChatStreamPayload, ChatStreamToolCall,
            ChatStreamToolResult, ChatStreamWebSearchAction, ChatToolKind, SkillsActivePayload,
        },
        files::File,
        llm::anthropic::{
            ANTHROPIC_DEFAULT_MAX_TOKENS, AnthropicContentBlock, AnthropicMessage,
        },
        llm::gemini::{prompts_to_gemini_payload, GeminiContent},
        llm::mistral::{MistralConversationFunctionResult, MistralMessage, MistralToolCall},
        llm::openai::OpenaiInputItem,
    },
    dto::skills::SkillToolsConfig,
    error::{AppError, ChatStreamError, ErrorResponse},
    handlers::llm::{
        StreamParseResult, StreamParser, StreamWebSearchAction as ParsedWebSearchAction,
        StreamWebSearchState, ToolCall, ToolInput,
        anthropic::{
            AnthropicStreamParser, build_anthropic_continuation, build_anthropic_tools,
            make_anthropic_tool_blocks,
        },
        gemini::{
            GeminiStreamParser, build_gemini_tool_config, build_gemini_tool_messages,
            build_gemini_tools,
        },
        mistral::{
            MistralStreamParser, build_mistral_agent_inputs, build_mistral_completion_args,
            build_mistral_continuation, build_mistral_conversation_tools,
            build_mistral_tool_choice, build_mistral_tools, make_mistral_conversation_result,
            make_mistral_tool_result,
        },
        mistral_conversations::MistralConversationStreamParser,
        openai::{OpenaiStreamParser, build_openai_tools, make_openai_function_output},
        update_web_search_action_state, update_web_search_results_state,
    },
    dto::models::ModelType,
    services::{image_gen_helpers::generate_and_save, models_cache::get_model_info_cached},
    llm::{
        prompt::Prompt,
        provider::{
            AnthropicApis, GeminiApis, MistralApis, OpenaiApis, get_title_generation_model,
        },
    },
    models::{
        conversation_projects, conversations,
        departments::ActionOnExceed,
        mcp_access_policies::McpPermission,
        mcp_executions,
        mcp_servers::McpTransportType,
        messages::{self, ChatRole},
        projects, users,
    },
    services::{
        artifacts::{
            ARTIFACT_SYSTEM_HINT, ArtifactAccum, ArtifactParser, ArtifactParseEvent,
            content_type_to_ext,
        },
        chat_helpers::LlmProviderConfig,
        budget_allocation::{get_department_budget_status, refresh_department_budget_available},
        department_policies::check_model_allowed,
        mcp_helpers::{
            build_mcp_oauth_prompt, build_mcp_server_context,
            resolve_mcp_oauth_token, resolve_mcp_tool_descriptor,
            McpOauthErrorPayload, McpOauthPrompt, McpOauthRequiredEvent,
        },
        mcp_tools::load_openai_mcp_tools,
        notifications::emit_budget_alerts,
        rag::{
            EmbeddingTarget, assemble_prompts_with_budget, build_retrieval_prompt, embed_messages,
            load_recent_prompts, load_summary, update_conversation_summary,
        },
        system_prompts,
        skills_helpers::{load_skill_knowledge_for_stream, load_skills_for_stream},
    },
    state::SharedState,
    utils::chat_stream::{
        calculate_cost_decimal, extract_llm_error_message, is_rate_limit_error,
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
use chrono::Utc;
use futures_util::StreamExt;
use num_traits::ToPrimitive;
use reqwest::StatusCode;
use reqwest_eventsource::Event as ReqwestEvent;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, QuerySelect, prelude::Decimal,
};
use serde_json::{Value, json};
use std::{collections::HashMap, convert::Infallible};
use tokio::time::Instant;
use uuid::Uuid;



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
      description = "event:conversation — emitted once at the start. is_new=true when a new conversation was created.",
      value = json!({
        "event": "conversation",
        "data": { "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "title": "My first chat", "is_new": true }
      })
    )),
    ("budget_warning" = (
      description = "event:budget_warning — emitted after conversation when the department budget is low. Stream continues.",
      value = json!({
        "event": "budget_warning",
        "data": { "department_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "budget_available": "1.25", "action": "warn", "message": "Department budget is running low." }
      })
    )),
    ("skills" = (
      description = "event:skills — emitted when one or more skills are active (builtin skills are always auto-included). Emitted before message_start.",
      value = json!({
        "event": "skills",
        "data": { "skills": [{ "id": "00000000-0000-0000-0000-000000000001", "identifier": "artifact-create", "name": "Artifact Creator" }] }
      })
    )),
    ("image_generated" = (
      description = "event:image_generated — emitted for image generation models instead of the LLM event sequence (message_start/delta/message_end). Contains the saved file and cost. Always followed immediately by done.",
      value = json!({
        "event": "image_generated",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "file_id": "92deac9e-7f9f-4d4f-a69b-43737cc2b3a7", "content_type": "image/png", "cost": 0.04 }
      })
    )),
    ("message_start" = (
      description = "event:message_start — emitted when the assistant message record is created.",
      value = json!({
        "event": "message_start",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6" }
      })
    )),
    ("delta" = (
      description = "event:delta — one or more text chunks streamed from the model.",
      value = json!({
        "event": "delta",
        "data": { "text": "Hello world" }
      })
    )),
    ("event" = (
      description = "event:event — internal model event such as a thinking_delta from extended reasoning.",
      value = json!({
        "event": "event",
        "data": { "event": { "event_type": "thinking_delta", "text": "Considering options..." } }
      })
    )),
    ("tool_call" = (
      description = "event:tool_call — emitted when the model invokes a tool (web search, MCP tool, artifact, etc.).",
      value = json!({
        "event": "tool_call",
        "data": { "tool_call": { "tool_name": "web_search_call", "tool_id": "ws_123", "kind": "web_search", "web_search": { "query": "latest rust release" } } }
      })
    )),
    ("tool_result" = (
      description = "event:tool_result — emitted after a tool call completes with its output.",
      value = json!({
        "event": "tool_result",
        "data": { "tool_result": { "tool_name": "web_search_call", "tool_id": "ws_123", "kind": "web_search", "status": "success", "web_search": { "query": "latest rust release", "results": [{ "title": "Rust 1.80 released", "url": "https://blog.rust-lang.org" }] } } }
      })
    )),
    ("artifact_start" = (
      description = "event:artifact_start — emitted when an artifact opening tag is parsed. Signals start of streamed artifact content.",
      value = json!({
        "event": "artifact_start",
        "data": { "id": "art_abc123", "title": "Hello World Page", "contentType": "text/html" }
      })
    )),
    ("artifact_delta" = (
      description = "event:artifact_delta — streamed content chunk for an in-progress artifact.",
      value = json!({
        "event": "artifact_delta",
        "data": { "id": "art_abc123", "chunk": "<!DOCTYPE html>\n<html>" }
      })
    )),
    ("artifact_end" = (
      description = "event:artifact_end — emitted when the artifact closing tag is parsed. Artifact is now complete.",
      value = json!({
        "event": "artifact_end",
        "data": { "id": "art_abc123" }
      })
    )),
    ("artifact_saved" = (
      description = "event:artifact_saved — emitted after the stream ends once the artifact has been persisted to disk and the database. Contains the saved artifact and file IDs. streamId links back to the artifact_start/delta/end events.",
      value = json!({
        "event": "artifact_saved",
        "data": { "streamId": "art_abc123", "id": "e6096689-e23b-488a-b6f3-340d32edc723", "fileId": "75127ab4-08ea-4570-9132-0f78ef24083a", "title": "Hello World Page", "contentType": "text/html" }
      })
    )),
    ("message_end" = (
      description = "event:message_end — emitted when the model finishes. Contains token usage and cost.",
      value = json!({
        "event": "message_end",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "input_tokens": 320, "output_tokens": 85, "latency_ms": 1240, "cost": 0.00042 }
      })
    )),
    ("cancelled" = (
      description = "event:cancelled — emitted when the stream was cancelled by the client before completion.",
      value = json!({
        "event": "cancelled",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6" }
      })
    )),
    ("done" = (
      description = "event:done — final event. Always emitted last (even after ai_error or cancelled).",
      value = json!({
        "event": "done",
        "data": {}
      })
    )),
    ("ai_error_quota" = (
      description = "event:ai_error (code 4005) — provider rate limit or API quota exhausted. Read detail.code to distinguish error types. Stream ends after this event.",
      value = json!({
        "event": "ai_error",
        "data": { "detail": { "type": "rich", "code": 4005, "description": "The openai API quota or rate limit has been exhausted.", "solution": "Check your API key quota and billing, or wait for the rate limit window to reset.", "description_key": "error.llm.api_quota_exhausted.description", "solution_key": "error.llm.api_quota_exhausted.solution", "params": { "app": "grengin", "provider": "openai" }, "external_code": null } }
      })
    )),
    ("ai_error_provider" = (
      description = "event:ai_error (code 4006) — provider returned an error inside the stream (discontinued model, bad request, server error). Stream ends after this event.",
      value = json!({
        "event": "ai_error",
        "data": { "detail": { "type": "rich", "code": 4006, "description": "Error from mistral: The model `mistral-small-latest` has been deprecated.", "solution": "Verify the selected model is available and your API key is valid.", "description_key": "error.llm.stream_provider_error.description", "solution_key": "error.llm.stream_provider_error.solution", "params": { "app": "grengin", "provider": "mistral", "message": "The model `mistral-small-latest` has been deprecated." }, "external_code": null } }
      })
    )),
    ("ai_error_connection" = (
      description = "event:ai_error (code 4007) — failed to initiate or reconnect a stream request. Stream ends after this event.",
      value = json!({
        "event": "ai_error",
        "data": { "detail": { "type": "rich", "code": 4007, "description": "Failed to establish or maintain a stream connection to gemini.", "solution": "Try again. If the problem persists, check the provider's status page.", "description_key": "error.llm.stream_connection_failed.description", "solution_key": "error.llm.stream_connection_failed.solution", "params": { "app": "grengin", "provider": "gemini" }, "external_code": null } }
      })
    ))
   )
   ),
    (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
    (status = 400, content_type = "application/json", body = ErrorResponse, description = "Validation error — empty messages (code=2002)"),
    (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003), model not allowed for department (code=6002), or budget exceeded (code=6001)"),
    (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found (code=5003)"),
    (status = 503, content_type = "application/json", body = ErrorResponse, description = "DB unavailable (code=5000) or timeout (code=5001)"),

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
      description = "event:conversation — emitted once at the start. is_new=true when a new conversation was created.",
      value = json!({
        "event": "conversation",
        "data": { "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "title": "My first chat", "is_new": true }
      })
    )),
    ("budget_warning" = (
      description = "event:budget_warning — emitted after conversation when the department budget is low. Stream continues.",
      value = json!({
        "event": "budget_warning",
        "data": { "department_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "budget_available": "1.25", "action": "warn", "message": "Department budget is running low." }
      })
    )),
    ("skills" = (
      description = "event:skills — emitted when one or more skills are active (builtin skills are always auto-included). Emitted before message_start.",
      value = json!({
        "event": "skills",
        "data": { "skills": [{ "id": "00000000-0000-0000-0000-000000000001", "identifier": "artifact-create", "name": "Artifact Creator" }] }
      })
    )),
    ("image_generated" = (
      description = "event:image_generated — emitted for image generation models instead of the LLM event sequence (message_start/delta/message_end). Contains the saved file and cost. Always followed immediately by done.",
      value = json!({
        "event": "image_generated",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "file_id": "92deac9e-7f9f-4d4f-a69b-43737cc2b3a7", "content_type": "image/png", "cost": 0.04 }
      })
    )),
    ("message_start" = (
      description = "event:message_start — emitted when the assistant message record is created.",
      value = json!({
        "event": "message_start",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6" }
      })
    )),
    ("delta" = (
      description = "event:delta — one or more text chunks streamed from the model.",
      value = json!({
        "event": "delta",
        "data": { "text": "Hello world" }
      })
    )),
    ("event" = (
      description = "event:event — internal model event such as a thinking_delta from extended reasoning.",
      value = json!({
        "event": "event",
        "data": { "event": { "event_type": "thinking_delta", "text": "Considering options..." } }
      })
    )),
    ("tool_call" = (
      description = "event:tool_call — emitted when the model invokes a tool (web search, MCP tool, artifact, etc.).",
      value = json!({
        "event": "tool_call",
        "data": { "tool_call": { "tool_name": "web_search_call", "tool_id": "ws_123", "kind": "web_search", "web_search": { "query": "latest rust release" } } }
      })
    )),
    ("tool_result" = (
      description = "event:tool_result — emitted after a tool call completes with its output.",
      value = json!({
        "event": "tool_result",
        "data": { "tool_result": { "tool_name": "web_search_call", "tool_id": "ws_123", "kind": "web_search", "status": "success", "web_search": { "query": "latest rust release", "results": [{ "title": "Rust 1.80 released", "url": "https://blog.rust-lang.org" }] } } }
      })
    )),
    ("artifact_start" = (
      description = "event:artifact_start — emitted when an artifact opening tag is parsed. Signals start of streamed artifact content.",
      value = json!({
        "event": "artifact_start",
        "data": { "id": "art_abc123", "title": "Hello World Page", "contentType": "text/html" }
      })
    )),
    ("artifact_delta" = (
      description = "event:artifact_delta — streamed content chunk for an in-progress artifact.",
      value = json!({
        "event": "artifact_delta",
        "data": { "id": "art_abc123", "chunk": "<!DOCTYPE html>\n<html>" }
      })
    )),
    ("artifact_end" = (
      description = "event:artifact_end — emitted when the artifact closing tag is parsed. Artifact is now complete.",
      value = json!({
        "event": "artifact_end",
        "data": { "id": "art_abc123" }
      })
    )),
    ("artifact_saved" = (
      description = "event:artifact_saved — emitted after the stream ends once the artifact has been persisted to disk and the database. Contains the saved artifact and file IDs. streamId links back to the artifact_start/delta/end events.",
      value = json!({
        "event": "artifact_saved",
        "data": { "streamId": "art_abc123", "id": "e6096689-e23b-488a-b6f3-340d32edc723", "fileId": "75127ab4-08ea-4570-9132-0f78ef24083a", "title": "Hello World Page", "contentType": "text/html" }
      })
    )),
    ("message_end" = (
      description = "event:message_end — emitted when the model finishes. Contains token usage and cost.",
      value = json!({
        "event": "message_end",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "input_tokens": 320, "output_tokens": 85, "latency_ms": 1240, "cost": 0.00042 }
      })
    )),
    ("cancelled" = (
      description = "event:cancelled — emitted when the stream was cancelled by the client before completion.",
      value = json!({
        "event": "cancelled",
        "data": { "message_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6" }
      })
    )),
    ("done" = (
      description = "event:done — final event. Always emitted last (even after ai_error or cancelled).",
      value = json!({
        "event": "done",
        "data": {}
      })
    )),
    ("ai_error_quota" = (
      description = "event:ai_error (code 4005) — provider rate limit or API quota exhausted. Read detail.code to distinguish error types. Stream ends after this event.",
      value = json!({
        "event": "ai_error",
        "data": { "detail": { "type": "rich", "code": 4005, "description": "The openai API quota or rate limit has been exhausted.", "solution": "Check your API key quota and billing, or wait for the rate limit window to reset.", "description_key": "error.llm.api_quota_exhausted.description", "solution_key": "error.llm.api_quota_exhausted.solution", "params": { "app": "grengin", "provider": "openai" }, "external_code": null } }
      })
    )),
    ("ai_error_provider" = (
      description = "event:ai_error (code 4006) — provider returned an error inside the stream (discontinued model, bad request, server error). Stream ends after this event.",
      value = json!({
        "event": "ai_error",
        "data": { "detail": { "type": "rich", "code": 4006, "description": "Error from mistral: The model `mistral-small-latest` has been deprecated.", "solution": "Verify the selected model is available and your API key is valid.", "description_key": "error.llm.stream_provider_error.description", "solution_key": "error.llm.stream_provider_error.solution", "params": { "app": "grengin", "provider": "mistral", "message": "The model `mistral-small-latest` has been deprecated." }, "external_code": null } }
      })
    )),
    ("ai_error_connection" = (
      description = "event:ai_error (code 4007) — failed to initiate or reconnect a stream request. Stream ends after this event.",
      value = json!({
        "event": "ai_error",
        "data": { "detail": { "type": "rich", "code": 4007, "description": "Failed to establish or maintain a stream connection to gemini.", "solution": "Try again. If the problem persists, check the provider's status page.", "description_key": "error.llm.stream_connection_failed.description", "solution_key": "error.llm.stream_connection_failed.solution", "params": { "app": "grengin", "provider": "gemini" }, "external_code": null } }
      })
    ))
   )
   ),
    (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
    (status = 400, content_type = "application/json", body = ErrorResponse, description = "Validation error — empty messages (code=2002)"),
    (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003), model not allowed for department (code=6002), or budget exceeded (code=6001)"),
    (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation not found (code=5003)"),
    (status = 503, content_type = "application/json", body = ErrorResponse, description = "DB unavailable (code=5000) or timeout (code=5001)"),
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
    let provider = req.provider.clone();
    let selected_tools = req.selected_tools.clone().unwrap_or_default();
    let request_selected_mcp_servers = req.selected_mcp_servers.clone().unwrap_or_default();
    let selected_mcp_servers = request_selected_mcp_servers.clone();
    let transient_skill_ids = req.selected_skills.clone().unwrap_or_default();
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
            let model = req.model_name.clone();
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
            let model = req.model_name.clone();
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
            let model = req.model_name.clone();
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
            let model = req.model_name.clone();
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

    let (input_rate, output_rate, image_input_rate, image_output_rate, is_image_gen) =
        match get_model_info_cached(&app_state.req_client, &model_name).await {
            Ok(Some(model)) => (
                model.input_token_rate,
                model.output_token_rate,
                model.image_input_token_rate,
                model.image_output_token_rate,
                model.model_type == ModelType::ImageGenerator,
            ),
            Ok(None) => (None, None, None, None, false),
            Err(error) => {
                eprintln!("models cache error: {error}");
                (None, None, None, None, false)
            }
        };
    if let Some(conversation_id) = req.conversation_id {
        chat_id = Some(Path(conversation_id));
    }
    let mut metadata = json!({
       "webSearch":req.web_search,
       "selectedTools":selected_tools.clone()
    });
    let last_message = req.messages.last();
    let retrieval_query = last_message
        .map(|m| m.content.clone())
        .unwrap_or_default();
    let input_image_file_ids: Vec<Uuid> = req
        .messages
        .iter()
        .rev()
        .find(|m| !m.files.is_empty())
        .map(|m| m.files.iter().map(|f| f.id).collect())
        .unwrap_or_default();
    let image_prompt = retrieval_query.clone();
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
                    .unwrap_or("mistral-small-2603")
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
            .filter(projects::Column::Id.is_in(project_ids.clone()))
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
    let active_skills = if is_image_gen {
        vec![]
    } else {
        load_skills_for_stream(&app_state.database, conversation_id, &transient_skill_ids).await
    };
    let mut skill_web_search = false;
    let mut skill_mcp_server_ids: Vec<Uuid> = Vec::new();
    if !active_skills.is_empty() {
        let skill_ids: Vec<uuid::Uuid> = active_skills.iter().map(|s| s.id).collect();
        let knowledge_map = load_skill_knowledge_for_stream(&app_state.database, &skill_ids).await;
        let skill_role_blocks: Vec<String> = active_skills
            .iter()
            .filter_map(|s| {
                let instructions = s.instructions.as_deref().unwrap_or("").trim().to_string();
                let knowledge = knowledge_map.get(&s.id).cloned().unwrap_or_default();
                if instructions.is_empty() && knowledge.is_empty() {
                    return None;
                }
                let mut block = format!("## Skill: {}", s.name);
                if !instructions.is_empty() {
                    block.push('\n');
                    block.push_str(&instructions);
                }
                if !knowledge.is_empty() {
                    block.push_str("\n\n### Knowledge\n");
                    block.push_str(&knowledge);
                }
                Some(block)
            })
            .collect();
        if !skill_role_blocks.is_empty() {
            let skill_text = skill_role_blocks.join("\n\n---\n\n");
            if let Some(sys) = previous_prompts.iter_mut().find(|p| p.role == ChatRole::System) {
                sys.text = format!("{}\n\n---\n\n{}", sys.text, skill_text);
            } else {
                previous_prompts.insert(0, Prompt {
                    role: ChatRole::System,
                    text: skill_text,
                    files: Vec::new(),
                });
            }
        }
        for skill in &active_skills {
            if let Some(config_json) = &skill.tools_config {
                let config = SkillToolsConfig::from_json(config_json);
                if config.web_search {
                    skill_web_search = true;
                }
                for id in config.mcp_server_ids {
                    if !skill_mcp_server_ids.contains(&id) {
                        skill_mcp_server_ids.push(id);
                    }
                }
            }
        }
    }
    let artifact_enabled = active_skills.iter().any(|s| s.identifier == "artifact-create");
    let web_search = web_search || skill_web_search;
    let selected_mcp_servers = {
        let mut merged = selected_mcp_servers;
        for id in skill_mcp_server_ids {
            if !merged.contains(&id) {
                merged.push(id);
            }
        }
        merged
    };
    if artifact_enabled {
        if let Some(sys) = previous_prompts.iter_mut().find(|p| p.role == ChatRole::System) {
            sys.text = format!("{}\n\n{}", sys.text, ARTIFACT_SYSTEM_HINT);
        } else {
            previous_prompts.insert(0, Prompt {
                role: ChatRole::System,
                text: ARTIFACT_SYSTEM_HINT.to_string(),
                files: Vec::new(),
            });
        }
    }
    let provider_is_openai = provider.to_lowercase() == "openai";
    let provider_is_anthropic = provider.to_lowercase() == "anthropic";
    let provider_is_mistral = provider.to_lowercase() == "mistral";
    let provider_is_gemini = provider.to_lowercase() == "gemini";
    let gemini_web_search_only = provider_is_gemini
        && web_search
        && selected_tools.is_empty()
        && selected_mcp_servers.is_empty();
    let supports_mcp_tools =
        provider_is_openai || provider_is_anthropic || provider_is_mistral || provider_is_gemini;
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
    let openai_tools = build_openai_tools(web_search, mcp_openai_tools);
    let anthropic_tools = build_anthropic_tools(web_search, &mcp_tool_lookup);
    let mistral_tools = build_mistral_tools(mistral_use_conversations, &mcp_tool_lookup);
    let mistral_conversation_tools = if mistral_use_conversations {
        build_mistral_conversation_tools(web_search, supports_mcp_tools, &mcp_tool_lookup)
    } else {
        None
    };
    let mistral_tool_choice = build_mistral_tool_choice(&selected_tools, &mcp_tool_lookup, mistral_tools.is_some());
    let gemini_tools = if provider_is_gemini {
        build_gemini_tools(web_search, &mcp_tool_lookup)
    } else {
        None
    };
    let gemini_tool_config = if provider_is_gemini {
        build_gemini_tool_config(web_search, &selected_tools, &mcp_tool_lookup)
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
        build_mistral_agent_inputs(&previous_prompts)
    } else {
        (String::new(), Value::Null)
    };
    let mistral_completion_args = build_mistral_completion_args(
        req.temperature,
        &selected_tools,
        &mcp_tool_lookup,
        mistral_use_conversations,
    );
    // Create event source based on provider (skipped for image generation models)
    let event_source = if !is_image_gen { Some(match &provider_config {
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
    })?) } else { None };
    // Create stream parser based on provider (skipped for image generation models)
    let stream_parser: Option<Box<dyn StreamParser>> = if !is_image_gen { Some(match &provider_config {
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
    }) } else { None };

    let sse_stream = async_stream::try_stream! {
       let mut message_content = String::new();
       let mut stream_message_content = String::new();
       let mut artifact_parser = ArtifactParser::new();
       let mut artifact_accumulator: HashMap<String, ArtifactAccum> = HashMap::new();
       let mut request_tokens = 0;
       let mut response_tokens = 0;
       let mut total_tokens = 0;
       let mut image_gen_cost: Decimal = Decimal::ZERO;
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
       // tool_id → tool_name for every tool_call SSE emitted to the client (non-artifact).
       // Used to flush unresolved calls as failed results when the stream ends early.
       let mut emitted_tc: HashMap<String, String> = HashMap::new();
       let mut completed_tr: std::collections::HashSet<String> = std::collections::HashSet::new();
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
        if !active_skills.is_empty() {
            let skills_payload = SkillsActivePayload {
                skills: active_skills.iter().map(|s| ActiveSkillInfo {
                    id: s.id,
                    identifier: s.identifier.clone(),
                    name: s.name.clone(),
                    avatar: s.avatar.clone(),
                }).collect(),
            };
            if let Ok(data) = serde_json::to_string(&skills_payload) {
                yield Event::default().event(ChatStreamEvents::Skills.to_string()).data(data);
            }
        }
        // Image generation — no LLM stream, just generate + save + emit one event
        if is_image_gen {
            let img_message_id = Uuid::new_v4();
            let now = Utc::now();
            match generate_and_save(&app_state, claims.user_id, &provider, &model_name, &image_prompt, &input_image_file_ids).await {
                Ok((file_id, content_type, text_input_tokens, image_input_tokens, img_output_tokens)) => {
                    let effective_output_rate = if image_input_tokens > 0 { image_output_rate.or(output_rate) } else { output_rate };
                    image_gen_cost =
                        calculate_cost_decimal(text_input_tokens, 0, input_rate, None)
                        + calculate_cost_decimal(image_input_tokens, 0, image_input_rate, None)
                        + calculate_cost_decimal(0, img_output_tokens, None, effective_output_rate);
                    let total_input_tokens = text_input_tokens + image_input_tokens;
                    let ext = if content_type == "image/png" { "png" } else { "webp" };
                    let img_msg = messages::ActiveModel {
                        id: Set(img_message_id),
                        conversation_id: Set(conversation_id.clone()),
                        previous_message_id: Set(previous_message_id),
                        deleted: Set(false),
                        role: Set(ChatRole::Assistant),
                        message_content: Set(String::new()),
                        model_provider: Set(provider.clone()),
                        model_name: Set(model_name.clone()),
                        request_tokens: Set(total_input_tokens),
                        response_tokens: Set(img_output_tokens),
                        total_tokens: Set(total_input_tokens + img_output_tokens),
                        latency: Set(latency),
                        cost: Set(image_gen_cost),
                        request_id: Set(None),
                        tools_calls: Set(vec![]),
                        tools_results: Set(vec![]),
                        created_at: Set(now),
                        updated_at: Set(now),
                        metadata: Set(Some(json!({
                            "files": [File {
                                id: file_id,
                                name: format!("{file_id}.{ext}"),
                                content_type: content_type.clone(),
                                size: None,
                                openai_id: None,
                                base64: None,
                            }]
                        }))),
                    };
                    if let Err(e) = img_msg.insert(&app_state.database).await {
                        eprintln!("image gen message insert error: {e}");
                    }
                    if let Ok(Some(conversation)) = conversations::Entity::find_by_id(conversation_id.clone())
                        .one(&app_state.database)
                        .await
                    {
                        let new_total = conversation.total_cost + image_gen_cost;
                        let mut active_conv = conversation.into_active_model();
                        active_conv.total_cost = Set(new_total);
                        active_conv.updated_at = Set(now);
                        let _ = active_conv.update(&app_state.database).await;
                    }
                    if let Ok(Some(user)) = users::Entity::find_by_id(claims.user_id)
                        .one(&app_state.database)
                        .await
                    {
                        if let Some(department_id) = user.department_id {
                            if let Err(e) = refresh_department_budget_available(&app_state.database, department_id).await {
                                eprintln!("image gen budget refresh error: {e}");
                            } else if let Err(e) = emit_budget_alerts(&app_state, department_id).await {
                                eprintln!("image gen budget alert error: {e:?}");
                            }
                        }
                    }
                    if let Ok(data) = serde_json::to_string(&json!({
                        "message_id": img_message_id,
                        "file_id": file_id,
                        "content_type": content_type,
                        "cost": image_gen_cost.to_f32().unwrap_or(0.0),
                    })) {
                        yield Event::default()
                            .event(ChatStreamEvents::ImageGenerated.to_string())
                            .data(data);
                    }
                }
                Err(e) => {
                    eprintln!("image gen error: {e:#}");
                    let err_str = e.to_string();
                    let stream_err = if err_str.contains(" 429 ") {
                        ChatStreamError::ApiQuotaExhausted { provider: provider.clone() }
                    } else {
                        let message = err_str
                            .split_once(": ")
                            .and_then(|(_, body)| extract_llm_error_message(body))
                            .unwrap_or(err_str);
                        ChatStreamError::ProviderError { provider: provider.clone(), message }
                    };
                    if let Ok(data) = serde_json::to_string(&stream_err.to_response()) {
                        yield Event::default()
                            .event(ChatStreamEvents::AiError.to_string())
                            .data(data);
                    }
                }
            }
            yield Event::default().event(ChatStreamEvents::Done.to_string()).data("{}");
            return;
        }

        // Unwrap is safe — both are Some when !is_image_gen
        let stream_parser = stream_parser.expect("stream_parser is None only for image gen");

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

       let mut event_source = event_source.expect("event_source is None only for image gen");
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
                           let (passthrough, parse_events) = artifact_parser.push(text);
                           if !passthrough.is_empty() {
                               let chat_stream = ChatStream {
                                   id: None, title: None, message_id: None, is_new: None,
                                   content: Some(passthrough),
                                   input_tokens: None, output_tokens: None, latency_ms: None, cost: None,
                                   event: None, tool_call: None, tool_result: None,
                               };
                               yield Event::default().event(ChatStreamEvents::Delta.to_string()).data(chat_stream.to_string());
                           }
                           for parse_event in parse_events {
                               match parse_event {
                                   ArtifactParseEvent::Start { id, title, content_type } => {
                                       artifact_accumulator.insert(id.clone(), ArtifactAccum {
                                           title: title.clone(),
                                           content_type: content_type.clone(),
                                           content: String::new(),
                                       });
                                       if let Ok(data) = serde_json::to_string(&serde_json::json!({
                                           "id": id, "title": title, "contentType": content_type,
                                       })) {
                                           yield Event::default().event(ChatStreamEvents::ArtifactStart.to_string()).data(data);
                                       }
                                   }
                                   ArtifactParseEvent::Delta { id, chunk } => {
                                       if let Some(acc) = artifact_accumulator.get_mut(&id) {
                                           acc.content.push_str(&chunk);
                                       }
                                       if let Ok(data) = serde_json::to_string(&serde_json::json!({
                                           "id": id, "chunk": chunk,
                                       })) {
                                           yield Event::default().event(ChatStreamEvents::ArtifactDelta.to_string()).data(data);
                                       }
                                   }
                                   ArtifactParseEvent::End { id } => {
                                       if let Ok(data) = serde_json::to_string(&serde_json::json!({ "id": id })) {
                                           yield Event::default().event(ChatStreamEvents::ArtifactEnd.to_string()).data(data);
                                       }
                                   }
                               }
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

                           // Track every MCP tool_call SSE for pending flush — even partial
                           // streaming events where JSON is not yet complete.
                           if mcp_tooling_enabled
                               && !is_web_search
                               && resolve_mcp_tool_descriptor(&mcp_tool_lookup, &resolved_name).is_some()
                           {
                               if let Some(tool_id) = tool_input.tool_id.as_ref() {
                                   emitted_tc.entry(tool_id.clone())
                                       .or_insert_with(|| resolved_name.clone());
                               }
                           }

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
                                       emitted_tc.insert(tool_id.clone(), resolved_name.clone());

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
                               if let Some(id) = call.tool_id.as_ref() {
                                   emitted_tc.insert(id.clone(), call.tool_name.clone());
                               }
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
                               completed_tr.insert(resolved_id.clone());
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
                           if let Some(id) = result.tool_id.as_ref() {
                               completed_tr.insert(id.clone());
                           }
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
                       StreamParseResult::Error { kind, message } => {
                         eprintln!("Stream error ({:?}): {}", kind, message);
                         let data = serde_json::to_string(
                             &ChatStreamError::from_stream_error(*kind, provider.clone(), message.clone()).to_response()
                         ).unwrap_or_else(|_| "{}".to_string());
                         yield Event::default().event(ChatStreamEvents::AiError.to_string()).data(data);
                         stream_finished = true;
                         break;
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
                           let mut mistral_function_results_entries: Vec<MistralConversationFunctionResult> = Vec::new();
                           let mut gemini_model_tool_messages: Vec<GeminiContent> = Vec::new();
                           let mut gemini_function_response_messages: Vec<GeminiContent> = Vec::new();

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

                               if let Some(id) = call.tool_id.as_ref() {
                                   completed_tr.insert(id.clone());
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
                                   let thought_signature = call.raw.as_ref().and_then(|raw| {
                                       raw.get("thoughtSignature")
                                           .cloned()
                                           .or_else(|| raw.get("thought_signature").cloned())
                                   });
                                   let (model_turn, user_turn) = build_gemini_tool_messages(
                                       gemini_call_id,
                                       call.tool_name.clone(),
                                       args_for_call.clone(),
                                       &output_payload,
                                       thought_signature,
                                   );
                                   gemini_model_tool_messages.push(model_turn);
                                   gemini_function_response_messages.push(user_turn);
                               }

                               if let Some(call_id) = call.tool_id.clone() {
                                   if openai_tooling_enabled {
                                       tool_outputs.push(make_openai_function_output(
                                           call_id.clone(),
                                           &output_payload,
                                       ));
                                   }
                                   if anthropic_tooling_enabled {
                                       let (use_block, result_block) = make_anthropic_tool_blocks(
                                           call_id.clone(),
                                           call.tool_name.clone(),
                                           args_for_call.clone(),
                                           &output_payload,
                                           is_error,
                                       );
                                       anthropic_tool_use_blocks.push(use_block);
                                       anthropic_tool_result_blocks.push(result_block);
                                   }
                                   if mistral_use_conversations {
                                       mistral_function_results_entries.push(
                                           make_mistral_conversation_result(call_id, &output_payload),
                                       );
                                   } else if mistral_tooling_enabled {
                                       let (tool_call, tool_message) = make_mistral_tool_result(
                                           call_id,
                                           call.tool_name.clone(),
                                           args_for_call.clone(),
                                           &output_payload,
                                       );
                                       mistral_tool_calls.push(tool_call);
                                       mistral_tool_messages.push(tool_message);
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
                                       serde_json::to_value(&mistral_function_results_entries).ok();
                                   tool_round += 1;
                                   stream_should_continue = true;
                               }
                           } else if mistral_tooling_enabled {
                               if mistral_tool_messages.is_empty() {
                                   stream_finished = true;
                               } else {
                                   mistral_messages = Some(build_mistral_continuation(
                                       mistral_messages.take(),
                                       base_prompts.clone().unwrap_or_default(),
                                       stream_message_content.clone(),
                                       mistral_tool_calls,
                                       mistral_tool_messages,
                                   ));
                                   tool_round += 1;
                                   stream_should_continue = true;
                               }
                           } else if anthropic_tooling_enabled {
                               if anthropic_tool_result_blocks.is_empty() {
                                   stream_finished = true;
                               } else {
                                   let (messages, derived_system) = build_anthropic_continuation(
                                       anthropic_messages.take(),
                                       base_prompts.clone().unwrap_or_default(),
                                       anthropic_tool_use_blocks,
                                       anthropic_tool_result_blocks,
                                   );
                                   if let Some(s) = derived_system {
                                       anthropic_system_prompt = Some(s);
                                   }
                                   anthropic_messages = Some(messages);
                                   tool_round += 1;
                                   stream_should_continue = true;
                               }
                           } else if gemini_tooling_enabled {
                               if gemini_function_response_messages.is_empty() {
                                   stream_finished = true;
                               } else {
                                   let mut contents = gemini_contents.take().unwrap_or_default();
                                   contents.extend(gemini_model_tool_messages.into_iter().filter_map(|c| serde_json::to_value(c).ok()));
                                   contents.extend(gemini_function_response_messages.into_iter().filter_map(|c| serde_json::to_value(c).ok()));
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
                           let stream_err = if status == StatusCode::TOO_MANY_REQUESTS
                               || is_rate_limit_error(&body)
                           {
                               ChatStreamError::ApiQuotaExhausted { provider: provider.clone() }
                           } else {
                               let message = extract_llm_error_message(&body)
                                   .unwrap_or_else(|| format!("{} error from {} provider", status.as_u16(), provider));
                               ChatStreamError::ProviderError { provider: provider.clone(), message }
                           };
                           eprintln!(
                               "Streaming error for provider:{} status:{} body:{}",
                               provider, status, body
                           );
                           let data = serde_json::to_string(&stream_err.to_response()).unwrap_or_else(|_| "{}".to_string());
                           yield Event::default().event(ChatStreamEvents::AiError.to_string()).data(data);
                           stream_finished = true;
                           break;
                       }
                       _ => {
                           let stream_err = ChatStreamError::ConnectionFailed { provider: provider.clone() };
                           let data = serde_json::to_string(&stream_err.to_response()).unwrap_or_else(|_| "{}".to_string());
                           yield Event::default().event(ChatStreamEvents::AiError.to_string()).data(data);
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
                               let stream_err = ChatStreamError::ConnectionFailed { provider: provider.clone() };
                               let data = serde_json::to_string(&stream_err.to_response()).unwrap_or_else(|_| "{}".to_string());
                               yield Event::default().event(ChatStreamEvents::AiError.to_string()).data(data);
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
                               let stream_err = ChatStreamError::ConnectionFailed { provider: provider.clone() };
                               let data = serde_json::to_string(&stream_err.to_response()).unwrap_or_else(|_| "{}".to_string());
                               yield Event::default().event(ChatStreamEvents::AiError.to_string()).data(data);
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
                                   let stream_err = ChatStreamError::ConnectionFailed { provider: provider.clone() };
                                   let data = serde_json::to_string(&stream_err.to_response()).unwrap_or_else(|_| "{}".to_string());
                                   yield Event::default().event(ChatStreamEvents::AiError.to_string()).data(data);
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
                                   let stream_err = ChatStreamError::ConnectionFailed { provider: provider.clone() };
                                   let data = serde_json::to_string(&stream_err.to_response()).unwrap_or_else(|_| "{}".to_string());
                                   yield Event::default().event(ChatStreamEvents::AiError.to_string()).data(data);
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
                               let stream_err = ChatStreamError::ConnectionFailed { provider: provider.clone() };
                               let data = serde_json::to_string(&stream_err.to_response()).unwrap_or_else(|_| "{}".to_string());
                               yield Event::default().event(ChatStreamEvents::AiError.to_string()).data(data);
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
               let token_cost = if final_message_cost > Decimal::from(0) {
                   final_message_cost
               } else {
                   calculate_cost_decimal(
                       request_tokens,
                       response_tokens,
                       input_rate,
                       output_rate,
                   )
               };
               let message_cost = token_cost + image_gen_cost;
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
               // Flush any tool calls that were emitted to the client but never got a result.
               // Happens when the stream errors or is cancelled mid-execution.
               let mut flush_dirty = false;
               for (tool_id, tool_name) in emitted_tc.iter() {
                   if !completed_tr.contains(tool_id) {
                       let failed = ChatStreamToolResult {
                           tool_name: Some(tool_name.clone()),
                           tool_id: Some(tool_id.clone()),
                           kind: Some(ChatToolKind::Other),
                           status: Some("error".to_string()),
                           output: Some(json!({"error": "Tool execution did not complete"})),
                           web_search: None,
                       };
                       tool_results.push(serde_json::to_value(&failed).unwrap_or_else(|_| json!({})));
                       new_llm_message.tools_results = Set(tool_results.clone());
                       flush_dirty = true;
                       let chat_stream = ChatStream {
                           id: None, title: None, message_id: None, is_new: None,
                           content: None, input_tokens: None, output_tokens: None,
                           latency_ms: None, cost: None, event: None, tool_call: None,
                           tool_result: Some(failed),
                       };
                       yield Event::default().event(ChatStreamEvents::ToolResult.to_string()).data(chat_stream.to_string());
                   }
               }
               if flush_dirty {
                   new_llm_message.updated_at = Set(Utc::now());
                   let _ = new_llm_message.clone().update(&app_state.database).await;
               }
               let (remaining, flush_events) = artifact_parser.flush();
               if !remaining.is_empty() {
                   let chat_stream = ChatStream {
                       content: Some(remaining),
                       id: None, title: None, message_id: None, is_new: None,
                       input_tokens: None, output_tokens: None, latency_ms: None, cost: None,
                       event: None, tool_call: None, tool_result: None,
                   };
                   yield Event::default().event(ChatStreamEvents::Delta.to_string()).data(chat_stream.to_string());
               }
               for flush_event in flush_events {
                   match flush_event {
                       ArtifactParseEvent::Delta { id, chunk } => {
                           if let Some(acc) = artifact_accumulator.get_mut(&id) {
                               acc.content.push_str(&chunk);
                           }
                           if let Ok(data) = serde_json::to_string(&json!({ "id": id, "chunk": chunk })) {
                               yield Event::default().event(ChatStreamEvents::ArtifactDelta.to_string()).data(data);
                           }
                       }
                       ArtifactParseEvent::End { id } => {
                           if let Ok(data) = serde_json::to_string(&json!({ "id": id })) {
                               yield Event::default().event(ChatStreamEvents::ArtifactEnd.to_string()).data(data);
                           }
                       }
                       ArtifactParseEvent::Start { .. } => {}
                   }
               }
               if !artifact_accumulator.is_empty() {
                   let mut saved_artifacts: Vec<serde_json::Value> = Vec::new();
                   for (stream_id, acc) in artifact_accumulator.drain() {
                       let artifact_id = stream_id.parse::<Uuid>().unwrap_or_else(|_| Uuid::new_v4());
                       let file_id = Uuid::new_v4();
                       let ext = content_type_to_ext(&acc.content_type);
                       let safe_title = acc.title.chars().map(|c| match c {
                           '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                           other => other,
                       }).collect::<String>();
                       let filename = format!("{}.{}", safe_title, ext);
                       let user_folder = format!("/data/files/{}/artifact/{}", claims.user_id, file_id);
                       let local_path = format!("{}/{}", user_folder, filename);
                       if let Err(e) = tokio::fs::create_dir_all(&user_folder).await {
                           eprintln!("artifact dir create error: {e}");
                           continue;
                       }
                       if let Err(e) = tokio::fs::write(&local_path, acc.content.as_bytes()).await {
                           eprintln!("artifact file write error: {e}");
                           continue;
                       }
                       let file_size = acc.content.len() as i64;
                       let new_file = crate::models::files::ActiveModel {
                           id: Set(file_id),
                           user_id: Set(claims.user_id),
                           name: Set(filename.clone()),
                           content_type: Set(acc.content_type.clone()),
                           size: Set(file_size),
                           local_path: Set(local_path),
                           description: Set(Some(format!("Artifact: {}", acc.title))),
                           url: Set(None),
                           status: Set(crate::models::files::FileUploadStatus::Uploaded),
                           created_at: Set(Utc::now()),
                           updated_at: Set(Utc::now()),
                           metadata: Set(None),
                       };
                       if let Err(e) = new_file.insert(&app_state.database).await {
                           eprintln!("artifact file db insert error: {e}");
                           continue;
                       }
                       let new_artifact = crate::models::artifacts::ActiveModel {
                           id: Set(artifact_id),
                           file_id: Set(file_id),
                           message_id: Set(new_message_id),
                           conversation_id: Set(conversation_id),
                           title: Set(acc.title.clone()),
                           content_type: Set(acc.content_type.clone()),
                           created_at: Set(Utc::now()),
                           updated_at: Set(Utc::now()),
                       };
                       if let Err(e) = new_artifact.insert(&app_state.database).await {
                           eprintln!("artifact db insert error: {e}");
                           continue;
                       }
                       saved_artifacts.push(json!({
                           "id": artifact_id,
                           "file_id": file_id,
                           "title": acc.title,
                           "content_type": acc.content_type,
                       }));
                       if let Ok(data) = serde_json::to_string(&ArtifactSavedPayload {
                           id: artifact_id,
                           file_id,
                           title: acc.title.clone(),
                           content_type: acc.content_type.clone(),
                       }) {
                           yield Event::default().event(ChatStreamEvents::ArtifactSaved.to_string()).data(data);
                       }
                   }
                   if !saved_artifacts.is_empty() {
                       new_llm_message.metadata = Set(Some(json!({
                           "webSearch": req.web_search,
                           "artifacts": saved_artifacts,
                       })));
                       new_llm_message.updated_at = Set(Utc::now());
                       let _ = new_llm_message.clone().update(&app_state.database).await;
                   }
               }
               yield Event::default().event(ChatStreamEvents::Done.to_string()).data("{}");
               break;
           }
       }
    };
    let sse_response = Sse::new(sse_stream).keep_alive(KeepAlive::new());
    Ok(sse_response)
}
