use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop UNIQUE(email)
        manager
            .get_connection()
            .execute_unprepared(r#"
                ALTER TABLE "users"
                DROP CONSTRAINT IF EXISTS "users_email_key";
            "#)
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Re-add UNIQUE(email) on rollback
        manager
            .get_connection()
            .execute_unprepared(r#"
                ALTER TABLE "users"
                ADD CONSTRAINT "users_email_key" UNIQUE ("email");
            "#)
            .await?;

        Ok(())
    }
}
