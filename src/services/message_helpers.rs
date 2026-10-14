// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, Condition, DatabaseConnection, EntityTrait,
    IntoActiveModel, QueryFilter, QuerySelect, TransactionTrait, UpdateMany, sea_query,
};
use uuid::Uuid;

use crate::{
    dto::chat_stream::ChatInput,
    error::AppError,
    models::{conversations, messages, messages::ChatRole},
    services::{
        conversation_access::can_access_conversation,
        message_branches::{
            hide_as_other_version, recount_conversation, show_messages, stale_summary,
        },
    },
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
    active_model.inactive_branch = Set(false);
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
    parent_id: Option<Uuid>,
    request: i32,
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
    let request = number_edit_request(db, chat_id).await?;
    pin_edit_conversation(req, chat_id);
    Ok(PendingEdit {
        conversation_id: chat_id,
        from: message.created_at,
        parent_id: message.previous_message_id,
        request,
    })
}

// Postgres numbers the requests, so which edit is newest never depends on a server clock.
async fn number_edit_request(db: &DatabaseConnection, chat_id: Uuid) -> Result<i32, AppError> {
    edit_request_counter(chat_id)
        .exec_with_returning(db)
        .await
        .map_err(|e| {
            eprintln!("edit request numbering error: {e}");
            AppError::DbTimeout
        })?
        .first()
        .map(|conversation| conversation.edit_requests)
        .ok_or(AppError::ResourceNotFound)
}

fn edit_request_counter(chat_id: Uuid) -> UpdateMany<conversations::Entity> {
    conversations::Entity::update_many()
        .filter(conversations::Column::Id.eq(chat_id))
        .col_expr(
            conversations::Column::EditRequests,
            sea_query::Expr::col(conversations::Column::EditRequests).add(1),
        )
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
        parent_id: edit.parent_id,
        request: edit.request,
        claimed: false,
        attempt_message_ids: Vec::new(),
    })
}

#[derive(Debug)]
pub struct EditInProgress {
    conversation_id: Uuid,
    history_cutoff: DateTime<Utc>,
    parent_id: Option<Uuid>,
    request: i32,
    claimed: bool,
    attempt_message_ids: Vec<Uuid>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum EditStatus {
    Current,
    Superseded,
}

impl EditInProgress {
    pub fn history_cutoff(&self) -> DateTime<Utc> {
        self.history_cutoff
    }

    pub fn request(&self) -> i32 {
        self.request
    }

    // The new version shares the edited message's parent, which makes the two siblings.
    pub fn parent_id(&self) -> Option<Uuid> {
        self.parent_id
    }

    pub fn record_attempt_message(&mut self, message_id: Uuid) {
        self.attempt_message_ids.push(message_id);
    }

    // Taken once this edit's reply has started, so a request that never got that far cannot
    // displace one that did. The newest request wins: an edit cannot claim a conversation a
    // newer edit has claimed, and loses its claim to one that claims before it commits,
    // whichever server instance the other edit ran on.
    pub async fn claim(&mut self, db: &DatabaseConnection) -> Result<EditStatus, AppError> {
        let claimed = self.claim_update().exec(db).await.map_err(|e| {
            eprintln!("edit claim error: {e}");
            AppError::DbTimeout
        })?;
        if claimed.rows_affected == 0 {
            return Ok(EditStatus::Superseded);
        }
        self.claimed = true;
        Ok(EditStatus::Current)
    }

    fn claim_update(&self) -> UpdateMany<conversations::Entity> {
        conversations::Entity::update_many()
            .filter(conversations::Column::Id.eq(self.conversation_id))
            .filter(
                Condition::any()
                    .add(conversations::Column::EditClaim.is_null())
                    .add(conversations::Column::EditClaim.lt(self.request)),
            )
            .col_expr(
                conversations::Column::EditClaim,
                sea_query::Expr::value(self.request),
            )
    }

