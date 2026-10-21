// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    handlers::chat_shares::*, services::share_access::share_middleware, state::SharedState,
};
use axum::{
    Router,
    middleware::from_fn_with_state,
    routing::{get, post, put},
};

pub fn chat_shares_routes(state: SharedState) -> Router<SharedState> {
    let viewer = Router::new()
        .route("/me/shared-chats/{share_id}/link", get(resolve_shared_link))
        .route("/share/{token}", get(view_share))
        .route("/share/{token}/files/{file_id}", get(download_share_file))
        .route("/share/{token}/fork", post(fork_share));
    Router::new()
        .route(
            "/chat/{chat_id}/shares",
            get(list_shares).post(create_share),
        )
        .route(
            "/chat/{chat_id}/shares/{share_id}",
            put(update_share).delete(revoke_share),
        )
        .merge(viewer)
        .route_layer(from_fn_with_state(state, share_middleware))
}
