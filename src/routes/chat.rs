use axum::{Router, middleware::from_extractor, routing::{delete, get, post}};
use crate::{auth::claims::Claims, handlers::chat::{delete_chat_by_id, get_chat_by_id, get_chats, handle_chat_stream, update_chat_by_id}, state::SharedState};

pub fn chat_routes() -> Router<SharedState> {
   Router::new()
    .route("/chat/stream",post(handle_chat_stream))
    .route("/chat",get(get_chats))
    .route("/chat/{chat_id}", delete(delete_chat_by_id).get(get_chat_by_id).put(update_chat_by_id))
    .route_layer(from_extractor::<Claims>())
}