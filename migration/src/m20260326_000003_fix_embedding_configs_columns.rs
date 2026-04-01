use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Align column names with camelCase entity mapping.
        if manager.has_column("embedding_configs", "is_enabled").await?
            && !manager.has_column("embedding_configs", "isEnabled").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("embedding_configs"))
                        .rename_column(Alias::new("is_enabled"), Alias::new("isEnabled"))
                        .to_owned(),
                )
                .await?;
        }

        if manager.has_column("embedding_configs", "created_at").await?
            && !manager.has_column("embedding_configs", "createdAt").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("embedding_configs"))
                        .rename_column(Alias::new("created_at"), Alias::new("createdAt"))
                        .to_owned(),
                )
                .await?;
        }

        if manager.has_column("embedding_configs", "updated_at").await?
            && !manager.has_column("embedding_configs", "updatedAt").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("embedding_configs"))
                        .rename_column(Alias::new("updated_at"), Alias::new("updatedAt"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_column("embedding_configs", "isEnabled").await?
            && !manager.has_column("embedding_configs", "is_enabled").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("embedding_configs"))
                        .rename_column(Alias::new("isEnabled"), Alias::new("is_enabled"))
                        .to_owned(),
                )
                .await?;
        }

        if manager.has_column("embedding_configs", "createdAt").await?
            && !manager.has_column("embedding_configs", "created_at").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("embedding_configs"))
                        .rename_column(Alias::new("createdAt"), Alias::new("created_at"))
                        .to_owned(),
                )
                .await?;
        }

        if manager.has_column("embedding_configs", "updatedAt").await?
            && !manager.has_column("embedding_configs", "updated_at").await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("embedding_configs"))
                        .rename_column(Alias::new("updatedAt"), Alias::new("updated_at"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}
