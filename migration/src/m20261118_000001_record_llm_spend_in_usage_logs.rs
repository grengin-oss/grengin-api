// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// Department budgets summed messages.cost, so deleting a chat or a message, or editing one (which
// hides the old replies), lowered recorded spend. Spend now goes to usage_logs as append-only rows
// that deletes never touch. A trigger records it because every path that prices a reply (finish,
// cancel, provider error, image generation) writes messages.cost; SeaORM cannot define triggers.
// Writes to messages are blocked from before the backfill snapshot until the trigger exists, so a cost
// committed while this runs is either in the snapshot or recorded by the trigger, never neither.
const UP: &str = r#"
LOCK TABLE messages IN SHARE ROW EXCLUSIVE MODE;

INSERT INTO usage_logs (id, identifier, "userId", "modelProvider", "modelName", "conversationId",
    "requestTokens", "responseTokens", "totalTokens", "costUsd", "latencyMs", "timestamp", status, metadata)
SELECT gen_random_uuid(), m.id::text, c."userId", m."modelProvider", m."modelName", m."conversationId",
    m."requestTokens", m."responseTokens", m."totalTokens", m.cost, m.latency, m."createdAt", 'charged',
    jsonb_build_object('messageId', m.id, 'backfilled', true)
FROM messages m JOIN conversations c ON c.id = m."conversationId"
WHERE m.cost > 0;

CREATE FUNCTION record_message_spend() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
    charged numeric := NEW.cost - CASE WHEN TG_OP = 'UPDATE' THEN OLD.cost ELSE 0 END;
BEGIN
    IF charged > 0 THEN
        INSERT INTO usage_logs (id, identifier, "userId", "modelProvider", "modelName", "conversationId",
            "requestTokens", "responseTokens", "totalTokens", "costUsd", "latencyMs", "timestamp", status, metadata)
        SELECT gen_random_uuid(), NEW.id::text, c."userId", NEW."modelProvider", NEW."modelName", NEW."conversationId",
            NEW."requestTokens", NEW."responseTokens", NEW."totalTokens", charged, NEW.latency, now(), 'charged',
            jsonb_build_object('messageId', NEW.id)
        FROM conversations c WHERE c.id = NEW."conversationId";
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER record_message_spend AFTER INSERT OR UPDATE OF cost ON messages
    FOR EACH ROW EXECUTE FUNCTION record_message_spend();
"#;

const DOWN: &str = r#"
DROP TRIGGER IF EXISTS record_message_spend ON messages;
DROP FUNCTION IF EXISTS record_message_spend();
DELETE FROM usage_logs WHERE status = 'charged';
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(UP)
            .await
            .map(|_| ())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::{ConnectionTrait, Database, Statement, TransactionTrait};

    // Runs on the disposable security-test database (which already has this migration): it undoes the
    // migration, then reapplies it while another transaction holds an uncommitted cost increase.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable database)"]
    async fn a_cost_written_while_the_migration_runs_is_not_lost() {
        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").expect("test database URL");
        assert!(
            url.contains("/grengin_security_test"),
            "refusing a non-test database"
        );
        let db = Database::connect(&url).await.expect("test database");
        db.execute_unprepared(DOWN)
            .await
            .expect("undo the migration");
        let (user, conversation, message) = (
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
        );
        db.execute_unprepared(&format!(
            r#"INSERT INTO users (id, status, email, "emailVerified", "mfaEnabled", "createdAt", "updatedAt", "lastLoginAt")
                 VALUES ('{user}', 'active', 'ledger-{user}@example.invalid', true, false, now(), now(), now());
               INSERT INTO conversations (id, "userId", "modelProvider", "modelName", "createdAt", "updatedAt", "messageCount", "totalTokens", "totalCost")
                 VALUES ('{conversation}', '{user}', 'fixture', 'fixture', now(), now(), 1, 0, 0);
               INSERT INTO messages (id, "conversationId", role, "messageContent", "modelProvider", "modelName", "requestTokens", "responseTokens", "toolsCalls", "toolsResults", "createdAt", "updatedAt", "totalTokens", latency, cost)
                 VALUES ('{message}', '{conversation}', 'assistant', 'reply', 'fixture', 'fixture', 0, 0, '{{}}', '{{}}', now(), now(), 0, 0, 10);"#
        ))
        .await
        .expect("fixture rows");

        let writer_db = Database::connect(&url).await.expect("writer connection");
        let writer = writer_db.begin().await.expect("writer transaction");
        writer
            .execute_unprepared(&format!(
                "UPDATE messages SET cost = 20 WHERE id = '{message}'"
            ))
            .await
            .expect("uncommitted cost increase");
        let migrating = {
            let db = db.clone();
            tokio::spawn(async move { Migration.up(&SchemaManager::new(&db)).await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        writer.commit().await.expect("commit the increase");
        migrating
            .await
            .expect("migration task")
            .expect("migration applies");
        db.execute_unprepared(&format!(
            "UPDATE messages SET cost = 25 WHERE id = '{message}'"
        ))
        .await
        .expect("later increase");

        let total = db
            .query_one(Statement::from_string(
                db.get_database_backend(),
                format!(
                    r#"SELECT COALESCE(SUM("costUsd"), 0)::float8 AS total FROM usage_logs WHERE identifier = '{message}'"#
                ),
            ))
            .await
            .expect("ledger total")
            .expect("one row")
            .try_get::<f64>("", "total")
            .expect("total");
        db.execute_unprepared(&format!("DELETE FROM users WHERE id = '{user}'"))
            .await
            .expect("cleanup");
        assert_eq!(total, 25.0, "the ledger must hold the message's whole cost");
    }
}
