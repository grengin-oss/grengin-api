// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

// Department budgets summed messages.cost, so deleting a chat or a message, or editing one (which
// hides the old replies), lowered recorded spend. Spend now goes to usage_logs as append-only rows
// that deletes never touch. A trigger records it because every path that prices a reply (finish,
// cancel, provider error, image generation) writes messages.cost; SeaORM cannot define triggers.
const UP: &str = r#"
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
