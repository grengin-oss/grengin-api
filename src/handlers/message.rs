// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{claims::Claims, error::Error},
    dto::chat_stream::{ChatInput, ChatStream},
    error::{AppError, ErrorResponse},
    handlers::chat_stream::handle_chat_stream,
    services::message_helpers::*,
    state::SharedState,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
    response::{Sse, sse::Event},
};
use reqwest::StatusCode;
use std::convert::Infallible;
use uuid::Uuid;

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
    claims: Claims,
    Path((chat_id, message_id)): Path<(Uuid, Uuid)>,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AppError> {
    soft_delete_message(&app_state.database, claims.user_id, chat_id, message_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    patch,
    path = "/chat/{chat_id}/message/{message_id}/stream",
    tag = "chat",
    description = "Edits one of your messages and streams the new reply. The request body and the SSE events are the same as POST /chat/stream/{chat_id}; the conversation event has is_new=false. Only the stream-ending cases specific to edits are shown below.\n\n- Only a visible user message in a conversation that is not archived can be edited. An assistant reply, or a message already replaced by an earlier edit, returns 404.\n- The edit replaces the conversation from the edited message on. The new turn becomes visible when its reply completes, or when it is stopped with POST /chat/stream/{message_id}/cancel (the partial reply is kept); until then GET /chat/{chat_id} still returns the original messages. If the reply fails or the client disconnects, the original messages stay.\n- A reply still generating in this conversation is stopped once this edit's reply starts: that stream ends with cancelled and response_stopped (stop_reason user_edited). A request rejected before its reply starts (any 4xx below) leaves the running reply alone.\n- When edits of the conversation overlap, the newest request wins. An older edit's stream ends with response_stopped (stop_reason user_edited), without message_start if it was overtaken before its reply started, and its messages are never shown.",
    params(
        ("chat_id" = Uuid, Path, description = "Conversation containing the message"),
        ("message_id" = Uuid, Path, description = "Your message to edit; the reply is regenerated from here"),
    ),
    request_body = ChatInput,
    responses(
        (status = 200, content_type = "text/event-stream", body = ChatStream,
          description = "Server-sent events for the edit's new reply",
          examples(
            ("conversation" = (
              description = "event:conversation — emitted once at the start; is_new is always false for an edit.",
              value = json!({
                "event": "conversation",
                "data": { "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6", "title": "My first chat", "is_new": false }
              })
            )),
            ("message_start" = (
              description = "event:message_start — the new reply has started; use this message_id with POST /chat/stream/{message_id}/cancel.",
              value = json!({
                "event": "message_start",
                "data": { "message_id": "9b2f6c1e-4d7a-4f3b-8c55-2a1e0f9d7b10" }
              })
            )),
            ("stream_finished" = (
              description = "event:stream_finished — the reply is complete and, when it was not stopped, the edit is now visible. Contains token usage and cost.",
              value = json!({
                "event": "stream_finished",
                "data": { "message_id": "9b2f6c1e-4d7a-4f3b-8c55-2a1e0f9d7b10", "input_tokens": 320, "output_tokens": 85, "latency_ms": 1240, "cost": 0.00042 }
              })
            )),
            ("response_stopped_stop" = (
              description = "event:response_stopped — the user pressed Stop on this edit's reply. Sent after cancelled and before stream_finished; the edit is kept with the partial reply.",
              value = json!({
                "event": "response_stopped",
                "data": { "message_id": "9b2f6c1e-4d7a-4f3b-8c55-2a1e0f9d7b10", "stop_reason": "user_cancelled" }
              })
            )),
            ("response_stopped_superseded" = (
              description = "event:response_stopped — a newer edit of this conversation took over. This edit is discarded and never shown. If it was overtaken before its reply started, this is the only event between conversation (or skills) and done.",
              value = json!({
                "event": "response_stopped",
                "data": { "message_id": "9b2f6c1e-4d7a-4f3b-8c55-2a1e0f9d7b10", "stop_reason": "user_edited" }
              })
            )),
            ("done" = (
              description = "event:done — final event. Always emitted last.",
              value = json!({
                "event": "done",
                "data": {}
              })
            ))
          )
        ),
        (status = 400, content_type = "application/json", body = ErrorResponse, description = "Unknown LLM provider (code=4001) or empty messages (code=2002)"),
        (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
        (status = 403, content_type = "application/json", body = ErrorResponse, description = "LLM provider disabled by admin (code=4003), model not whitelisted by admin or not allowed for department (code=6002), or budget exceeded (code=6001)"),
        (status = 404, content_type = "application/json", body = ErrorResponse, description = "Conversation or message not found, conversation archived or owned by another user, message is an assistant reply, or message was already replaced by an earlier edit (code=1001)"),
        (status = 503, content_type = "application/json", body = ErrorResponse, description = "DB unavailable (code=5000) or timeout (code=5001)"),
    )
)]
pub async fn edit_chat_message_by_id_and_stream(
    claims: Claims,
    Path((chat_id, message_id)): Path<(Uuid, Uuid)>,
    State(app_state): State<SharedState>,
    Json(mut req): Json<ChatInput>,
) -> Result<Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>>, AppError> {
    let pending_edit = prepare_message_edit(
        &app_state.database,
        claims.user_id,
        chat_id,
        message_id,
        &mut req,
    )
    .await?;
    Ok(handle_chat_stream(
        claims,
        Some(Path(chat_id)),
        State(app_state),
        Some(Extension(pending_edit)),
        Json(req),
    )
    .await?)
}
