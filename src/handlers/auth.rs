// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::error::{AuthError, Error},
    dto::auth::{AuthToken, RefreshToken},
    services::auth_session::refresh_access_token,
    state::SharedState,
};
use axum::{Json, extract::State};
use reqwest::StatusCode;

#[utoipa::path(
    post,
    path = "/auth/refresh",
    tag = "admin",
    request_body = RefreshToken,
    responses(
       (status = 400, content_type = "application/json", body = Error, description = "Missing credentials (code=6102)"),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired refresh token (code=6103)"),
       (status = 401, content_type = "application/json", body = Error, description = "Account deactivated or suspended (code=6105)"),
       (status = 403, content_type = "application/json", body = Error, description = "Account pending approval (code=6107)"),
       (status = 404, content_type = "application/json", body = Error, description = "Email does not exist (code=6101)"),
       (status = 404, content_type = "application/json", body = Error, description = "User not found (code=5003)"),
       (status = 503, content_type = "application/json", body = Error, description = "DB timeout/unavailable (code=5001/5000)"),
    )
)]
pub async fn handle_refresh_token(
    State(app_state): State<SharedState>,
    Json(req): Json<RefreshToken>,
) -> Result<(StatusCode, Json<AuthToken>), AuthError> {
    let resp = refresh_access_token(&app_state, &req.refresh_token).await?;
    Ok((StatusCode::OK, Json(resp)))
}
