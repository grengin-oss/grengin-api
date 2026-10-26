// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::{
    auth::error::AuthError,
    dto::chat_shares::ShareNotificationPayload,
    models::{
        chat_share_departments,
        chat_shares::{self, ShareAccess},
        notifications,
        users::{self, UserStatus},
    },
    services::notifications::{NotificationEvent, to_notification_dto},
    state::SharedState,
};
use chrono::{DateTime, Utc};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    Set,
    sea_query::{Expr, OnConflict, Query},
};
use uuid::Uuid;

fn database_error(error: sea_orm::DbErr) -> AuthError {
    eprintln!("chat share notification operation failed: {error}");
    AuthError::DbTimeout
}

pub async fn clear<C: ConnectionTrait>(db: &C, share_id: Uuid) -> Result<(), AuthError> {
    notifications::Entity::delete_many()
        .filter(notifications::Column::ShareId.eq(share_id))
        .exec(db)
        .await
        .map_err(database_error)?;
    Ok(())
}

pub async fn reconcile<C: ConnectionTrait>(
    db: &C,
    share: &chat_shares::Model,
    departments: &[Uuid],
    notify: bool,
    now: DateTime<Utc>,
) -> Result<Vec<NotificationEvent>, AuthError> {
    if !notify
        || share.revoked_at.is_some()
        || share.expires_at.is_some_and(|expires| expires <= now)
    {
        return Ok(Vec::new());
    }
    let mut eligible = users::Entity::find()
        .filter(users::Column::Status.eq(UserStatus::Active))
        .filter(users::Column::Id.ne(share.owner_id));
    if share.access == ShareAccess::Department {
        if departments.is_empty() {
            return Ok(Vec::new());
        }
        eligible = eligible.filter(users::Column::DepartmentId.is_in(departments.iter().copied()));
    }
    // Keep old recipient rows so toggling scope cannot reset read status or ping them again.
    // Inbox queries and SSE delivery filter these retained rows against live eligibility.
    let payload = serde_json::to_value(ShareNotificationPayload { share_id: share.id })
        .map_err(|_| AuthError::ServiceTemporarilyUnavailable)?;
    let mut cursor = None;
    let mut events = Vec::new();
    loop {
        let mut page = eligible.clone();
        if let Some(cursor) = cursor {
            page = page.filter(users::Column::Id.gt(cursor));
        }
        let recipients = page
            .select_only()
            .columns([users::Column::Id, users::Column::DepartmentId])
            .order_by_asc(users::Column::Id)
            .limit(500)
            .into_tuple::<(Uuid, Option<Uuid>)>()
            .all(db)
            .await
            .map_err(database_error)?;
        if recipients.is_empty() {
            break;
        }
        cursor = recipients.last().map(|(id, _)| *id);
        let rows = recipients
            .into_iter()
            .map(|(user_id, department_id)| notifications::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(user_id),
                share_id: Set(Some(share.id)),
                department_id: Set(if share.access == ShareAccess::Department {
                    department_id
                } else {
                    None
                }),
                kind: Set("chat.shared".to_string()),
                title: Set("Chat shared with you".to_string()),
                body: Set("A chat is available to you in Grengin.".to_string()),
                payload: Set(payload.clone()),
                period_start: Set(share.created_at),
                created_at: Set(now),
                read_at: Set(None),
            })
            .collect::<Vec<_>>();
        // The unique share/recipient key preserves read status and suppresses refresh/retry duplicates.
        let inserted = notifications::Entity::insert_many(rows)
            .on_conflict(
                OnConflict::columns([
                    notifications::Column::ShareId,
                    notifications::Column::UserId,
                ])
                .do_nothing()
                .to_owned(),
            )
            .exec_with_returning_many(db)
            .await
            .map_err(database_error)?;
        events.extend(inserted.iter().map(|row| NotificationEvent {
            user_id: row.user_id,
            notification: to_notification_dto(row),
        }));
    }
    Ok(events)
}

pub fn publish(state: &SharedState, events: Vec<NotificationEvent>) {
    for event in events {
        let _ = state.notification_hub.send(event);
    }
}

pub fn visible_condition(user: Uuid) -> Condition {
    let active_owners = Query::select()
        .column(users::Column::Id)
        .from(users::Entity)
        .and_where(users::Column::Status.eq(UserStatus::Active))
        .to_owned();
    let membership = Query::select()
        .column(users::Column::DepartmentId)
        .from(users::Entity)
        .and_where(users::Column::Id.eq(user))
        .and_where(users::Column::Status.eq(UserStatus::Active))
        .to_owned();
    let departments = Query::select()
        .column(chat_share_departments::Column::ShareId)
        .from(chat_share_departments::Entity)
        .and_where(Expr::col(chat_share_departments::Column::DepartmentId).in_subquery(membership))
        .to_owned();
    let live = Query::select()
        .column(chat_shares::Column::Id)
        .from(chat_shares::Entity)
        .and_where(chat_shares::Column::RevokedAt.is_null())
        .cond_where(
            Condition::any()
                .add(chat_shares::Column::ExpiresAt.is_null())
                .add(chat_shares::Column::ExpiresAt.gt(Utc::now())),
        )
        .and_where(Expr::col(chat_shares::Column::OwnerId).in_subquery(active_owners))
        .cond_where(
            Condition::any()
                .add(chat_shares::Column::Access.eq(ShareAccess::Organization))
                .add(Expr::col(chat_shares::Column::Id).in_subquery(departments)),
        )
        .to_owned();
    Condition::any()
        .add(notifications::Column::ShareId.is_null())
        .add(Expr::col(notifications::Column::ShareId).in_subquery(live))
}

pub async fn event_is_visible(state: &SharedState, event: &NotificationEvent, user: Uuid) -> bool {
    if event.user_id != user {
        return false;
    }
    if event.notification.kind != "chat.shared" {
        return true;
    }
    match notifications::Entity::find_by_id(event.notification.id)
        .filter(notifications::Column::UserId.eq(user))
        .filter(visible_condition(user))
        .one(&state.database)
        .await
    {
        Ok(row) => row.is_some(),
        Err(error) => {
            eprintln!("sharing notification visibility check failed: {error}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_contains_no_chat_content_or_secret_link() {
        let id = Uuid::new_v4();
        let payload = serde_json::to_value(ShareNotificationPayload { share_id: id }).unwrap();
        assert_eq!(payload, serde_json::json!({"share_id":id}));
    }

    #[test]
    fn retained_notifications_are_filtered_by_live_scope_expiry_and_owner_status() {
        use sea_orm::{DbBackend, QueryTrait};
        let sql = notifications::Entity::find()
            .filter(visible_condition(Uuid::nil()))
            .build(DbBackend::Postgres)
            .to_string();
        for clause in [
            "\"shareId\" IS NULL",
            "\"revokedAt\" IS NULL",
            "\"expiresAt\"",
            "\"departmentId\"",
            "\"ownerId\"",
            "'active'",
            "'organization'",
        ] {
            assert!(sql.contains(clause), "{sql} is missing {clause}");
        }
        assert!(!sql.contains("user_role_assignments"));
    }
}
