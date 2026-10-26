// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
    },
    dto::chat_shares::*,
    services::chat_shares_helpers,
    state::SharedState,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use uuid::Uuid;

#[utoipa::path(post, path = "/chat/{chat_id}/shares", tag = "chat shares",
    summary = "Create a frozen, sign-in-required chat share",
    description = "Owner only; requires chats:share. Organization includes every active account in this deployment. Department scope uses exact primary department membership, not descendants or roles. Public links and individual recipients are unsupported. Organization notifications are silent by default; notify=true requires an org-wide Super Admin. Department notifications default on. At most 100 links per chat. Link tokens must not be logged.",
    params(("chat_id" = Uuid, Path, description = "Owned, non-archived chat")),
    request_body(content = ShareCreateRequest, examples(
        ("organization" = (value = json!({"access":"organization","notify":false}))),
        ("departments" = (value = json!({"access":"department","department_ids":["3fa85f64-5717-4562-b3fc-2c963f66afa6"]})))
    )),
    responses((status = 201, description = "Stable encrypted-at-rest link to the current visible thread", body = ChatShareLink),
        (status = 400, description = "Invalid scope targets, expiry, snapshot bounds or share limit", body = Error),
        (status = 401, description = "Missing/invalid access token or inactive user", body = Error),
        (status = 403, description = "Missing share or organization-notification authorization", body = Error),
        (status = 404, description = "Chat not owned/available, invalid department or foreign attachment", body = Error),
        (status = 422, description = "Malformed request or unsupported field/scope"),
        (status = 429, description = "Per-user change limit; respect Retry-After"),
        (status = 503, description = "Persistence, required audit or notification write failed", body = Error)), security(("bearerAuth" = [])))]
pub async fn create_share(
    claims: Claims,
    State(state): State<SharedState>,
    Path(chat): Path<Uuid>,
    Json(request): Json<ShareCreateRequest>,
) -> Result<(StatusCode, Json<ChatShareLink>), AuthError> {
    Ok((
        StatusCode::CREATED,
        Json(chat_shares_helpers::create_share(&state, claims.user_id, chat, request).await?),
    ))
}

#[utoipa::path(get, path = "/chat/{chat_id}/shares", tag = "chat shares",
    summary = "List the owner's share links",
    description = "Returns stable links, department targets, expiry and revocation state, newest first. This owner-only response includes tokens in share_url: do not log or cache it.",
    params(("chat_id" = Uuid, Path)), responses((status = 200, body = Vec<ChatShareLink>),
        (status = 401, description = "Active sign-in required", body = Error),
        (status = 404, description = "Chat not owned or not found", body = Error),
        (status = 429, description = "Per-user read limit; respect Retry-After"),
        (status = 503, description = "Storage or token decryption unavailable", body = Error)), security(("bearerAuth" = [])))]
pub async fn list_shares(
    claims: Claims,
    State(state): State<SharedState>,
    Path(chat): Path<Uuid>,
) -> Result<Json<Vec<ChatShareLink>>, AuthError> {
    Ok(Json(
        chat_shares_helpers::list_shares(&state, claims.user_id, chat).await?,
    ))
}

#[utoipa::path(put, path = "/chat/{chat_id}/shares/{share_id}", tag = "chat shares",
    summary = "Update share access, expiry or frozen contents",
    description = "Owner with chats:share only. Keeps the same URL. refresh_snapshot explicitly captures the currently shown thread; ordinary chat changes never alter a snapshot. clear_expiry and expires_at cannot be combined. Scope toggles do not re-notify existing recipients or reset read state. Organization notification requests require Super Admin. Revoked links cannot be reopened.",
    params(("chat_id" = Uuid, Path), ("share_id" = Uuid, Path)),
    request_body(content = ShareUpdateRequest, examples(
        ("refresh" = (value = json!({"refresh_snapshot":true}))),
        ("organization" = (value = json!({"access":"organization","clear_expiry":true,"notify":false})))
    )), responses((status = 200, body = ChatShareLink),
        (status = 400, description = "Invalid scope, expiry or snapshot", body = Error),
        (status = 401, description = "Active sign-in required", body = Error),
        (status = 403, description = "Missing share or organization-notification authorization", body = Error),
        (status = 404, description = "Link not owned, revoked, or target unavailable", body = Error),
        (status = 422, description = "Malformed request or unsupported field/scope"),
        (status = 429, description = "Per-user change limit; respect Retry-After"),
        (status = 503, description = "Transactional write or snapshot storage unavailable", body = Error)), security(("bearerAuth" = [])))]
pub async fn update_share(
    claims: Claims,
    State(state): State<SharedState>,
    Path((chat, id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ShareUpdateRequest>,
) -> Result<Json<ChatShareLink>, AuthError> {
    Ok(Json(
        chat_shares_helpers::update_share(&state, claims.user_id, chat, id, request).await?,
    ))
}

#[utoipa::path(delete, path = "/chat/{chat_id}/shares/{share_id}", tag = "chat shares",
    summary = "Revoke a share link",
    description = "Idempotent for an existing owner link. Still allowed if chats:share was removed. Revocation removes sharing notifications and stops future access/forks; existing independent copies remain. Uses a separate revoke budget so exhausting link-change limits does not prevent revocation.",
    params(("chat_id" = Uuid, Path), ("share_id" = Uuid, Path)), responses((status = 204, description = "Revoked, including an already-revoked owner link"),
        (status = 401, description = "Active sign-in required", body = Error),
        (status = 404, description = "Link/chat not owned or not found", body = Error),
        (status = 429, description = "Per-user revoke limit; respect Retry-After"),
        (status = 503, description = "Required transactional write failed", body = Error)), security(("bearerAuth" = [])))]
