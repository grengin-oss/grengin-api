// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(ColumnDef::new(Users::Identities).json_binary().null())
                    .to_owned(),
            )
            .await?;

        backfill_identities(manager).await?;

        // GIN backs the `identities @> '{"<provider>":{"subject":"..."}}'` containment
        // lookup that resolves a login to an existing user.
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_users_identities")
                    .table(Users::Table)
                    .col(Users::Identities)
                    .index_type(IndexType::Custom(Alias::new("gin").into_iden()))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .if_exists()
                    .name("idx_users_identities")
                    .table(Users::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::Identities)
                    .to_owned(),
            )
            .await
    }
}

/// Copies the legacy googleId/azureId columns into the new identity map so no linked
/// account is lost. Both columns are left in place — dto/auth.rs still derives `sub`
/// from them. Bounded one-time pass over only the rows that have a linked account.
async fn backfill_identities(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    let db = manager.get_connection();
    let backend = manager.get_database_backend();

    let select = Query::select()
        .column(Users::Id)
        .column(Users::GoogleId)
        .column(Users::AzureId)
        .column(Users::CreatedAt)
        .from(Users::Table)
        .cond_where(
            Cond::any()
                .add(Expr::col(Users::GoogleId).is_not_null())
                .add(Expr::col(Users::AzureId).is_not_null()),
        )
        .to_owned();

    let rows = db.query_all(backend.build(&select)).await?;

    for row in rows {
        let id: uuid::Uuid = row.try_get("", "id")?;
        let google_id: Option<String> = row.try_get("", "googleId")?;
        let azure_id: Option<String> = row.try_get("", "azureId")?;
        let created_at: chrono::DateTime<chrono::FixedOffset> = row.try_get("", "createdAt")?;
        let linked_at = created_at.to_rfc3339();

        let mut identities = serde_json::Map::new();
        for (provider, subject) in [("google", google_id), ("azure", azure_id)] {
            let Some(subject) = subject else {
                continue;
            };
            identities.insert(
                provider.to_string(),
                serde_json::json!({ "subject": subject, "linkedAt": linked_at }),
            );
        }
        if identities.is_empty() {
            continue;
        }

        let update = Query::update()
            .table(Users::Table)
            .value(Users::Identities, serde_json::Value::Object(identities))
            .and_where(Expr::col(Users::Id).eq(id))
            .to_owned();
        db.execute(backend.build(&update)).await?;
    }

    Ok(())
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "googleId")]
    GoogleId,
    #[sea_orm(iden = "azureId")]
    AzureId,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "identities")]
    Identities,
}
