use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_table("mcp_oauth_states").await? {
            return Ok(());
        }

        manager
            .create_table(
                Table::create()
                    .table(Alias::new("mcp_oauth_states"))
                    .if_not_exists()
                    .col(ColumnDef::new(Alias::new("id")).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Alias::new("serverId")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("userId")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("state")).string().not_null())
                    .col(ColumnDef::new(Alias::new("pkceVerifier")).text().not_null())
                    .col(ColumnDef::new(Alias::new("redirectUri")).text().null())
                    .col(
                        ColumnDef::new(Alias::new("expiresAt"))
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("createdAt"))
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-mcp-oauth-state")
                    .table(Alias::new("mcp_oauth_states"))
                    .col(Alias::new("state"))
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-mcp-oauth-states-user")
                    .table(Alias::new("mcp_oauth_states"))
                    .col(Alias::new("userId"))
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-mcp-oauth-states-server")
                    .table(Alias::new("mcp_oauth_states"))
                    .col(Alias::new("serverId"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("mcp_oauth_states")).to_owned())
            .await
    }
}