pub async fn revoke_share(
    claims: Claims,
    State(state): State<SharedState>,
    Path((chat, id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, AuthError> {
    chat_shares_helpers::revoke_share(&state, claims.user_id, chat, id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/share/{token}", tag = "chat shares",
    summary = "View a frozen shared conversation",
    description = "Active signed-in viewers only. Returns visible user/assistant snapshot text, safe attachment metadata and artifacts, not tool data, system prompts, hidden branches or private associations. Owner must remain active; department membership and link expiry/revocation are checked live. Missing/denied links are indistinguishable. Responses are private/no-store.",
    params(("token" = String, Path, description = "Opaque 43-character link token; never log it")),
    responses((status = 200, body = SharedChatView), (status = 401, description = "Active sign-in required", body = Error),
        (status = 404, description = "Invalid, revoked, expired or inaccessible link", body = Error),
        (status = 429, description = "Per-user read limit; respect Retry-After"),
        (status = 503, description = "Snapshot storage unavailable", body = Error)), security(("bearerAuth" = [])))]
pub async fn view_share(
    viewer: Claims,
    State(state): State<SharedState>,
    Path(token): Path<String>,
) -> Result<Json<SharedChatView>, AuthError> {
    Ok(Json(
        chat_shares_helpers::view_share(&state, &token, &viewer).await?,
    ))
}

#[utoipa::path(get, path = "/share/{token}/files/{file_id}", tag = "chat shares",
    summary = "Download one snapshot attachment",
    description = "Requires current share access and a file identifier present in that snapshot. Deleted/unavailable source files return 404; inline snapshot files remain available. HTML uses attachment-only headers and nosniff. Downloads have a separate per-user budget from chat views.",
    params(("token" = String, Path), ("file_id" = Uuid, Path)), responses((status = 200, description = "Binary bytes with safe Content-Type and Content-Disposition", body = Vec<u8>, content_type = "application/octet-stream"),
        (status = 400, description = "File exceeds size bounds", body = Error),
        (status = 401, description = "Active sign-in required", body = Error),
        (status = 404, description = "Denied link, unknown or unavailable attachment", body = Error),
        (status = 429, description = "Per-user download limit; respect Retry-After"),
        (status = 503, description = "File storage unavailable", body = Error)), security(("bearerAuth" = [])))]
pub async fn download_share_file(
    viewer: Claims,
    State(state): State<SharedState>,
    Path((token, id)): Path<(String, Uuid)>,
) -> Result<Response, AuthError> {
    chat_shares_helpers::download_share_file(&state, &token, id, &viewer).await
}

#[utoipa::path(post, path = "/share/{token}/fork", tag = "chat shares",
    summary = "Continue a shared snapshot in a private conversation",
    description = "Copies messages and available attachments under fresh viewer-owned identifiers. Unavailable files/artifacts are omitted, not fatal. No projects, skills, MCP associations or source retrieval state are copied. Usage resets to zero. Access is rechecked before commit. Two copy slots are released at commit; optional background indexing has two independent slots and is skipped when busy. Indexing never delays new copies. Not idempotent: each successful call creates a new copy.",
    params(("token" = String, Path)), responses((status = 201, description = "New private conversation", body = ForkedChat, example = json!({"conversation_id":"3fa85f64-5717-4562-b3fc-2c963f66afa6"})),
        (status = 400, description = "Copy exceeds file bounds", body = Error),
        (status = 401, description = "Active sign-in required", body = Error),
        (status = 404, description = "Link denied, expired or revoked", body = Error),
        (status = 409, description = "Snapshot changed during preparation; retry", body = Error),
        (status = 429, description = "Per-user fork limit or copy slots busy; respect Retry-After"),
        (status = 503, description = "File storage or transaction write unavailable", body = Error)), security(("bearerAuth" = [])))]
pub async fn fork_share(
    claims: Claims,
    State(state): State<SharedState>,
    Path(token): Path<String>,
) -> Result<Response, AuthError> {
    chat_shares_helpers::fork_share(&state, &token, &claims).await
}

#[utoipa::path(get, path = "/me/shared-chats/{share_id}/link", tag = "chat shares",
    summary = "Resolve a sharing notification to its current URL",
    description = "Use payload.share_id from chat.shared notifications. The notification is not authorization: current account, owner, department, expiry and revocation rules are checked before returning a URL. Do not cache or log the returned token.",
    params(("share_id" = Uuid, Path)), responses((status = 200, body = SharedChatLink),
        (status = 401, description = "Active sign-in required", body = Error),
        (status = 404, description = "Share absent or no longer accessible", body = Error),
        (status = 429, description = "Per-user read limit; respect Retry-After"),
        (status = 503, description = "Storage/token decryption unavailable", body = Error)), security(("bearerAuth" = [])))]
pub async fn resolve_shared_link(
    viewer: Claims,
    State(state): State<SharedState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SharedChatLink>, AuthError> {
    Ok(Json(
        chat_shares_helpers::resolve_shared_link(&state, id, &viewer).await?,
    ))
}