    fn holds_claim(&self, latest_claim: Option<i32>) -> bool {
        self.claimed && latest_claim == Some(self.request)
    }

    pub async fn commit(self, db: &DatabaseConnection) -> Result<EditStatus, AppError> {
        let txn = db.begin().await.map_err(|e| {
            eprintln!("edit commit begin error: {e}");
            AppError::DbTimeout
        })?;
        // Commits of one conversation run one at a time, and a claim waits for one in progress.
        let latest_claim = conversations::Entity::find_by_id(self.conversation_id)
            .lock_exclusive()
            .one(&txn)
            .await
            .map_err(|e| {
                eprintln!("edit commit lock error: {e}");
                AppError::DbTimeout
            })?
            .and_then(|conversation| conversation.edit_claim);
        if !self.holds_claim(latest_claim) {
            return Ok(EditStatus::Superseded);
        }
        for update in self.visibility_swap() {
            update.exec(&txn).await.map_err(|e| {
                eprintln!("edit commit update error: {e}");
                AppError::DbTimeout
            })?;
        }
        stale_summary(self.conversation_id, self.history_cutoff)
            .exec(&txn)
            .await
            .map_err(|e| {
                eprintln!("edit commit summary reset error: {e}");
                AppError::DbTimeout
            })?;
        recount_conversation(&txn, self.conversation_id).await?;
        txn.commit().await.map_err(|e| {
            eprintln!("edit commit error: {e}");
            AppError::DbTimeout
        })?;
        Ok(EditStatus::Current)
    }

