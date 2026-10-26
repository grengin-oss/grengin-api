// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::{
        claims::Claims,
        encryption::{decrypt_key, encrypt_key},
        error::AuthError,
        permissions::PERMISSION_CHATS_SHARE,
    },
    dto::{chat_shares::*, files::File},
    error::AppError,
    models::{
        artifacts, chat_share_departments,
        chat_shares::{self, ShareAccess},
        conversations, departments,
        files::{self, FileUploadStatus},
        messages::{self, ChatRole},
        users::{self, UserStatus},
    },
    services::{
        auth_audit::record_required_auth_event,
        authorization::{AuthorizationService, PermissionScopeMode},
        chat_share_notifications,
        file_download::download_response,
        file_storage::{
            FileWrite, StorageCategory, file_is_available, find_file_for_user, read_model_bytes,
            remove_model_file, safe_file_name, stage_file_bytes,
        },
    },
    state::SharedState,
};
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chrono::{DateTime, Utc};
use openssl::{rand::rand_bytes, sha::sha256};
use sea_orm::{
    AccessMode, ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait,
    DatabaseConnection, DbErr, EntityTrait, IntoActiveModel, IsolationLevel, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, TransactionTrait, prelude::Decimal,
};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};
use uuid::Uuid;

const MAX_MESSAGES: usize = 1000;
const MAX_FILES: usize = 100;
const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 50 * 1024 * 1024;

fn database_error(error: DbErr) -> AuthError {
    eprintln!("chat share database operation failed: {error}");
    AuthError::DbTimeout
}
fn file_error(error: AppError) -> AuthError {
    match error {
        AppError::ResourceNotFound | AppError::DbNotFound => AuthError::ResourceNotFound,
        AppError::ValidationEmptyField { field } | AppError::ValidationMissingField { field } => {
            AuthError::InvalidRequest { field }
        }
        _ => AuthError::ServiceTemporarilyUnavailable,
    }
}

