// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    handlers::message::{
        delete_chat_message_by_id, edit_chat_message_by_id_and_stream, show_chat_branch,
    },
    state::SharedState,
};
use axum::{
    Router,
    routing::{delete, patch, put},
};

pub fn message_routes() -> Router<SharedState> {
    Router::new()
        .route(
            "/chat/{chat_id}/message/{message_id}",
            delete(delete_chat_message_by_id),
        )
        .route(
            "/chat/{chat_id}/message/{message_id}/stream",
            patch(edit_chat_message_by_id_and_stream),
        )
        .route("/chat/{chat_id}/branch", put(show_chat_branch))
}
