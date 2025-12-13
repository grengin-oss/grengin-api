use axum::{Router, routing::{delete, patch}};
use crate::{handlers::message::{delete_chat_message_by_id, edit_chat_message_by_id_and_stream}, state::SharedState};

pub fn message_routes() -> Router<SharedState> {
   Router::new()
    .route("/chat/{chat_id}/message/{message_id}",delete(delete_chat_message_by_id))
    .route("/chat/{chat_id}/message/{message_id}/stream", patch(edit_chat_message_by_id_and_stream))
}