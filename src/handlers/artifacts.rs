use crate::{
    auth::claims::Claims,
    dto::artifacts::{ArtifactListResponse, ArtifactResponse},
    error::AppError,
    services::artifacts_helpers::{
        delete_artifact_owned, get_artifact_owned, list_conversation_artifacts_owned,
    },
    state::SharedState,
};
use axum::{
    Json,
    extract::{Path, State},
};
use reqwest::StatusCode;
use uuid::Uuid;

#[utoipa::path(
    get,
    path = "/artifacts/{id}",
    tag = "artifacts",
    params(
        ("id" = Uuid, Path, description = "Artifact ID"),
    ),
    responses(
        (status = 200, description = "Artifact with content", body = ArtifactResponse),
        (status = 404, description = "Not found"),
        (status = 503, description = "Service unavailable"),
    )
)]
pub async fn get_artifact(
    claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<Json<ArtifactResponse>, AppError> {
    let row = get_artifact_owned(&app_state.database, id, claims.user_id)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AppError::ServiceTemporarilyUnavailable
        })?
        .ok_or(AppError::DbNotFound)?;

    Ok(Json(ArtifactResponse {
        id: row.artifact.id,
        file_id: row.artifact.file_id,
        message_id: row.artifact.message_id,
        conversation_id: row.artifact.conversation_id,
        title: row.artifact.title,
        content_type: row.artifact.content_type,
        content: row.content,
        created_at: row.artifact.created_at,
        updated_at: row.artifact.updated_at,
    }))
}

#[utoipa::path(
    get,
    path = "/conversations/{id}/artifacts",
    tag = "artifacts",
    params(
        ("id" = Uuid, Path, description = "Conversation ID"),
    ),
    responses(
        (status = 200, description = "List of artifacts for the conversation", body = ArtifactListResponse),
        (status = 404, description = "Conversation not found"),
        (status = 503, description = "Service unavailable"),
    )
)]
pub async fn list_conversation_artifacts(
    claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<Json<ArtifactListResponse>, AppError> {
    let rows = list_conversation_artifacts_owned(&app_state.database, id, claims.user_id)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AppError::ServiceTemporarilyUnavailable
        })?
        .ok_or(AppError::DbNotFound)?;

    let artifacts = rows
        .into_iter()
        .map(|a| ArtifactResponse {
            id: a.id,
            file_id: a.file_id,
            message_id: a.message_id,
            conversation_id: a.conversation_id,
            title: a.title,
            content_type: a.content_type,
            content: None,
            created_at: a.created_at,
            updated_at: a.updated_at,
        })
        .collect();

    Ok(Json(ArtifactListResponse { artifacts }))
}

#[utoipa::path(
    delete,
    path = "/artifacts/{id}",
    tag = "artifacts",
    params(
        ("id" = Uuid, Path, description = "Artifact ID"),
    ),
    responses(
        (status = 204, description = "Deleted successfully"),
        (status = 404, description = "Not found"),
        (status = 503, description = "Service unavailable"),
    )
)]
pub async fn delete_artifact(
    claims: Claims,
    Path(id): Path<Uuid>,
    State(app_state): State<SharedState>,
) -> Result<StatusCode, AppError> {
    delete_artifact_owned(&app_state.database, id, claims.user_id)
        .await
        .map_err(|e| {
            eprintln!("db error: {e}");
            AppError::ServiceTemporarilyUnavailable
        })?
        .ok_or(AppError::DbNotFound)?;

    Ok(StatusCode::NO_CONTENT)
}
