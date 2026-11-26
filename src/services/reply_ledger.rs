// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use rust_decimal::Decimal;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QuerySelect, TryIntoModel, sea_query::Expr,
};
use serde_json::json;
use uuid::Uuid;

use crate::models::{
    messages,
    usage_logs::{self, UsageLogStatus},
};

// The message-cost trigger charges a reply whenever its cost is written. Once its chat is deleted
// (which also deletes the reply) there is no row left to write, so the part not yet charged goes
// to the ledger directly: deleting a chat while it generates must not lower recorded spend.
pub async fn save_reply(
    db: &DatabaseConnection,
    user_id: Uuid,
    reply: &messages::ActiveModel,
) -> Result<(), DbErr> {
    match reply.clone().update(db).await {
        Ok(_) => Ok(()),
        Err(DbErr::RecordNotUpdated) => {
            charge_unsaved_reply(db, user_id, reply).await;
            Err(DbErr::RecordNotUpdated)
        }
        Err(error) => Err(error),
    }
}

// For a reply whose row could not be written and never will be (its chat is gone).
pub async fn charge_unsaved_reply(
    db: &DatabaseConnection,
    user_id: Uuid,
    reply: &messages::ActiveModel,
) {
    if let Err(error) = charge_remaining(db, user_id, reply).await {
        eprintln!("unsaved reply charge error: {error}");
    }
}

async fn charge_remaining(
    db: &DatabaseConnection,
    user_id: Uuid,
    reply: &messages::ActiveModel,
) -> Result<(), DbErr> {
    let reply = reply.clone().try_into_model()?;
    let identifier = reply.id.to_string();
    let charged: Option<Decimal> = usage_logs::Entity::find()
        .filter(usage_logs::Column::UserId.eq(user_id))
        .filter(usage_logs::Column::Identifier.eq(identifier.as_str()))
        .select_only()
        .column_as(Expr::col(usage_logs::Column::CostUsd).sum(), "charged")
        .into_tuple::<Option<Decimal>>()
        .one(db)
        .await?
        .flatten();
    let remaining = reply.cost - charged.unwrap_or_default();
    if remaining <= Decimal::ZERO {
        return Ok(());
    }
    usage_logs::ActiveModel {
        id: Set(Uuid::new_v4()),
        identifier: Set(identifier),
        user_id: Set(user_id),
        model_provider: Set(reply.model_provider),
        model_name: Set(reply.model_name),
        conversation_id: Set(None),
        request_tokens: Set(i64::from(reply.request_tokens)),
        response_tokens: Set(i64::from(reply.response_tokens)),
        total_tokens: Set(i64::from(reply.total_tokens)),
        cost_usd: Set(remaining),
        latency_ms: Set(i64::from(reply.latency)),
        timestamp: Set(chrono::Utc::now()),
        department: Set(None),
        status: Set(UsageLogStatus::Charged),
        error_message: Set(None),
        metadata: Set(Some(json!({
            "messageId": reply.id,
            "conversationId": reply.conversation_id,
            "conversationDeleted": true,
        }))),
    }
    .insert(db)
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::messages::ChatRole;
    use sea_orm::{ConnectionTrait, Database};

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn deleting_a_generating_chat_keeps_its_reply_charged_once() {
        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").expect("test database URL");
        assert!(
            url.contains("/grengin_security_test"),
            "refusing a non-test database"
        );
        let db = Database::connect(&url).await.expect("test database");
        let (user, conversation, reply_id) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        db.execute_unprepared(&format!(
            r#"INSERT INTO users (id, status, email, "emailVerified", "mfaEnabled", "createdAt", "updatedAt", "lastLoginAt")
                 VALUES ('{user}', 'active', 'ledger-{user}@example.invalid', true, false, now(), now(), now());
               INSERT INTO conversations (id, "userId", "modelProvider", "modelName", "createdAt", "updatedAt", "messageCount", "totalTokens", "totalCost")
                 VALUES ('{conversation}', '{user}', 'acme', 'acme-text', now(), now(), 0, 0, 0);"#
        ))
        .await
        .expect("fixture rows");

        let now = chrono::Utc::now();
        let mut reply = messages::ActiveModel {
            id: Set(reply_id),
            conversation_id: Set(conversation),
            previous_message_id: Set(None),
            deleted: Set(false),
            inactive_branch: Set(false),
            role: Set(ChatRole::Assistant),
            message_content: Set(String::new()),
            model_provider: Set("acme".to_string()),
            model_name: Set("acme-text".to_string()),
            request_tokens: Set(10),
            response_tokens: Set(5),
            request_id: Set(None),
            tools_calls: Set(vec![]),
            tools_results: Set(vec![]),
            created_at: Set(now),
            updated_at: Set(now),
            total_tokens: Set(15),
            latency: Set(0),
            cost: Set(Decimal::ZERO),
            metadata: Set(None),
        };
        reply.clone().insert(&db).await.expect("reply row");
        reply.cost = Set(Decimal::new(50, 2));
        save_reply(&db, user, &reply)
            .await
            .expect("saved while the chat exists");

        db.execute_unprepared(&format!(
            r#"DELETE FROM messages WHERE "conversationId" = '{conversation}';
               DELETE FROM conversations WHERE id = '{conversation}';"#
        ))
        .await
        .expect("chat deleted");
        reply.cost = Set(Decimal::new(120, 2));
        for _ in 0..2 {
            assert!(matches!(
                save_reply(&db, user, &reply).await,
                Err(DbErr::RecordNotUpdated)
            ));
        }

        let rows = usage_logs::Entity::find()
            .filter(usage_logs::Column::UserId.eq(user))
            .all(&db)
            .await
            .expect("ledger rows");
        let mut costs: Vec<Decimal> = rows.iter().map(|row| row.cost_usd).collect();
        costs.sort();
        assert_eq!(costs, [Decimal::new(50, 2), Decimal::new(70, 2)]);
        assert!(
            rows.iter()
                .all(|row| row.identifier == reply_id.to_string())
        );

        db.execute_unprepared(&format!(
            r#"DELETE FROM usage_logs WHERE "userId" = '{user}';
               DELETE FROM users WHERE id = '{user}';"#
        ))
        .await
        .expect("cleanup");
    }
}