fn token_hash(token: &str) -> Result<String, AuthError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| AuthError::ResourceNotFound)?;
    if bytes.len() != 32 || URL_SAFE_NO_PAD.encode(&bytes) != token {
        return Err(AuthError::ResourceNotFound);
    }
    Ok(sha256(token.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn generate_token() -> Result<String, AuthError> {
    let mut bytes = [0u8; 32];
    rand_bytes(&mut bytes).map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn expiry_valid(expiry: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Result<(), AuthError> {
    if expiry.is_some_and(|expiry| expiry <= now) {
        return Err(AuthError::InvalidRequest {
            field: "expires_at",
        });
    }
    Ok(())
}

fn link_is_live(share: &chat_shares::Model, now: DateTime<Utc>) -> bool {
    share.revoked_at.is_none() && share.expires_at.is_none_or(|expiry| expiry > now)
}

async fn owned_conversation<C: ConnectionTrait>(
    db: &C,
    chat: Uuid,
    owner: Uuid,
) -> Result<conversations::Model, AuthError> {
    conversations::Entity::find_by_id(chat)
        .filter(conversations::Column::UserId.eq(owner))
        .one(db)
        .await
        .map_err(database_error)?
        .ok_or(AuthError::ResourceNotFound)
}

async fn sharing_permission(db: &DatabaseConnection, user: Uuid) -> Result<(), AuthError> {
    AuthorizationService::new(db)
        .ensure_permission(
            user,
            PERMISSION_CHATS_SHARE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await
}

async fn notification_permission(
    state: &SharedState,
    owner: Uuid,
    access: ShareAccess,
    notify: Option<bool>,
) -> Result<bool, AuthError> {
    let notify = notify.unwrap_or(access == ShareAccess::Department);
    if notify
        && access == ShareAccess::Organization
        && !AuthorizationService::new(&state.database)
            .user_has_role_name(owner, crate::auth::permissions::ROLE_SUPER_ADMIN)
            .await?
    {
        return Err(AuthError::PermissionDenied);
    }
    Ok(notify)
}

fn share_url(base: &str, token: &str) -> Result<String, AuthError> {
    let base = url::Url::parse(base).map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
    {
        return Err(AuthError::ServiceTemporarilyUnavailable);
    }
    Ok(format!(
        "{}/share/{token}",
        base.origin().ascii_serialization()
    ))
}

fn link_response(
    state: &SharedState,
    share: chat_shares::Model,
    department_ids: Vec<Uuid>,
) -> Result<ChatShareLink, AuthError> {
    let token = decrypt_key(&state.settings.auth.app_key, &share.token)
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    Ok(ChatShareLink {
        id: share.id,
        conversation_id: share.conversation_id,
        share_url: share_url(&state.settings.auth.redirect_url, &token)?,
        access: share.access,
        department_ids,
        created_at: share.created_at,
        updated_at: share.updated_at,
        expires_at: share.expires_at,
        revoked_at: share.revoked_at,
    })
}

fn scope_departments(access: ShareAccess, ids: Vec<Uuid>) -> Result<Vec<Uuid>, AuthError> {
    if ids.len() > 100 {
        return Err(AuthError::InvalidRequest {
            field: "department_ids",
        });
    }
    let mut ids = ids;
    ids.sort_unstable();
    ids.dedup();
    if (access == ShareAccess::Department && ids.is_empty())
        || (access == ShareAccess::Organization && !ids.is_empty())
    {
        return Err(AuthError::InvalidRequest {
            field: "department_ids",
        });
    }
    Ok(ids)
}

async fn target_departments<C: ConnectionTrait>(
    db: &C,
    share: Uuid,
) -> Result<Vec<Uuid>, AuthError> {
    chat_share_departments::Entity::find()
        .filter(chat_share_departments::Column::ShareId.eq(share))
        .select_only()
        .column(chat_share_departments::Column::DepartmentId)
        .order_by_asc(chat_share_departments::Column::DepartmentId)
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(database_error)
}

async fn replace_departments<C: ConnectionTrait>(
    db: &C,
    share: Uuid,
    ids: &[Uuid],
) -> Result<(), AuthError> {
    if !ids.is_empty()
        && departments::Entity::find()
            .filter(departments::Column::Id.is_in(ids.iter().copied()))
            .count(db)
            .await
            .map_err(database_error)?
            != ids.len() as u64
    {
        return Err(AuthError::ResourceNotFound);
    }
    chat_share_departments::Entity::delete_many()
        .filter(chat_share_departments::Column::ShareId.eq(share))
        .exec(db)
        .await
        .map_err(database_error)?;
    for id in ids {
        chat_share_departments::ActiveModel {
            share_id: Set(share),
            department_id: Set(*id),
        }
        .insert(db)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

fn account_snapshot_bytes(used: &mut usize, additional: usize) -> Result<(), AuthError> {
    *used = used.saturating_add(additional);
    if *used > MAX_SNAPSHOT_BYTES {
        return Err(AuthError::InvalidRequest {
            field: "snapshot_size",
        });
    }
    Ok(())
}

fn register_attachment(
    seen: &mut HashMap<Uuid, (SharedFile, bool)>,
    file: &SharedFile,
    inline: bool,
) -> Result<(), AuthError> {
    if let Some((previous, was_inline)) = seen.get(&file.id) {
        if *was_inline != inline
            || previous.name != file.name
            || previous.content_type != file.content_type
            || previous.size != file.size
        {
            return Err(AuthError::InvalidRequest { field: "files" });
        }
    } else {
        seen.insert(file.id, (file.clone(), inline));
    }
    Ok(())
}

async fn capture_file(
    state: &SharedState,
    owner: Uuid,
    input: File,
    inline: &mut HashMap<Uuid, String>,
) -> Result<Option<SharedFile>, AuthError> {
    let (name, content_type, size) = if let Some(data) = input.base64 {
        if data.len() > MAX_SNAPSHOT_BYTES {
            return Err(AuthError::InvalidRequest {
                field: "snapshot_size",
            });
        }
        let bytes = STANDARD
            .decode(&data)
            .map_err(|_| AuthError::InvalidRequest { field: "files" })?;
        if let Some(previous) = inline.insert(input.id, data.clone())
            && previous != data
        {
            return Err(AuthError::InvalidRequest { field: "files" });
        }
        (input.name, input.content_type, Some(bytes.len()))
    } else {
        let Some(file) = available_owned_file(state, owner, input.id).await? else {
            return Ok(None);
        };
        (
            file.name,
            file.content_type,
            usize::try_from(file.size).ok(),
        )
    };
    if safe_file_name(&name).is_none() || size.is_none_or(|size| size > MAX_FILE_BYTES) {
        return Err(AuthError::InvalidRequest { field: "files" });
    }
    Ok(Some(SharedFile {
        id: input.id,
        name,
        content_type,
        size,
        download_url: None,
    }))
}

async fn available_owned_file(
    state: &SharedState,
    owner: Uuid,
    id: Uuid,
) -> Result<Option<files::Model>, AuthError> {
    let Some(file) = files::Entity::find_by_id(id)
        .one(&state.database)
        .await
        .map_err(database_error)?
    else {
        return Ok(None);
    };
    if file.user_id != owner {
        return Err(AuthError::ResourceNotFound);
    }
    if file.status != FileUploadStatus::Uploaded
        || !file_is_available(&state.settings.file_storage_root, &file)
            .await
            .map_err(file_error)?
    {
        return Ok(None);
    }
    Ok(Some(file))
}

async fn capture_snapshot<C: ConnectionTrait>(
    state: &SharedState,
    db: &C,
    conversation: conversations::Model,
) -> Result<serde_json::Value, AuthError> {
    if conversation.archived_at.is_some() {
        return Err(AuthError::ResourceNotFound);
    }
    let rows = messages::Entity::find()
        .filter(messages::Column::ConversationId.eq(conversation.id))
        .filter(messages::Column::Deleted.eq(false))
        .filter(messages::Column::InactiveBranch.eq(false))
        .filter(messages::Column::Role.is_in([ChatRole::User, ChatRole::Assistant]))
        .order_by_asc(messages::Column::CreatedAt)
        .order_by_asc(messages::Column::Id)
        .limit((MAX_MESSAGES + 1) as u64)
        .all(db)
        .await
        .map_err(database_error)?;
    if rows.is_empty() || rows.len() > MAX_MESSAGES {
        return Err(AuthError::InvalidRequest { field: "messages" });
    }
    let ids = rows.iter().map(|message| message.id).collect::<Vec<_>>();
    let artifacts = artifacts::Entity::find()
        .filter(artifacts::Column::ConversationId.eq(conversation.id))
        .filter(artifacts::Column::MessageId.is_in(ids))
        .limit((MAX_FILES + 1) as u64)
        .all(db)
        .await
        .map_err(database_error)?;
    if artifacts.len() > MAX_FILES {
        return Err(AuthError::InvalidRequest { field: "artifacts" });
    }
    let mut inline = HashMap::new();
    let mut captured = Vec::new();
    let mut file_ids = HashSet::new();
    let mut seen: HashMap<Uuid, (SharedFile, bool)> = HashMap::new();
    let mut used = conversation.title.as_ref().map_or(0, String::len);
    for message in rows {
        account_snapshot_bytes(&mut used, message.message_content.len())?;
        if let Some(value) = message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("files"))
        {
            account_snapshot_bytes(
                &mut used,
                serde_json::to_vec(value)
                    .map_err(|_| AuthError::InvalidRequest { field: "files" })?
                    .len(),
            )?;
        }
        let mut attachments = Vec::new();
        let input_files: Vec<File> = message
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("files"))
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| AuthError::InvalidRequest { field: "files" })?
            .unwrap_or_default();
        if input_files.len() > MAX_FILES {
            return Err(AuthError::InvalidRequest { field: "files" });
        }
        for file in input_files {
            file_ids.insert(file.id);
            let is_inline = file.base64.is_some();
            let captured = match seen.get(&file.id) {
                Some((previous, false)) if !is_inline => Some(previous.clone()),
                _ => capture_file(state, conversation.user_id, file, &mut inline).await?,
            };
            let Some(captured) = captured else { continue };
            register_attachment(&mut seen, &captured, is_inline)?;
            attachments.push(captured);
        }
        let mut message_artifacts = Vec::new();
        for artifact in artifacts
            .iter()
            .filter(|artifact| artifact.message_id == message.id)
        {
            let Some(file) =
                available_owned_file(state, conversation.user_id, artifact.file_id).await?
            else {
                continue;
            };
            if file.size < 0 || file.size as usize > MAX_SNAPSHOT_BYTES {
                return Err(AuthError::InvalidRequest {
                    field: "artifact_size",
                });
            }
            let bytes = match read_model_bytes(&state.settings.file_storage_root, &file).await {
                Ok(bytes) => bytes,
                Err(AppError::ResourceNotFound) => continue,
                Err(error) => return Err(file_error(error)),
            };
            account_snapshot_bytes(&mut used, bytes.len())?;
            let content = String::from_utf8(bytes).ok();
            file_ids.insert(file.id);
            register_attachment(
                &mut seen,
                &SharedFile {
                    id: file.id,
                    name: file.name.clone(),
                    content_type: file.content_type.clone(),
                    size: usize::try_from(file.size).ok(),
                    download_url: None,
                },
                false,
            )?;
            if !attachments
                .iter()
                .any(|attachment| attachment.id == file.id)
            {
                if let Some(file) = capture_file(
                    state,
                    conversation.user_id,
                    File {
                        id: file.id,
                        name: file.name,
                        content_type: file.content_type,
                        size: usize::try_from(file.size).ok(),
                        openai_id: None,
                        base64: None,
                    },
                    &mut inline,
                )
                .await?
                {
                    attachments.push(file);
                } else {
                    continue;
                }
            }
            message_artifacts.push(SharedArtifact {
                id: artifact.id,
                file_id: artifact.file_id,
                title: artifact.title.clone(),
                content_type: artifact.content_type.clone(),
                content,
            });
        }
        if file_ids.len() > MAX_FILES {
            return Err(AuthError::InvalidRequest { field: "files" });
        }
        captured.push(SharedMessage {
            id: message.id,
            role: message.role,
            model_provider: message.model_provider,
            model_name: message.model_name,
            created_at: message.created_at,
            text: message.message_content,
            files: attachments,
            artifacts: message_artifacts,
        });
    }
    let snapshot = ShareSnapshot {
        schema_version: 1,
        chat: SharedChatContent {
            title: conversation.title,
            model_provider: conversation.model_provider,
            model_name: conversation.model_name,
            messages: captured,
        },
        inline_files: inline,
    };
    let bytes =
        serde_json::to_vec(&snapshot).map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(AuthError::InvalidRequest {
            field: "snapshot_size",
        });
    }
    serde_json::from_slice(&bytes).map_err(|_| AuthError::ServiceTemporarilyUnavailable)
}

pub async fn create_share(
    state: &SharedState,
    owner: Uuid,
    chat: Uuid,
    request: ShareCreateRequest,
) -> Result<ChatShareLink, AuthError> {
    owned_conversation(&state.database, chat, owner).await?;
    sharing_permission(&state.database, owner).await?;
    let access = request.access.unwrap_or(ShareAccess::Organization);
    let notify = notification_permission(state, owner, access, request.notify).await?;
    let department_ids = scope_departments(access, request.department_ids.unwrap_or_default())?;
    let now = Utc::now();
    expiry_valid(request.expires_at, now)?;
    let token = generate_token()?;
    // Serializable predicate checks keep concurrent creates inside the per-chat share limit.
    let txn = state
        .database
        .begin_with_config(
            Some(IsolationLevel::Serializable),
            Some(AccessMode::ReadWrite),
        )
        .await
        .map_err(database_error)?;
    let conversation = conversations::Entity::find_by_id(chat)
        .filter(conversations::Column::UserId.eq(owner))
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(database_error)?
        .ok_or(AuthError::ResourceNotFound)?;
    if chat_shares::Entity::find()
        .filter(chat_shares::Column::ConversationId.eq(chat))
        .count(&txn)
        .await
        .map_err(database_error)?
        >= 100
    {
        return Err(AuthError::InvalidRequest {
            field: "share_limit",
        });
    }
    let snapshot = capture_snapshot(state, &txn, conversation).await?;
    let share = chat_shares::ActiveModel {
        id: Set(Uuid::new_v4()),
        conversation_id: Set(chat),
        owner_id: Set(owner),
        token_hash: Set(token_hash(&token)?),
        token: Set(encrypt_key(&state.settings.auth.app_key, token.as_bytes())
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?),
        access: Set(access),
        snapshot: Set(snapshot),
        expires_at: Set(request.expires_at),
        revoked_at: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(&txn)
    .await
    .map_err(database_error)?;
    replace_departments(&txn, share.id, &department_ids).await?;
    record_required_auth_event(
        &txn,
        "chat.share_created",
        Some(owner),
        json!({"share_id":share.id,"conversation_id":chat,"access":share.access,"department_ids":department_ids}),
    )
    .await?;
    let events =
        chat_share_notifications::reconcile(&txn, &share, &department_ids, notify, Utc::now())
            .await?;
    let response = link_response(state, share, department_ids)?;
    txn.commit().await.map_err(database_error)?;
    chat_share_notifications::publish(state, events);
    Ok(response)
}

pub async fn list_shares(
    state: &SharedState,
    owner: Uuid,
    chat: Uuid,
) -> Result<Vec<ChatShareLink>, AuthError> {
    owned_conversation(&state.database, chat, owner).await?;
    let shares = chat_shares::Entity::find()
        .filter(chat_shares::Column::ConversationId.eq(chat))
        .filter(chat_shares::Column::OwnerId.eq(owner))
        .order_by_desc(chat_shares::Column::CreatedAt)
        .all(&state.database)
        .await
        .map_err(database_error)?;
    let mut links = Vec::with_capacity(shares.len());
    for share in shares {
        let ids = target_departments(&state.database, share.id).await?;
        links.push(link_response(state, share, ids)?);
    }
    Ok(links)
}

pub async fn update_share(
    state: &SharedState,
    owner: Uuid,
    chat: Uuid,
    id: Uuid,
    request: ShareUpdateRequest,
) -> Result<ChatShareLink, AuthError> {
    sharing_permission(&state.database, owner).await?;
    if request.clear_expiry && request.expires_at.is_some() {
        return Err(AuthError::InvalidRequest {
            field: "expires_at",
        });
    }
    expiry_valid(request.expires_at, Utc::now())?;
    let txn = state
        .database
        .begin_with_config(
            Some(IsolationLevel::RepeatableRead),
            Some(AccessMode::ReadWrite),
        )
        .await
        .map_err(database_error)?;
    let conversation = owned_conversation(&txn, chat, owner).await?;
    let share = chat_shares::Entity::find_by_id(id)
        .filter(chat_shares::Column::ConversationId.eq(chat))
        .filter(chat_shares::Column::OwnerId.eq(owner))
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(database_error)?
        .ok_or(AuthError::ResourceNotFound)?;
    if share.revoked_at.is_some() {
        return Err(AuthError::ResourceNotFound);
    }
    let access = request.access.unwrap_or(share.access);
    let notify = notification_permission(state, owner, access, request.notify).await?;
    let ids = match request.department_ids {
        Some(ids) => ids,
        None if access == ShareAccess::Organization => Vec::new(),
        None => target_departments(&txn, id).await?,
    };
    let department_ids = scope_departments(access, ids)?;
    replace_departments(&txn, id, &department_ids).await?;
    let mut active: chat_shares::ActiveModel = share.into();
    if let Some(access) = request.access {
        active.access = Set(access);
    }
    if request.clear_expiry || request.expires_at.is_some() {
        active.expires_at = Set(request.expires_at);
    }
    if request.refresh_snapshot {
        active.snapshot = Set(capture_snapshot(state, &txn, conversation).await?);
    }
    active.updated_at = Set(Utc::now());
    let share = active.update(&txn).await.map_err(database_error)?;
    record_required_auth_event(&txn,"chat.share_updated",Some(owner),json!({"share_id":id,"conversation_id":chat,"access":share.access,"department_ids":department_ids,"snapshot_refreshed":request.refresh_snapshot})).await?;
    let events =
        chat_share_notifications::reconcile(&txn, &share, &department_ids, notify, Utc::now())
            .await?;
    let response = link_response(state, share, department_ids)?;
    txn.commit().await.map_err(database_error)?;
    chat_share_notifications::publish(state, events);
    Ok(response)
}

pub async fn revoke_share(
    state: &SharedState,
    owner: Uuid,
    chat: Uuid,
    id: Uuid,
) -> Result<(), AuthError> {
    let txn = state.database.begin().await.map_err(database_error)?;
    owned_conversation(&txn, chat, owner).await?;
    let share = chat_shares::Entity::find_by_id(id)
        .filter(chat_shares::Column::ConversationId.eq(chat))
        .filter(chat_shares::Column::OwnerId.eq(owner))
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(database_error)?
        .ok_or(AuthError::ResourceNotFound)?;
    if share.revoked_at.is_none() {
        let mut active: chat_shares::ActiveModel = share.into();
        active.revoked_at = Set(Some(Utc::now()));
        active.updated_at = Set(Utc::now());
        active.update(&txn).await.map_err(database_error)?;
        record_required_auth_event(
            &txn,
            "chat.share_revoked",
            Some(owner),
            json!({"share_id":id,"conversation_id":chat}),
        )
        .await?;
    }
    chat_share_notifications::clear(&txn, id).await?;
    txn.commit().await.map_err(database_error)
}

async fn allowed_share<C: ConnectionTrait>(
    db: &C,
    token: &str,
    viewer: &Claims,
    lock: bool,
) -> Result<chat_shares::Model, AuthError> {
    let mut query =
        chat_shares::Entity::find().filter(chat_shares::Column::TokenHash.eq(token_hash(token)?));
    if lock {
        query = query.lock_exclusive();
    }
    let share = query
        .one(db)
        .await
        .map_err(database_error)?
        .ok_or(AuthError::ResourceNotFound)?;
    check_share_access(db, &share, viewer).await?;
    Ok(share)
}

async fn check_share_access<C: ConnectionTrait>(
    db: &C,
    share: &chat_shares::Model,
    viewer: &Claims,
) -> Result<(), AuthError> {
    if !link_is_live(share, Utc::now()) {
        return Err(AuthError::ResourceNotFound);
    }
    let owner = users::Entity::find_by_id(share.owner_id)
        .select_only()
        .column(users::Column::Status)
        .into_tuple::<UserStatus>()
        .one(db)
        .await
        .map_err(database_error)?;
    if owner != Some(UserStatus::Active) {
        return Err(AuthError::ResourceNotFound);
    }
    if share.access == ShareAccess::Department && viewer.user_id != share.owner_id {
        let department = users::Entity::find_by_id(viewer.user_id)
            .select_only()
            .column(users::Column::DepartmentId)
            .into_tuple::<Option<Uuid>>()
            .one(db)
            .await
            .map_err(database_error)?
            .flatten()
            .ok_or(AuthError::ResourceNotFound)?;
        if chat_share_departments::Entity::find_by_id((share.id, department))
            .one(db)
            .await
            .map_err(database_error)?
            .is_none()
        {
            return Err(AuthError::ResourceNotFound);
        }
    }
    Ok(())
}

pub async fn resolve_shared_link(
    state: &SharedState,
    id: Uuid,
    viewer: &Claims,
) -> Result<SharedChatLink, AuthError> {
    let share = chat_shares::Entity::find_by_id(id)
        .one(&state.database)
        .await
        .map_err(database_error)?
        .ok_or(AuthError::ResourceNotFound)?;
    check_share_access(&state.database, &share, viewer).await?;
    let token = decrypt_key(&state.settings.auth.app_key, &share.token)
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    Ok(SharedChatLink {
        share_id: id,
        share_url: share_url(&state.settings.auth.redirect_url, &token)?,
    })
}

fn stored_snapshot(share: &chat_shares::Model) -> Result<ShareSnapshot, AuthError> {
    let snapshot: ShareSnapshot = serde_json::from_value(share.snapshot.clone())
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    if snapshot.schema_version != 1 {
        return Err(AuthError::ServiceTemporarilyUnavailable);
    }
    Ok(snapshot)
}

pub async fn view_share(
    state: &SharedState,
    token: &str,
    viewer: &Claims,
) -> Result<SharedChatView, AuthError> {
    let share = allowed_share(&state.database, token, viewer, false).await?;
    let snapshot = stored_snapshot(&share)?;
    let mut available = HashSet::new();
    for file in snapshot
        .chat
        .messages
        .iter()
        .flat_map(|message| &message.files)
    {
        if available.contains(&file.id) {
            continue;
        }
        if snapshot.inline_files.contains_key(&file.id)
            || available_owned_file(state, share.owner_id, file.id)
                .await?
                .is_some()
        {
            available.insert(file.id);
        }
    }
    let mut chat = snapshot.chat;
    for message in &mut chat.messages {
        message.files.retain(|file| available.contains(&file.id));
        message
            .artifacts
            .retain(|artifact| available.contains(&artifact.file_id));
    }
    for file in chat
        .messages
        .iter_mut()
        .flat_map(|message| &mut message.files)
    {
        file.download_url = Some(format!("/share/{token}/files/{}", file.id));
    }
    Ok(SharedChatView {
        share_id: share.id,
        access: share.access,
        created_at: share.created_at,
        updated_at: share.updated_at,
        expires_at: share.expires_at,
        chat,
    })
}

async fn snapshot_file(
    state: &SharedState,
    share: &chat_shares::Model,
    snapshot: &ShareSnapshot,
    id: Uuid,
) -> Result<(SharedFile, Vec<u8>), AuthError> {
    let file = snapshot
        .chat
        .messages
        .iter()
        .flat_map(|message| &message.files)
        .find(|file| file.id == id)
        .cloned()
        .ok_or(AuthError::ResourceNotFound)?;
    let bytes = if let Some(data) = snapshot.inline_files.get(&id) {
        STANDARD
            .decode(data)
            .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?
    } else {
        let stored = find_file_for_user(&state.database, share.owner_id, id)
            .await
            .map_err(file_error)?;
        if stored.size < 0 || stored.size as usize > MAX_FILE_BYTES {
            return Err(AuthError::InvalidRequest { field: "file_size" });
        }
        read_model_bytes(&state.settings.file_storage_root, &stored)
            .await
            .map_err(file_error)?
    };
    if bytes.len() > MAX_FILE_BYTES {
        return Err(AuthError::InvalidRequest { field: "file_size" });
    }
    Ok((file, bytes))
}

pub async fn download_share_file(
    state: &SharedState,
    token: &str,
    id: Uuid,
    viewer: &Claims,
) -> Result<Response, AuthError> {
    let share = allowed_share(&state.database, token, viewer, false).await?;
    let (file, bytes) = snapshot_file(state, &share, &stored_snapshot(&share)?, id).await?;
    let now = Utc::now();
    let model = files::Model {
        id: file.id,
        user_id: share.owner_id,
        name: file.name,
        content_type: file.content_type,
        size: bytes.len() as i64,
        local_path: String::new(),
        description: None,
        url: None,
        sha256: None,
        status: FileUploadStatus::Uploaded,
        created_at: now,
        updated_at: now,
        metadata: None,
    };
    Ok(download_response(&model, bytes))
}

async fn rebuild_copy_index(
    state: &SharedState,
    viewer: Uuid,
    chat: Uuid,
    targets: Vec<crate::services::rag::EmbeddingTarget>,
    source: &SharedChatContent,
) -> Result<(), AppError> {
    use crate::services::{
        chat_stream_helpers::{
            DepartmentBudgetGate, department_budget_status, ensure_model_whitelisted,
        },
        department_policies::check_model_allowed,
    };
    let user = users::Entity::find_by_id(viewer)
        .one(&state.database)
        .await
        .map_err(|_| AppError::DbTimeout)?
        .ok_or(AppError::ResourceNotFound)?;
    if user.status != UserStatus::Active {
        return Err(AppError::ResourceNotFound);
    }
    if let Some(department) = user.department_id {
        let (gate, _, _) = department_budget_status(&state.database, department).await?;
        if gate == DepartmentBudgetGate::Block {
            return Err(AppError::DepartmentBudgetExceeded);
        }
    }
    let provider = state
        .settings
        .rag
        .summary_llm_provider
        .as_deref()
        .unwrap_or(&source.model_provider)
        .to_lowercase();
    let model = state
        .settings
        .rag
        .summary_llm_model
        .as_deref()
        .unwrap_or(&source.model_name);
    if !check_model_allowed(&state.database, user.department_id, &provider, model)
        .await
        .map_err(|_| AppError::DbTimeout)?
    {
        return Err(AppError::DepartmentModelNotAllowed {
            provider,
            model: model.to_string(),
        });
    }
    ensure_model_whitelisted(state, &provider, &provider, model, &[model]).await?;
    crate::services::rag::embed_messages(state, targets).await?;
    crate::services::rag::update_conversation_summary(state, chat, &provider, model).await
}

async fn cleanup_fork_files(state: &SharedState, files: &HashMap<Uuid, files::Model>) {
    for file in files.values() {
        if let Err(error) = remove_model_file(&state.settings.file_storage_root, file).await {
            eprintln!("failed fork file cleanup: {error:?}");
        }
    }
}

async fn prepare_fork_files(
    state: &SharedState,
    share: &chat_shares::Model,
    snapshot: &ShareSnapshot,
    viewer: Uuid,
) -> Result<HashMap<Uuid, files::Model>, AuthError> {
    let mut staged = HashMap::new();
    let result: Result<(), AuthError> = async {
        let mut total = 0usize;
        let mut visited = HashSet::new();
        for source in snapshot
            .chat
            .messages
            .iter()
            .flat_map(|message| &message.files)
        {
            if !visited.insert(source.id) {
                continue;
            }
            let (file, bytes) = match snapshot_file(state, share, snapshot, source.id).await {
                Ok(result) => result,
                Err(AuthError::ResourceNotFound) => continue,
                Err(error) => return Err(error),
            };
            total = total.saturating_add(bytes.len());
            if total > MAX_FILE_BYTES {
                return Err(AuthError::InvalidRequest { field: "fork_size" });
            }
            let saved = stage_file_bytes(
                &state.settings.file_storage_root,
                viewer,
                FileWrite {
                    id: Uuid::new_v4(),
                    category: StorageCategory::File,
                    name: &file.name,
                    content_type: &file.content_type,
                    bytes: &bytes,
                    content_sha256: None,
                    description: None,
                    metadata: None,
                },
            )
            .await
            .map_err(file_error)?;
            staged.insert(source.id, saved);
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        cleanup_fork_files(state, &staged).await;
        return Err(error);
    }
    Ok(staged)
}

fn start_fork_index<F>(
    copy_permit: OwnedSemaphorePermit,
    slots: Arc<Semaphore>,
    work: F,
    timeout: Duration,
) -> Option<JoinHandle<()>>
where
    F: Future<Output = Result<(), AppError>> + Send + 'static,
{
    // The private copy is committed; optional model work must not reserve a copy slot.
    drop(copy_permit);
    let permit = match slots.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => {
            eprintln!("shared chat copy saved; optional indexing capacity is busy");
            return None;
        }
    };
    Some(tokio::spawn(async move {
        let _permit = permit;
        if !matches!(tokio::time::timeout(timeout, work).await, Ok(Ok(()))) {
            eprintln!("shared chat copy saved; optional RAG rebuild did not complete");
        }
    }))
}

pub async fn fork_share(
    state: &SharedState,
    token: &str,
    viewer: &Claims,
) -> Result<Response, AuthError> {
    let share = allowed_share(&state.database, token, viewer, false).await?;
    let permit = match state.share_fork_slots.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return Ok(crate::services::share_access::fork_busy_response()),
    };
    let snapshot = stored_snapshot(&share)?;
    let staged = prepare_fork_files(state, &share, &snapshot, viewer.user_id).await?;
    let txn = match state.database.begin().await {
        Ok(txn) => txn,
        Err(error) => {
            cleanup_fork_files(state, &staged).await;
            return Err(database_error(error));
        }
    };
    let id = Uuid::new_v4();
    let now = Utc::now();
    let mut indexing = Vec::new();
    let result: Result<(), AuthError> = async {
        // Approval, revocation, expiry and snapshot changes are rechecked after slow disk I/O.
        let current = allowed_share(&txn, token, viewer, true).await?;
        if current.snapshot != share.snapshot {
            return Err(AuthError::DbConflict);
        }
        conversations::ActiveModel {
            id: Set(id),
            user_id: Set(viewer.user_id),
            title: Set(snapshot.chat.title.clone()),
            model_provider: Set(snapshot.chat.model_provider.clone()),
            model_name: Set(snapshot.chat.model_name.clone()),
            created_at: Set(now),
            updated_at: Set(now),
            last_message_at: Set(Some(now)),
            archived_at: Set(None),
            message_count: Set(snapshot
                .chat
                .messages
                .iter()
                .filter(|message| message.role == ChatRole::User)
                .count() as i32),
            total_tokens: Set(0),
            total_cost: Set(Decimal::ZERO),
            metadata: Set(None),
            pinned: Set(false),
            edit_requests: Set(0),
            edit_claim: Set(None),
        }
        .insert(&txn)
        .await
        .map_err(database_error)?;
        let mut file_map = HashMap::new();
        for (source_id, file) in &staged {
            file.clone()
                .into_active_model()
                .insert(&txn)
                .await
                .map_err(database_error)?;
            file_map.insert(
                *source_id,
                File {
                    id: file.id,
                    name: file.name.clone(),
                    content_type: file.content_type.clone(),
                    size: Some(file.size as usize),
                    base64: None,
                    openai_id: None,
                },
            );
        }
        let mut previous = None;
        for (index, message) in snapshot.chat.messages.iter().enumerate() {
            let message_id = Uuid::new_v4();
            let at = now + chrono::Duration::microseconds(index as i64);
            let attachments = message
                .files
                .iter()
                .filter_map(|file| file_map.get(&file.id).cloned())
                .collect::<Vec<_>>();
            let mut copied_artifacts = Vec::new();
            for artifact in &message.artifacts {
                let Some(file) = file_map.get(&artifact.file_id) else {
                    continue;
                };
                copied_artifacts.push(crate::dto::chat::ArtifactMeta {
                    id: Uuid::new_v4(),
                    file_id: file.id,
                    title: artifact.title.clone(),
                    content_type: artifact.content_type.clone(),
                });
            }
            messages::ActiveModel {
                id: Set(message_id),
                conversation_id: Set(id),
                previous_message_id: Set(previous),
                deleted: Set(false),
                inactive_branch: Set(false),
                role: Set(message.role),
                message_content: Set(message.text.clone()),
                model_provider: Set(message.model_provider.clone()),
                model_name: Set(message.model_name.clone()),
                request_tokens: Set(0),
                response_tokens: Set(0),
                request_id: Set(None),
                tools_calls: Set(Vec::new()),
                tools_results: Set(Vec::new()),
                created_at: Set(at),
                updated_at: Set(at),
                total_tokens: Set(0),
                latency: Set(0),
                cost: Set(Decimal::ZERO),
                metadata: Set(Some(
                    json!({"files":attachments,"artifacts":copied_artifacts}),
                )),
            }
            .insert(&txn)
            .await
            .map_err(database_error)?;
            for artifact in copied_artifacts {
                artifacts::ActiveModel {
                    id: Set(artifact.id),
                    file_id: Set(artifact.file_id),
                    message_id: Set(message_id),
                    conversation_id: Set(id),
                    title: Set(artifact.title),
                    content_type: Set(artifact.content_type),
                    created_at: Set(at),
                    updated_at: Set(at),
                }
                .insert(&txn)
                .await
                .map_err(database_error)?;
            }
            previous = Some(message_id);
            indexing.push(crate::services::rag::EmbeddingTarget {
                message_id,
                conversation_id: id,
                role: message.role,
                content: message.text.clone(),
                created_at: at,
            });
        }
        let last = now
            + chrono::Duration::microseconds(snapshot.chat.messages.len().saturating_sub(1) as i64);
        conversations::Entity::update_many()
            .col_expr(
                conversations::Column::LastMessageAt,
                sea_orm::sea_query::Expr::value(last),
            )
            .filter(conversations::Column::Id.eq(id))
            .exec(&txn)
            .await
            .map_err(database_error)?;
        record_required_auth_event(
            &txn,
            "chat.share_forked",
            Some(viewer.user_id),
            json!({"share_id":share.id,"conversation_id":id}),
        )
        .await?;
        Ok(())
    }
    .await;
    if let Err(error) = result {
        let _ = txn.rollback().await;
        cleanup_fork_files(state, &staged).await;
        return Err(error);
    }
    // Commit errors can have an unknown outcome. Keep bytes rather than unlink a committed file.
    txn.commit().await.map_err(database_error)?;
    if state.settings.rag.enabled {
        let slots = state.share_index_slots.clone();
        let state = state.clone();
        let user = viewer.user_id;
        start_fork_index(
            permit,
            slots,
            async move { rebuild_copy_index(&state, user, id, indexing, &snapshot.chat).await },
            Duration::from_secs(30),
        );
    } else {
        drop(permit);
    }
    Ok((
        StatusCode::CREATED,
        Json(ForkedChat {
            conversation_id: id,
        }),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn blocked_indexing_releases_copy_slots_and_has_its_own_capacity() {
        let copies = Arc::new(Semaphore::new(2));
        let indexes = Arc::new(Semaphore::new(2));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let copy = copies.clone().try_acquire_owned().unwrap();
            tasks.push(
                start_fork_index(
                    copy,
                    indexes.clone(),
                    std::future::pending(),
                    Duration::from_secs(30),
                )
                .unwrap(),
            );
        }
        assert_eq!(copies.available_permits(), 2);
        assert_eq!(indexes.available_permits(), 0);
        let copy = copies
            .clone()
            .try_acquire_owned()
            .expect("another user can still copy");
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let work_ran = ran.clone();
        assert!(
            start_fork_index(
                copy,
                indexes.clone(),
                async move {
                    work_ran.store(true, std::sync::atomic::Ordering::SeqCst);
                    Ok(())
                },
                Duration::from_secs(30)
            )
            .is_none()
        );
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(copies.available_permits(), 2);
        for task in tasks {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        assert_eq!(indexes.available_permits(), 2);
    }

    #[tokio::test]
    async fn index_slots_release_on_completion_error_and_timeout() {
        let copies = Arc::new(Semaphore::new(2));
        let indexes = Arc::new(Semaphore::new(2));
        for result in [Ok(()), Err(AppError::ServiceTemporarilyUnavailable)] {
            let copy = copies.clone().try_acquire_owned().unwrap();
            start_fork_index(
                copy,
                indexes.clone(),
                async move { result },
                Duration::from_secs(1),
            )
            .unwrap()
            .await
            .unwrap();
            assert_eq!(copies.available_permits(), 2);
            assert_eq!(indexes.available_permits(), 2);
        }
        let copy = copies.clone().try_acquire_owned().unwrap();
        let task = start_fork_index(
            copy,
            indexes.clone(),
            std::future::pending(),
            Duration::from_millis(1),
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(copies.available_permits(), 2);
        assert_eq!(indexes.available_permits(), 2);
    }
    #[test]
    fn sharing_scopes_reject_removed_targets_and_require_departments() {
        for access in ["public", "user", "users", "role", "roles"] {
            assert!(
                serde_json::from_value::<ShareCreateRequest>(json!({"access":access})).is_err()
            );
            assert!(
                serde_json::from_value::<ShareUpdateRequest>(json!({"access":access})).is_err()
            );
        }
        for key in ["user_ids", "role_ids"] {
            let body = json!({key:[Uuid::new_v4()]});
            assert!(serde_json::from_value::<ShareCreateRequest>(body.clone()).is_err());
            assert!(serde_json::from_value::<ShareUpdateRequest>(body).is_err());
        }
        let id = Uuid::new_v4();
        assert!(scope_departments(ShareAccess::Department, vec![]).is_err());
        assert!(scope_departments(ShareAccess::Organization, vec![id]).is_err());
        assert!(scope_departments(ShareAccess::Organization, vec![]).is_ok());
        assert_eq!(
            scope_departments(ShareAccess::Department, vec![id, id]).unwrap(),
            vec![id]
        );
        assert!(scope_departments(ShareAccess::Department, vec![id; 101]).is_err());
    }
    #[test]
    fn attachments_cannot_change_identity_or_switch_between_inline_and_stored() {
        let mut seen = HashMap::new();
        let file = SharedFile {
            id: Uuid::new_v4(),
            name: "test.txt".into(),
            content_type: "text/plain".into(),
            size: Some(3),
            download_url: None,
        };
        assert!(register_attachment(&mut seen, &file, false).is_ok());
        assert!(register_attachment(&mut seen, &file, false).is_ok());
        assert!(register_attachment(&mut seen, &file, true).is_err());
        let mut renamed = file.clone();
        renamed.name = "other.txt".into();
        assert!(register_attachment(&mut seen, &renamed, false).is_err());
    }

    #[test]
    fn snapshot_budget_cannot_overflow_or_accept_oversized_content() {
        let mut used = MAX_SNAPSHOT_BYTES - 1;
        assert!(account_snapshot_bytes(&mut used, 1).is_ok());
        assert!(account_snapshot_bytes(&mut used, 1).is_err());
        assert!(account_snapshot_bytes(&mut used, usize::MAX).is_err());
    }
    #[test]
    fn tokens_are_random_canonical_and_hashes_do_not_reveal_them() {
        let a = generate_token().unwrap();
        let b = generate_token().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43);
        assert_eq!(token_hash(&a).unwrap().len(), 64);
        assert_ne!(token_hash(&a).unwrap(), a);
        for bad in ["", "../secret", "short", &format!("{a}=")] {
            assert!(token_hash(bad).is_err());
        }
    }
    #[test]
    fn expired_and_revoked_links_fail_closed_at_the_boundary() {
        let now = Utc::now();
        let mut row = chat_shares::Model {
            id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            owner_id: Uuid::new_v4(),
            token_hash: String::new(),
            token: String::new(),
            access: ShareAccess::Organization,
            snapshot: json!({}),
            created_at: now,
            updated_at: now,
            expires_at: None,
            revoked_at: None,
        };
        assert!(link_is_live(&row, now));
        row.expires_at = Some(now);
        assert!(!link_is_live(&row, now));
        row.expires_at = Some(now + chrono::Duration::seconds(1));
        assert!(link_is_live(&row, now));
        row.revoked_at = Some(now);
        assert!(!link_is_live(&row, now));
        assert!(expiry_valid(Some(now), now).is_err());
    }
    #[test]
    fn link_origins_are_from_the_deployment_not_callback_paths_or_queries() {
        assert_eq!(
            share_url("https://app.example.com/auth/callback?code=secret", "token").unwrap(),
            "https://app.example.com/share/token"
        );
        assert!(share_url("https://user:secret@app.example.com", "token").is_err());
        assert!(share_url("javascript:alert(1)", "token").is_err());
    }
}
