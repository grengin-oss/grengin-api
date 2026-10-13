// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::claims::Claims,
    dto::chat_stream::{ChatInput, ChatStream},
    error::AppError,
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
    params(
        ("chat_id" = Uuid, Path, description = "Unique identifier for the conversation"),
        ("message_id" = Uuid, Path, description = "Unique identifier for the message"),
    ),
    request_body = ChatInput,
    responses(
        (status = 200, content_type = "text/event-stream", body = ChatStream),
        (status = 503, description = "Oops! We're experiencing some technical issues. Please try again later."),
        (status = 404, description = "Resource not found"),
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
