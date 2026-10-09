// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, DatabaseConnection,
    DatabaseTransaction, DeleteMany, EntityTrait, IntoActiveModel, PaginatorTrait, QueryFilter,
    QuerySelect, TransactionTrait, UpdateMany, sea_query,
};
use uuid::Uuid;

use crate::{
    dto::chat_stream::ChatInput,
    error::AppError,
    models::{
        conversation_summaries, conversations, messages,
        messages::{ChatRole, StopReason},
    },
    services::conversation_access::can_access_conversation,
};

pub async fn soft_delete_message(
    db: &DatabaseConnection,
    user_id: Uuid,
    chat_id: Uuid,
    message_id: Uuid,
) -> Result<(), AppError> {
    let (_, message) = find_message_in_conversation(db, user_id, chat_id, message_id).await?;
    let mut active_model = message.into_active_model();
    active_model.deleted = Set(true);
    active_model.updated_at = Set(Utc::now());
    active_model.update(db).await.map_err(|e| {
        eprintln!("db error :{}", e);
        AppError::DbTimeout
    })?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PendingEdit {
    conversation_id: Uuid,
    from: DateTime<Utc>,
}

pub async fn prepare_message_edit(
    db: &DatabaseConnection,
    user_id: Uuid,
    chat_id: Uuid,
    message_id: Uuid,
    req: &mut ChatInput,
) -> Result<PendingEdit, AppError> {
    let (conversation, message) =
        find_message_in_conversation(db, user_id, chat_id, message_id).await?;
    if !accepts_edits(&conversation) || !is_editable(&message) {
        return Err(AppError::ResourceNotFound);
    }
    pin_edit_conversation(req, chat_id);
    Ok(PendingEdit {
        conversation_id: chat_id,
        from: message.created_at,
    })
}

// Called by the stream once its provider, model, budget and access checks pass. Nothing is
// hidden here: the stream leaves the edited messages out of the model's context, writes the
// new turn hidden, and `commit` swaps the two in one transaction once the turn completes, so a
// failed attempt needs no rollback.
pub fn begin_pending_edit(
    conversation_id: Uuid,
    edit: Option<PendingEdit>,
) -> Option<EditInProgress> {
    let edit = edit_for_conversation(edit, conversation_id)?;
    Some(EditInProgress {
        conversation_id: edit.conversation_id,
        history_cutoff: edit.from,
        begun_at: Utc::now(),
        attempt_message_ids: Vec::new(),
    })
}

// A newer edit supersedes the attempt it interrupted, so that attempt stays hidden.
pub fn edit_commits(turn_failed: bool, stop_reason: Option<StopReason>) -> bool {
    !turn_failed && stop_reason != Some(StopReason::UserEdited)
}

#[derive(Debug)]
pub struct EditInProgress {
    conversation_id: Uuid,
    history_cutoff: DateTime<Utc>,
    begun_at: DateTime<Utc>,
    attempt_message_ids: Vec<Uuid>,
}

impl EditInProgress {
    pub fn history_cutoff(&self) -> DateTime<Utc> {
        self.history_cutoff
    }

    pub fn record_attempt_message(&mut self, message_id: Uuid) {
        self.attempt_message_ids.push(message_id);
    }

    pub async fn commit(self, db: &DatabaseConnection) -> Result<(), AppError> {
        let txn = db.begin().await.map_err(|e| {
            eprintln!("edit commit begin error: {e}");
            AppError::DbTimeout
        })?;
        // Edits of one conversation commit one at a time, so each sees a turn another revealed.
        conversations::Entity::find_by_id(self.conversation_id)
            .lock_exclusive()
            .one(&txn)
            .await
            .map_err(|e| {
                eprintln!("edit commit lock error: {e}");
                AppError::DbTimeout
            })?;
        for update in self.visibility_swap() {
            update.exec(&txn).await.map_err(|e| {
                eprintln!("edit commit update error: {e}");
                AppError::DbTimeout
            })?;
        }
        self.stale_summary().exec(&txn).await.map_err(|e| {
            eprintln!("edit commit summary reset error: {e}");
            AppError::DbTimeout
        })?;
        self.recount_conversation(&txn).await?;
        txn.commit().await.map_err(|e| {
            eprintln!("edit commit error: {e}");
            AppError::DbTimeout
        })
    }

    // The summary is cumulative and only ever extended, so one that already covers the edited
    // message would keep the replaced content; dropping it lets the next update rebuild it from
    // visible messages.
    fn stale_summary(&self) -> DeleteMany<conversation_summaries::Entity> {
        conversation_summaries::Entity::delete_many()
            .filter(conversation_summaries::Column::ConversationId.eq(self.conversation_id))
            .filter(
                Condition::any()
                    .add(conversation_summaries::Column::LastMessageAt.gte(self.history_cutoff))
                    .add(conversation_summaries::Column::LastMessageAt.is_null()),
            )
    }

    // message_count grows by the request's messages each turn, which in practice means user
    // messages, so it is recounted the same way from what is visible after the swap.
    async fn recount_conversation(&self, txn: &DatabaseTransaction) -> Result<(), AppError> {
        let visible = messages::Entity::find()
            .filter(messages::Column::ConversationId.eq(self.conversation_id))
            .filter(messages::Column::Deleted.eq(false));
        let user_messages = visible
            .clone()
            .filter(messages::Column::Role.eq(ChatRole::User))
            .count(txn)
            .await
            .map_err(|e| {
                eprintln!("edit commit message count error: {e}");
                AppError::DbTimeout
            })?;
        let last_message_at: Option<DateTime<Utc>> = visible
            .select_only()
            .expr(sea_query::Expr::col(messages::Column::CreatedAt).max())
            .into_tuple()
            .one(txn)
            .await
            .map_err(|e| {
                eprintln!("edit commit last message lookup error: {e}");
                AppError::DbTimeout
            })?
            .flatten();
        conversations::Entity::update_many()
            .filter(conversations::Column::Id.eq(self.conversation_id))
            .col_expr(
                conversations::Column::MessageCount,
                sea_query::Expr::value(i32::try_from(user_messages).unwrap_or(i32::MAX)),
            )
            .col_expr(
                conversations::Column::LastMessageAt,
                sea_query::Expr::value(last_message_at),
            )
            .col_expr(
                conversations::Column::UpdatedAt,
                sea_query::Expr::value(Utc::now()),
            )
            .exec(txn)
            .await
            .map_err(|e| {
                eprintln!("edit commit conversation recount error: {e}");
                AppError::DbTimeout
            })?;
        Ok(())
    }

    // The replaced messages are picked at commit, not at begin, because an edit interrupted by
    // this one may have revealed its turn in between; messages sent after this edit began stay.
    fn visibility_swap(&self) -> [UpdateMany<messages::Entity>; 2] {
        let replaced = messages::Entity::update_many()
            .filter(messages::Column::ConversationId.eq(self.conversation_id))
            .filter(messages::Column::Deleted.eq(false))
            .filter(messages::Column::CreatedAt.gte(self.history_cutoff))
            .filter(messages::Column::CreatedAt.lt(self.begun_at));
        let attempt = messages::Entity::update_many()
            .filter(messages::Column::Id.is_in(self.attempt_message_ids.iter().copied()));
        [set_deleted(replaced, true), set_deleted(attempt, false)]
    }
}

fn set_deleted(
    update: UpdateMany<messages::Entity>,
    deleted: bool,
) -> UpdateMany<messages::Entity> {
    update
        .col_expr(messages::Column::Deleted, sea_query::Expr::value(deleted))
        .col_expr(
            messages::Column::UpdatedAt,
            sea_query::Expr::value(Utc::now()),
        )
}

fn edit_for_conversation(edit: Option<PendingEdit>, conversation_id: Uuid) -> Option<PendingEdit> {
    edit.filter(|edit| edit.conversation_id == conversation_id)
}

// The stream rejects archived chats, so deleting the edited tail first would lose history.
fn accepts_edits(conversation: &conversations::Model) -> bool {
    conversation.archived_at.is_none()
}

// A replaced message is out of view and a reply is not the user's to rewrite, so editing
// either would cut the conversation at a point the user cannot see.
fn is_editable(message: &messages::Model) -> bool {
    message.role == ChatRole::User && !message.deleted
}

fn pin_edit_conversation(req: &mut ChatInput, chat_id: Uuid) {
    req.conversation_id = Some(chat_id);
}

async fn find_message_in_conversation(
    db: &DatabaseConnection,
    user_id: Uuid,
    chat_id: Uuid,
    message_id: Uuid,
) -> Result<(conversations::Model, messages::Model), AppError> {
    let (conversation, message) = conversations::Entity::find()
        .filter(conversations::Column::Id.eq(chat_id))
        .inner_join(messages::Entity)
        .filter(messages::Column::Id.eq(message_id))
        .select_also(messages::Entity)
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("db error :{}", e);
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;
    if !can_access_conversation(conversation.user_id, user_id) {
        return Err(AppError::ResourceNotFound);
    }
    let message = message.ok_or(AppError::ResourceNotFound)?;
    Ok((conversation, message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn chat_input(conversation_id: Option<Uuid>) -> ChatInput {
        serde_json::from_value(json!({
            "provider": "openai",
            "model_name": "gpt-test",
            "conversation_id": conversation_id,
            "messages": [],
        }))
        .expect("valid chat input")
    }

    fn conversation(archived: bool) -> conversations::Model {
        let now = Utc::now();
        conversations::Model {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            title: None,
            model_provider: "openai".to_string(),
            model_name: "gpt-test".to_string(),
            created_at: now,
            updated_at: now,
            last_message_at: None,
            archived_at: archived.then_some(now),
            message_count: 0,
            total_tokens: 0,
            total_cost: Default::default(),
            metadata: None,
            pinned: false,
        }
    }

    fn sql(update: &UpdateMany<messages::Entity>) -> String {
        use sea_orm::{DbBackend, QueryTrait};
        update.clone().build(DbBackend::Postgres).to_string()
    }

    fn edit_in_progress(conversation_id: Uuid) -> EditInProgress {
        let history_cutoff = Utc::now();
        EditInProgress {
            conversation_id,
            history_cutoff,
            begun_at: history_cutoff + chrono::Duration::seconds(5),
            attempt_message_ids: Vec::new(),
        }
    }

    fn message(role: ChatRole, deleted: bool) -> messages::Model {
        let now = Utc::now();
        messages::Model {
            id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            previous_message_id: None,
            deleted,
            role,
            message_content: "hello".to_string(),
            model_provider: "openai".to_string(),
            model_name: "gpt-test".to_string(),
            request_tokens: 0,
            response_tokens: 0,
            request_id: None,
            tools_calls: Vec::new(),
            tools_results: Vec::new(),
            created_at: now,
            updated_at: now,
            total_tokens: 0,
            latency: 0,
            cost: Default::default(),
            metadata: None,
        }
    }

    #[test]
    fn commit_hides_this_conversations_visible_messages_from_the_edit_until_it_began() {
        let conversation_id = Uuid::from_u128(3);
        let edit = edit_in_progress(conversation_id);

        let [hide, _] = edit.visibility_swap();
        let hide = sql(&hide);

        assert!(hide.contains(r#"SET "deleted" = TRUE"#), "{hide}");
        assert!(
            hide.contains(&format!(r#""conversationId" = '{conversation_id}'"#)),
            "{hide}"
        );
        assert!(hide.contains(r#""deleted" = FALSE"#), "{hide}");
        assert!(hide.contains(r#""createdAt" >= "#), "{hide}");
        assert!(hide.contains(r#""createdAt" < "#), "{hide}");
    }

    #[test]
    fn commit_reveals_exactly_the_attempt_messages() {
        let attempt = Uuid::from_u128(9);
        let mut edit = edit_in_progress(Uuid::nil());
        edit.record_attempt_message(attempt);

        let [_, reveal] = edit.visibility_swap();
        let reveal = sql(&reveal);

        assert!(reveal.contains(r#"SET "deleted" = FALSE"#), "{reveal}");
        assert!(
            reveal.contains(&format!(r#""id" IN ('{attempt}')"#)),
            "{reveal}"
        );
        assert!(!reveal.contains("conversationId"), "{reveal}");
        assert!(!reveal.contains("createdAt"), "{reveal}");
    }

    #[test]
    fn commit_drops_only_this_conversations_summary_that_covers_the_edit() {
        use sea_orm::{DbBackend, QueryTrait};
        let conversation_id = Uuid::from_u128(3);
        let edit = edit_in_progress(conversation_id);

        let sql = edit.stale_summary().build(DbBackend::Postgres).to_string();

        assert!(
            sql.starts_with(r#"DELETE FROM "conversation_summaries""#),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(r#""conversationId" = '{conversation_id}'"#)),
            "{sql}"
        );
        assert!(sql.contains(r#""lastMessageAt" >= "#), "{sql}");
        assert!(sql.contains(r#""lastMessageAt" IS NULL"#), "{sql}");
    }

    #[test]
    fn history_cutoff_is_the_edited_message_time() {
        let conversation_id = Uuid::new_v4();
        let from = Utc::now() - chrono::Duration::minutes(1);
        let edit = begin_pending_edit(
            conversation_id,
            Some(PendingEdit {
                conversation_id,
                from,
            }),
        )
        .expect("edit for this conversation");
        assert_eq!(edit.history_cutoff(), from);
        assert!(edit.begun_at > from);
    }

    #[test]
    fn plain_streams_begin_no_edit() {
        assert!(begin_pending_edit(Uuid::new_v4(), None).is_none());
    }

    #[test]
    fn completed_or_stopped_edits_commit() {
        assert!(edit_commits(false, None));
        assert!(edit_commits(false, Some(StopReason::UserCancelled)));
    }

    #[test]
    fn failed_edits_do_not_commit() {
        assert!(!edit_commits(true, None));
        assert!(!edit_commits(true, Some(StopReason::ProviderError)));
    }

    #[test]
    fn an_edit_interrupted_by_a_newer_edit_does_not_commit() {
        assert!(!edit_commits(false, Some(StopReason::UserEdited)));
    }

    #[test]
    fn visible_user_messages_are_editable() {
        assert!(is_editable(&message(ChatRole::User, false)));
    }

    #[test]
    fn assistant_replies_are_not_editable() {
        assert!(!is_editable(&message(ChatRole::Assistant, false)));
    }

    #[test]
    fn replaced_messages_are_not_editable() {
        assert!(!is_editable(&message(ChatRole::User, true)));
    }

    #[test]
    fn pending_edit_applies_only_to_the_conversation_it_was_prepared_for() {
        let conversation_id = Uuid::new_v4();
        let edit = PendingEdit {
            conversation_id,
            from: Utc::now(),
        };
        assert_eq!(
            edit_for_conversation(Some(edit), conversation_id),
            Some(edit)
        );
        assert_eq!(edit_for_conversation(Some(edit), Uuid::new_v4()), None);
    }

    #[test]
    fn plain_streams_have_no_edit_to_apply() {
        assert_eq!(edit_for_conversation(None, Uuid::new_v4()), None);
    }

    #[test]
    fn open_conversation_accepts_edits() {
        assert!(accepts_edits(&conversation(false)));
    }

    #[test]
    fn archived_conversation_rejects_edits_before_deleting_history() {
        assert!(!accepts_edits(&conversation(true)));
    }

    #[test]
    fn edit_stream_ignores_a_different_body_conversation_id() {
        let chat_id = Uuid::new_v4();
        let mut req = chat_input(Some(Uuid::new_v4()));

        pin_edit_conversation(&mut req, chat_id);

        assert_eq!(req.conversation_id, Some(chat_id));
    }

    #[test]
    fn edit_stream_targets_the_path_conversation_when_body_has_none() {
        let chat_id = Uuid::new_v4();
        let mut req = chat_input(None);

        pin_edit_conversation(&mut req, chat_id);

        assert_eq!(req.conversation_id, Some(chat_id));
    }
}