    // An edit replaces the conversation from the edited message on, so everything visible from
    // there becomes the previous version, including the turn of an edit that committed before
    // this one claimed. The attempt is still hidden at this point, and the bound is the edited
    // message itself rather than a clock reading, so servers with drifting clocks agree on it.
    fn visibility_swap(&self) -> [UpdateMany<messages::Entity>; 2] {
        let replaced = messages::Entity::update_many()
            .filter(messages::Column::ConversationId.eq(self.conversation_id))
            .filter(messages::Column::Deleted.eq(false))
            .filter(messages::Column::CreatedAt.gte(self.history_cutoff));
        let attempt = messages::Entity::update_many()
            .filter(messages::Column::Id.is_in(self.attempt_message_ids.iter().copied()));
        [hide_as_other_version(replaced), show_messages(attempt)]
    }
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
            edit_requests: 0,
            edit_claim: None,
        }
    }

    fn sql(update: &UpdateMany<messages::Entity>) -> String {
        use sea_orm::{DbBackend, QueryTrait};
        update.clone().build(DbBackend::Postgres).to_string()
    }

    fn edit_in_progress(conversation_id: Uuid) -> EditInProgress {
        EditInProgress {
            conversation_id,
            history_cutoff: Utc::now(),
            parent_id: None,
            request: 4,
            claimed: false,
            attempt_message_ids: Vec::new(),
        }
    }

    fn claimed() -> EditInProgress {
        let mut edit = edit_in_progress(Uuid::nil());
        edit.claimed = true;
        edit
    }

    fn message(role: ChatRole, deleted: bool) -> messages::Model {
        let now = Utc::now();
        messages::Model {
            id: Uuid::new_v4(),
            conversation_id: Uuid::new_v4(),
            previous_message_id: None,
            deleted,
            inactive_branch: false,
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
    fn commit_hides_this_conversations_visible_messages_from_the_edited_one_on() {
        let conversation_id = Uuid::from_u128(3);
        let edit = edit_in_progress(conversation_id);

        let [hide, _] = edit.visibility_swap();
        let hide = sql(&hide);

        assert!(hide.contains(r#"SET "deleted" = TRUE"#), "{hide}");
        assert!(hide.contains(r#""inactiveBranch" = TRUE"#), "{hide}");
        assert!(
            hide.contains(&format!(r#""conversationId" = '{conversation_id}'"#)),
            "{hide}"
        );
        assert!(hide.contains(r#""deleted" = FALSE"#), "{hide}");
        assert!(hide.contains(r#""createdAt" >= "#), "{hide}");
    }

    #[test]
    fn what_a_commit_replaces_never_depends_on_a_clock_reading() {
        let [hide, _] = edit_in_progress(Uuid::nil()).visibility_swap();
        let hide = sql(&hide);
        assert_eq!(hide.matches(r#""createdAt""#).count(), 1, "{hide}");
    }

    #[test]
    fn commit_reveals_exactly_the_attempt_messages() {
        let attempt = Uuid::from_u128(9);
        let mut edit = edit_in_progress(Uuid::nil());
        edit.record_attempt_message(attempt);

        let [_, reveal] = edit.visibility_swap();
        let reveal = sql(&reveal);

        assert!(reveal.contains(r#"SET "deleted" = FALSE"#), "{reveal}");
        assert!(reveal.contains(r#""inactiveBranch" = FALSE"#), "{reveal}");
        assert!(
            reveal.contains(&format!(r#""id" IN ('{attempt}')"#)),
            "{reveal}"
        );
        assert!(!reveal.contains("conversationId"), "{reveal}");
        assert!(!reveal.contains("createdAt"), "{reveal}");
    }

    #[test]
    fn history_cutoff_is_the_edited_message_time() {
        let conversation_id = Uuid::new_v4();
        let from = Utc::now() - chrono::Duration::minutes(1);
        let parent = Uuid::new_v4();
        let edit = begin_pending_edit(
            conversation_id,
            Some(PendingEdit {
                conversation_id,
                from,
                parent_id: Some(parent),
                request: 7,
            }),
        )
        .expect("edit for this conversation");
        assert_eq!(edit.history_cutoff(), from);
        assert_eq!(edit.request(), 7);
        assert!(!edit.claimed);
        assert_eq!(edit.parent_id(), Some(parent));
    }

    #[test]
    fn plain_streams_begin_no_edit() {
        assert!(begin_pending_edit(Uuid::new_v4(), None).is_none());
    }

    #[test]
    fn edit_requests_are_numbered_per_conversation() {
        use sea_orm::{DbBackend, QueryTrait};
        let conversation_id = Uuid::from_u128(3);

        let sql = edit_request_counter(conversation_id)
            .build(DbBackend::Postgres)
            .to_string();

        assert!(
            sql.contains(r#"SET "editRequests" = "editRequests" + 1"#),
            "{sql}"
        );
        assert!(
            sql.ends_with(&format!(r#""conversations"."id" = '{conversation_id}'"#)),
            "{sql}"
        );
    }

    #[test]
    fn an_edit_claims_only_its_conversation_and_only_over_older_requests() {
        use sea_orm::{DbBackend, QueryTrait};
        let conversation_id = Uuid::from_u128(3);

        let sql = edit_in_progress(conversation_id)
            .claim_update()
            .build(DbBackend::Postgres)
            .to_string();

        assert!(sql.contains(r#"SET "editClaim" = 4"#), "{sql}");
        assert!(
            sql.contains(&format!(r#""conversations"."id" = '{conversation_id}'"#)),
            "{sql}"
        );
        assert!(
            sql.contains(
                r#"("conversations"."editClaim" IS NULL OR "conversations"."editClaim" < 4)"#
            ),
            "{sql}"
        );
    }

    #[test]
    fn an_edit_still_holding_the_claim_commits() {
        assert!(claimed().holds_claim(Some(4)));
    }

    #[test]
    fn an_edit_claimed_by_a_newer_request_does_not_commit() {
        assert!(!claimed().holds_claim(Some(5)));
    }

    #[test]
    fn an_unclaimed_edit_does_not_commit() {
        assert!(!edit_in_progress(Uuid::nil()).holds_claim(Some(4)));
    }

    #[test]
    fn an_edit_of_a_deleted_conversation_does_not_commit() {
        assert!(!claimed().holds_claim(None));
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
            parent_id: None,
            request: 1,
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
