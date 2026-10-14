// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait,
    ActiveValue::{NotSet, Set},
    ColumnTrait, Condition, ConnectionTrait, DatabaseConnection, DatabaseTransaction, DeleteMany,
    EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
    UpdateMany, sea_query,
};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::{
    dto::chat::MessageVersions,
    error::AppError,
    models::{conversation_summaries, conversations, messages, messages::ChatRole},
    services::conversation_access::can_access_conversation,
};

// Conversations are trees, as in Claude and ChatGPT: every message points to its parent, an edit
// adds a sibling of the edited message under the same parent, and the thread on screen is the
// path of visible messages. Messages of the versions not on screen are hidden with
// `inactive_branch` set, which is what sets them apart from deleted messages and from edit
// attempts that never finished.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    Shown,
    OtherVersion,
    Gone,
}

impl Placement {
    fn of(deleted: bool, inactive_branch: bool) -> Self {
        match (deleted, inactive_branch) {
            (false, _) => Self::Shown,
            (true, true) => Self::OtherVersion,
            (true, false) => Self::Gone,
        }
    }

    fn can_show(self) -> bool {
        self != Self::Gone
    }
}

#[derive(Debug, Clone)]
struct BranchNode {
    id: Uuid,
    parent_id: Option<Uuid>,
    role: ChatRole,
    placement: Placement,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

type BranchRow = (
    Uuid,
    Option<Uuid>,
    ChatRole,
    bool,
    bool,
    DateTime<Utc>,
    DateTime<Utc>,
);

impl BranchNode {
    fn from_row(
        (id, parent_id, role, deleted, inactive_branch, created_at, updated_at): BranchRow,
    ) -> Self {
        Self {
            id,
            parent_id,
            role,
            placement: Placement::of(deleted, inactive_branch),
            created_at,
            updated_at,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct BranchChange {
    show: Vec<Uuid>,
    hide: Vec<Uuid>,
    since: Option<DateTime<Utc>>,
}

struct MessageTree {
    nodes: HashMap<Uuid, BranchNode>,
    children: HashMap<Option<Uuid>, Vec<Uuid>>,
    // Latest time anything showable under a message (itself included) was last touched. A version
    // is hidden or shown with a fresh `updatedAt`, so this points at the thread seen most recently.
    last_seen: HashMap<Uuid, DateTime<Utc>>,
}

impl MessageTree {
    fn new(nodes: Vec<BranchNode>) -> Self {
        let mut children: HashMap<Option<Uuid>, Vec<Uuid>> = HashMap::new();
        let mut ordered = nodes.clone();
        ordered.sort_by_key(|node| (node.created_at, node.id));
        for node in &ordered {
            children.entry(node.parent_id).or_default().push(node.id);
        }
        let nodes: HashMap<Uuid, BranchNode> =
            nodes.into_iter().map(|node| (node.id, node)).collect();
        let last_seen = last_seen_under(&nodes, &children);
        Self {
            nodes,
            children,
            last_seen,
        }
    }

    fn versions(&self) -> HashMap<Uuid, MessageVersions> {
        self.nodes
            .values()
            .filter(|node| node.placement == Placement::Shown)
            .filter_map(|node| {
                let siblings: Vec<Uuid> = self
                    .children
                    .get(&node.parent_id)?
                    .iter()
                    .filter(|id| {
                        self.nodes.get(id).is_some_and(|sibling| {
                            sibling.role == node.role && sibling.placement.can_show()
                        })
                    })
                    .copied()
                    .collect();
                let position = siblings.iter().position(|id| *id == node.id)?;
                (siblings.len() > 1).then(|| {
                    (
                        node.id,
                        MessageVersions {
                            current: position as u32 + 1,
                            total: siblings.len() as u32,
                            message_ids: siblings,
                        },
                    )
                })
            })
            .collect()
    }

    // The new thread is the target, its ancestors, and below it the thread on screen if the
    // target is on screen, otherwise the one seen most recently.
    fn select(&self, target: Uuid) -> Result<BranchChange, AppError> {
        if !self
            .nodes
            .get(&target)
            .is_some_and(|node| node.placement.can_show())
        {
            return Err(AppError::ResourceNotFound);
        }
        let mut thread = HashSet::new();
        let mut cursor = Some(target);
        while let Some(id) = cursor {
            if !thread.insert(id) {
                break;
            }
            cursor = self.nodes.get(&id).and_then(|node| node.parent_id);
        }
        let mut cursor = target;
        while let Some(next) = self.next_in_thread(cursor) {
            if !thread.insert(next) {
                break;
            }
            cursor = next;
        }

        let mut change = BranchChange::default();
        let mut changed: Vec<&BranchNode> = Vec::new();
        for node in self.nodes.values() {
            match (node.placement, thread.contains(&node.id)) {
                (Placement::OtherVersion, true) => {
                    change.show.push(node.id);
                    changed.push(node);
                }
                (Placement::Shown, false) => {
                    change.hide.push(node.id);
                    changed.push(node);
                }
                _ => {}
            }
        }
        change.since = changed.iter().map(|node| node.created_at).min();
        Ok(change)
    }

    fn next_in_thread(&self, id: Uuid) -> Option<Uuid> {
        let children = self.children.get(&Some(id))?;
        let shown = children.iter().find(|child| {
            self.nodes
                .get(child)
                .is_some_and(|node| node.placement == Placement::Shown)
        });
        shown.copied().or_else(|| {
            children
                .iter()
                .filter_map(|child| self.last_seen.get(child).map(|seen| (*seen, *child)))
                .max()
                .map(|(_, child)| child)
        })
    }
}

fn last_seen_under(
    nodes: &HashMap<Uuid, BranchNode>,
    children: &HashMap<Option<Uuid>, Vec<Uuid>>,
) -> HashMap<Uuid, DateTime<Utc>> {
    let mut last_seen: HashMap<Uuid, DateTime<Utc>> = HashMap::new();
    let mut visited: HashSet<Uuid> = HashSet::new();
    for root in nodes.keys() {
        let mut stack = vec![(*root, false)];
        while let Some((id, expanded)) = stack.pop() {
            if expanded {
                let own = nodes
                    .get(&id)
                    .filter(|node| node.placement.can_show())
                    .map(|node| node.updated_at);
                let below = children
                    .get(&Some(id))
                    .into_iter()
                    .flatten()
                    .filter_map(|child| last_seen.get(child).copied())
                    .max();
                if let Some(seen) = own.into_iter().chain(below).max() {
                    last_seen.insert(id, seen);
                }
            } else if visited.insert(id) {
                stack.push((id, true));
                for child in children.get(&Some(id)).into_iter().flatten() {
                    if !visited.contains(child) {
                        stack.push((*child, false));
                    }
                }
            }
        }
    }
    last_seen
}

async fn load_tree<C: ConnectionTrait>(
    conn: &C,
    conversation_id: Uuid,
) -> Result<MessageTree, AppError> {
    let rows: Vec<BranchRow> = messages::Entity::find()
        .select_only()
        .columns([
            messages::Column::Id,
            messages::Column::PreviousMessageId,
            messages::Column::Role,
            messages::Column::Deleted,
            messages::Column::InactiveBranch,
            messages::Column::CreatedAt,
            messages::Column::UpdatedAt,
        ])
        .filter(messages::Column::ConversationId.eq(conversation_id))
        .into_tuple()
        .all(conn)
        .await
        .map_err(|e| {
            eprintln!("message tree lookup error: {e}");
            AppError::DbTimeout
        })?;
    Ok(MessageTree::new(
        rows.into_iter().map(BranchNode::from_row).collect(),
    ))
}

pub async fn load_message_versions(
    db: &DatabaseConnection,
    conversation_id: Uuid,
) -> Result<HashMap<Uuid, MessageVersions>, AppError> {
    Ok(load_tree(db, conversation_id).await?.versions())
}

// The message a new turn continues: the last one on screen when it is sent.
pub async fn last_shown_message<C: ConnectionTrait>(
    db: &C,
    conversation_id: Uuid,
) -> Result<Option<Uuid>, AppError> {
    messages::Entity::find()
        .select_only()
        .column(messages::Column::Id)
        .filter(messages::Column::ConversationId.eq(conversation_id))
        .filter(messages::Column::Deleted.eq(false))
        .order_by_desc(messages::Column::CreatedAt)
        .order_by_desc(messages::Column::Id)
        .into_tuple::<Uuid>()
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("last shown message lookup error: {e}");
            AppError::DbTimeout
        })
}

#[derive(Debug, PartialEq, Eq)]
struct ThreadSpot {
    parent_id: Option<Uuid>,
    shown: bool,
}

// A message joins the end of the thread on screen, unless the turn it follows has been switched
// or edited away meanwhile; then it stays with that turn, in the version no longer shown.
fn thread_spot(follows: Option<Uuid>, follows_shown: bool, last_shown: Option<Uuid>) -> ThreadSpot {
    if follows.is_none() || follows_shown {
        ThreadSpot {
            parent_id: last_shown,
            shown: true,
        }
    } else {
        ThreadSpot {
            parent_id: follows,
            shown: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadEntry {
    Sent,
    Reply,
}

// Every message added outside an edit goes through here, under the conversation lock that edits
// and version switches take too, so the thread on screen stays one line: two turns sent at once
// follow each other instead of looking like versions, and a reply that lands after a switch can't
// appear under a question that is no longer shown. `follows` is what the message continues: the
// message shown last when the turn was sent, or the turn's own question for its reply. A sent
// message counts toward the chat list's message count and last-message time only when it is
// shown, which a switch's recount agrees with. The placement fields are cleared afterwards, so
// later saves of the same model leave them alone. Returns the creation time it stamped.
pub async fn add_to_thread(
    db: &DatabaseConnection,
    message: &mut messages::ActiveModel,
    conversation_id: Uuid,
    follows: Option<Uuid>,
    entry: ThreadEntry,
) -> Result<DateTime<Utc>, AppError> {
    let txn = db.begin().await.map_err(|e| {
        eprintln!("thread insert transaction error: {e}");
        AppError::DbTimeout
    })?;
    conversations::Entity::find_by_id(conversation_id)
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(|e| {
            eprintln!("thread insert lock error: {e}");
            AppError::DbTimeout
        })?;
    let follows_shown = match follows {
        Some(id) => is_shown(&txn, id).await?,
        None => true,
    };
    let last_shown = last_shown_message(&txn, conversation_id).await?;
    let spot = thread_spot(follows, follows_shown, last_shown);
    // Stamped under the lock, so the order of `createdAt` is the order of the thread.
    let created_at = Utc::now();
    message.previous_message_id = Set(spot.parent_id);
    message.deleted = Set(!spot.shown);
    message.inactive_branch = Set(!spot.shown);
    message.created_at = Set(created_at);
    message.updated_at = Set(created_at);
    message.clone().insert(&txn).await.map_err(|e| {
        eprintln!("thread insert error: {e}");
        AppError::DbTimeout
    })?;
    if spot.shown && entry == ThreadEntry::Sent {
        count_sent_message(conversation_id, created_at)
            .exec(&txn)
            .await
            .map_err(|e| {
                eprintln!("thread insert count error: {e}");
                AppError::DbTimeout
            })?;
    }
    txn.commit().await.map_err(|e| {
        eprintln!("thread insert commit error: {e}");
        AppError::DbTimeout
    })?;
    message.previous_message_id = NotSet;
    message.deleted = NotSet;
    message.inactive_branch = NotSet;
    message.created_at = NotSet;
    Ok(created_at)
}

fn count_sent_message(
    conversation_id: Uuid,
    at: DateTime<Utc>,
) -> UpdateMany<conversations::Entity> {
    conversations::Entity::update_many()
        .filter(conversations::Column::Id.eq(conversation_id))
        .col_expr(
            conversations::Column::MessageCount,
            sea_query::Expr::col(conversations::Column::MessageCount).add(1),
        )
        .col_expr(
            conversations::Column::LastMessageAt,
            sea_query::Expr::value(Some(at)),
        )
}

pub async fn is_shown<C: ConnectionTrait>(conn: &C, message_id: Uuid) -> Result<bool, AppError> {
    messages::Entity::find_by_id(message_id)
        .filter(messages::Column::Deleted.eq(false))
        .count(conn)
        .await
        .map(|count| count > 0)
        .map_err(|e| {
            eprintln!("thread message lookup error: {e}");
            AppError::DbTimeout
        })
}

// Shows the version holding `message_id`, in one transaction with the conversation locked. When
// that changes the thread it also takes the conversation's edit claim, so an edit still replying
// can't commit over the switch, and returns the claim's number for interrupting such replies.
pub async fn show_branch(
    db: &DatabaseConnection,
    user_id: Uuid,
    chat_id: Uuid,
    message_id: Uuid,
) -> Result<Option<i32>, AppError> {
    let txn = db.begin().await.map_err(|e| {
        eprintln!("branch switch transaction error: {e}");
        AppError::DbTimeout
    })?;
    let conversation = conversations::Entity::find_by_id(chat_id)
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(|e| {
            eprintln!("branch switch conversation lookup error: {e}");
            AppError::DbTimeout
        })?
        .filter(|conversation| {
            can_access_conversation(conversation.user_id, user_id)
                && conversation.archived_at.is_none()
        })
        .ok_or(AppError::ResourceNotFound)?;
    let change = load_tree(&txn, conversation.id).await?.select(message_id)?;
    let Some(since) = change.since else {
        return Ok(None);
    };
    let claim = claim_for_branch_switch(&txn, chat_id).await?;
    for update in [
        hide_as_other_version(
            messages::Entity::update_many().filter(messages::Column::Id.is_in(change.hide)),
        ),
        show_messages(
            messages::Entity::update_many().filter(messages::Column::Id.is_in(change.show)),
        ),
    ] {
        update.exec(&txn).await.map_err(|e| {
            eprintln!("branch switch update error: {e}");
            AppError::DbTimeout
        })?;
    }
    stale_summary(chat_id, since)
        .exec(&txn)
        .await
        .map_err(|e| {
            eprintln!("branch switch summary reset error: {e}");
            AppError::DbTimeout
        })?;
    recount_conversation(&txn, chat_id).await?;
    txn.commit().await.map_err(|e| {
        eprintln!("branch switch commit error: {e}");
        AppError::DbTimeout
    })?;
    Ok(Some(claim))
}

async fn claim_for_branch_switch(
    txn: &DatabaseTransaction,
    chat_id: Uuid,
) -> Result<i32, AppError> {
    branch_switch_claim(chat_id)
        .exec_with_returning(txn)
        .await
        .map_err(|e| {
            eprintln!("branch switch claim error: {e}");
            AppError::DbTimeout
        })?
        .first()
        .map(|conversation| conversation.edit_requests)
        .ok_or(AppError::ResourceNotFound)
}

// Numbered and claimed like a new edit, so every edit requested before the switch is superseded.
fn branch_switch_claim(chat_id: Uuid) -> UpdateMany<conversations::Entity> {
    let next = sea_query::Expr::col(conversations::Column::EditRequests).add(1);
    conversations::Entity::update_many()
        .filter(conversations::Column::Id.eq(chat_id))
        .col_expr(conversations::Column::EditRequests, next.clone())
        .col_expr(conversations::Column::EditClaim, next)
}

pub fn show_messages(update: UpdateMany<messages::Entity>) -> UpdateMany<messages::Entity> {
    set_placement(update, false, false)
}

pub fn hide_as_other_version(update: UpdateMany<messages::Entity>) -> UpdateMany<messages::Entity> {
    set_placement(update, true, true)
}

fn set_placement(
    update: UpdateMany<messages::Entity>,
    deleted: bool,
    inactive_branch: bool,
) -> UpdateMany<messages::Entity> {
    update
        .col_expr(messages::Column::Deleted, sea_query::Expr::value(deleted))
        .col_expr(
            messages::Column::InactiveBranch,
            sea_query::Expr::value(inactive_branch),
        )
        .col_expr(
            messages::Column::UpdatedAt,
            sea_query::Expr::value(Utc::now()),
        )
}

// The summary is cumulative and only ever extended, so one that already covers a message that
// left the screen would keep its content; dropping it lets the next update rebuild it from what
// is visible.
pub fn stale_summary(
    conversation_id: Uuid,
    since: DateTime<Utc>,
) -> DeleteMany<conversation_summaries::Entity> {
    conversation_summaries::Entity::delete_many()
        .filter(conversation_summaries::Column::ConversationId.eq(conversation_id))
        .filter(
            Condition::any()
                .add(conversation_summaries::Column::LastMessageAt.gte(since))
                .add(conversation_summaries::Column::LastMessageAt.is_null()),
        )
}

// message_count grows by the request's messages each turn, which in practice means user
// messages, so it is recounted the same way from what is visible.
pub async fn recount_conversation(
    txn: &DatabaseTransaction,
    conversation_id: Uuid,
) -> Result<(), AppError> {
    let visible = messages::Entity::find()
        .filter(messages::Column::ConversationId.eq(conversation_id))
        .filter(messages::Column::Deleted.eq(false));
    let user_messages = visible
        .clone()
        .filter(messages::Column::Role.eq(ChatRole::User))
        .count(txn)
        .await
        .map_err(|e| {
            eprintln!("conversation message count error: {e}");
            AppError::DbTimeout
        })?;
    let last_message_at: Option<DateTime<Utc>> = visible
        .select_only()
        .expr(sea_query::Expr::col(messages::Column::CreatedAt).max())
        .into_tuple()
        .one(txn)
        .await
        .map_err(|e| {
            eprintln!("conversation last message lookup error: {e}");
            AppError::DbTimeout
        })?
        .flatten();
    conversations::Entity::update_many()
        .filter(conversations::Column::Id.eq(conversation_id))
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
            eprintln!("conversation recount error: {e}");
            AppError::DbTimeout
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use sea_orm::{DbBackend, QueryTrait};

    fn at(minute: i64) -> DateTime<Utc> {
        DateTime::<Utc>::UNIX_EPOCH + Duration::minutes(minute)
    }

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn node(
        n: u128,
        parent: Option<u128>,
        role: ChatRole,
        placement: Placement,
        created: i64,
        seen: i64,
    ) -> BranchNode {
        BranchNode {
            id: id(n),
            parent_id: parent.map(id),
            role,
            placement,
            created_at: at(created),
            updated_at: at(seen),
        }
    }

    use ChatRole::{Assistant, User};
    use Placement::{Gone, OtherVersion, Shown};

    // u1 → a1 → u2 → a2 was edited at minute 10 into u2b → a2b, then the chat went on with u3 → a3.
    fn edited_chat() -> Vec<BranchNode> {
        vec![
            node(1, None, User, Shown, 1, 1),
            node(2, Some(1), Assistant, Shown, 2, 2),
            node(3, Some(2), User, OtherVersion, 3, 10),
            node(4, Some(3), Assistant, OtherVersion, 4, 10),
            node(5, Some(2), User, Shown, 9, 10),
            node(6, Some(5), Assistant, Shown, 9, 10),
            node(7, Some(6), User, Shown, 11, 11),
            node(8, Some(7), Assistant, Shown, 12, 12),
        ]
    }

    fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
        ids.sort();
        ids
    }

    #[test]
    fn an_edited_message_lists_both_versions_oldest_first() {
        let versions = MessageTree::new(edited_chat()).versions();
        assert_eq!(
            versions.get(&id(5)),
            Some(&MessageVersions {
                current: 2,
                total: 2,
                message_ids: vec![id(3), id(5)],
            })
        );
        assert_eq!(versions.len(), 1, "only the edited message has versions");
    }

    #[test]
    fn switching_to_the_earlier_version_shows_its_own_thread() {
        let change = MessageTree::new(edited_chat()).select(id(3)).unwrap();
        assert_eq!(sorted(change.show), vec![id(3), id(4)]);
        assert_eq!(sorted(change.hide), vec![id(5), id(6), id(7), id(8)]);
        assert_eq!(change.since, Some(at(3)));
    }

    #[test]
    fn switching_back_restores_the_whole_later_thread() {
        let mut nodes = edited_chat();
        for node in &mut nodes {
            match node.id.as_u128() {
                3 | 4 => {
                    node.placement = Shown;
                    node.updated_at = at(20);
                }
                5..=8 => {
                    node.placement = OtherVersion;
                    node.updated_at = at(20);
                }
                _ => {}
            }
        }
        let change = MessageTree::new(nodes).select(id(5)).unwrap();
        assert_eq!(sorted(change.show), vec![id(5), id(6), id(7), id(8)]);
        assert_eq!(sorted(change.hide), vec![id(3), id(4)]);
    }

    #[test]
    fn selecting_a_message_already_on_screen_changes_nothing() {
        let tree = MessageTree::new(edited_chat());
        for shown in [1, 2, 5, 8] {
            assert_eq!(tree.select(id(shown)).unwrap(), BranchChange::default());
        }
    }

    #[test]
    fn deleted_messages_and_unfinished_attempts_are_never_versions() {
        let mut nodes = edited_chat();
        nodes.push(node(9, Some(2), User, Gone, 13, 13));
        let tree = MessageTree::new(nodes);
        assert_eq!(tree.versions().get(&id(5)).map(|v| v.total), Some(2));
        assert!(matches!(
            tree.select(id(9)),
            Err(AppError::ResourceNotFound)
        ));
        assert!(matches!(
            tree.select(id(99)),
            Err(AppError::ResourceNotFound)
        ));
    }

    #[test]
    fn a_version_returns_to_the_sub_version_seen_most_recently() {
        let mut nodes = edited_chat();
        for node in &mut nodes {
            match node.id.as_u128() {
                3 | 4 => node.placement = Shown,
                5 | 6 => node.placement = OtherVersion,
                7 | 8 => {
                    node.placement = OtherVersion;
                    node.updated_at = at(14);
                }
                _ => {}
            }
        }
        nodes.push(node(10, Some(6), User, OtherVersion, 13, 15));
        nodes.push(node(11, Some(10), Assistant, OtherVersion, 13, 15));
        let change = MessageTree::new(nodes).select(id(5)).unwrap();
        assert_eq!(sorted(change.show), vec![id(5), id(6), id(10), id(11)]);
    }

    #[test]
    fn a_deleted_message_inside_a_version_stays_deleted_but_the_thread_goes_on() {
        let mut nodes = edited_chat();
        nodes.push(node(12, Some(4), User, OtherVersion, 5, 10));
        nodes.push(node(13, Some(12), Assistant, OtherVersion, 6, 10));
        for node in &mut nodes {
            if node.id == id(4) {
                node.placement = Gone;
            }
        }
        let change = MessageTree::new(nodes).select(id(3)).unwrap();
        assert_eq!(sorted(change.show), vec![id(3), id(12), id(13)]);
    }

    #[test]
    fn a_switch_takes_the_next_edit_number_as_its_claim() {
        let chat = id(3);
        let sql = branch_switch_claim(chat)
            .build(DbBackend::Postgres)
            .to_string();
        assert!(
            sql.contains(r#""editRequests" = "editRequests" + 1"#),
            "{sql}"
        );
        assert!(sql.contains(r#""editClaim" = "editRequests" + 1"#), "{sql}");
        assert!(
            sql.ends_with(&format!(r#""conversations"."id" = '{chat}'"#)),
            "{sql}"
        );
    }

    #[test]
    fn a_sent_message_adds_one_to_this_conversations_count_without_reading_it_first() {
        let chat = id(3);
        let sql = count_sent_message(chat, at(5))
            .build(DbBackend::Postgres)
            .to_string();
        assert!(
            sql.contains(r#""messageCount" = "messageCount" + 1"#),
            "{sql}"
        );
        assert!(sql.contains(r#""lastMessageAt" = '"#), "{sql}");
        assert!(
            sql.ends_with(&format!(r#""conversations"."id" = '{chat}'"#)),
            "{sql}"
        );
    }

    #[test]
    fn hidden_versions_stay_showable_and_shown_ones_are_plain_visible() {
        let hide = hide_as_other_version(messages::Entity::update_many())
            .build(DbBackend::Postgres)
            .to_string();
        assert!(
            hide.contains(r#""deleted" = TRUE, "inactiveBranch" = TRUE"#),
            "{hide}"
        );
        let show = show_messages(messages::Entity::update_many())
            .build(DbBackend::Postgres)
            .to_string();
        assert!(
            show.contains(r#""deleted" = FALSE, "inactiveBranch" = FALSE"#),
            "{show}"
        );
    }

    #[test]
    fn only_this_conversations_summary_that_covers_the_change_is_dropped() {
        let chat = id(3);
        let sql = stale_summary(chat, at(5))
            .build(DbBackend::Postgres)
            .to_string();
        assert!(
            sql.starts_with(r#"DELETE FROM "conversation_summaries""#),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(r#""conversationId" = '{chat}'"#)),
            "{sql}"
        );
        assert!(sql.contains(r#""lastMessageAt" >= "#), "{sql}");
        assert!(sql.contains(r#""lastMessageAt" IS NULL"#), "{sql}");
    }

    #[test]
    fn a_new_message_joins_the_end_of_the_thread_on_screen() {
        assert_eq!(
            thread_spot(Some(id(1)), true, Some(id(1))),
            ThreadSpot {
                parent_id: Some(id(1)),
                shown: true
            }
        );
    }

    #[test]
    fn a_second_turn_sent_at_once_follows_the_first_instead_of_branching() {
        assert_eq!(
            thread_spot(Some(id(1)), true, Some(id(2))),
            ThreadSpot {
                parent_id: Some(id(2)),
                shown: true
            }
        );
    }

    #[test]
    fn a_reply_whose_question_was_switched_away_stays_with_it() {
        assert_eq!(
            thread_spot(Some(id(1)), false, Some(id(9))),
            ThreadSpot {
                parent_id: Some(id(1)),
                shown: false
            }
        );
    }

    #[test]
    fn the_first_message_of_a_chat_has_no_parent() {
        assert_eq!(
            thread_spot(None, true, None),
            ThreadSpot {
                parent_id: None,
                shown: true
            }
        );
    }

    #[test]
    fn a_long_thread_does_not_overflow_the_walk() {
        let nodes: Vec<BranchNode> = (1..=5000u128)
            .map(|n| {
                let role = if n % 2 == 1 { User } else { Assistant };
                node(n, (n > 1).then_some(n - 1), role, Shown, n as i64, n as i64)
            })
            .collect();
        let tree = MessageTree::new(nodes);
        assert_eq!(tree.select(id(1)).unwrap(), BranchChange::default());
    }
}
