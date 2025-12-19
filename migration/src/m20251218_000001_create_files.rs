use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Files::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Files::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Files::UserId).uuid().not_null())
                    .col(ColumnDef::new(Files::Name).string().not_null())
                    .col(ColumnDef::new(Files::ContentType).string().not_null())
                    .col(ColumnDef::new(Files::Size).big_integer().not_null())
                    .col(ColumnDef::new(Files::LocalPath).string().not_null())
                    .col(ColumnDef::new(Files::Description).text().null())
                    .col(ColumnDef::new(Files::Url).text().null())
                    // Stored as String in your ActiveEnum (lowercase via rename_all)
                    .col(ColumnDef::new(Files::Status).string().not_null())
                    .col(
                        ColumnDef::new(Files::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Files::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(ColumnDef::new(Files::Metadata).json_binary().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_files_user")
                            .from(Files::Table, Files::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_files_user_id")
                    .table(Files::Table)
                    .col(Files::UserId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(Index::drop().name("idx_files_user_id").to_owned())
            .await?;

        manager
            .drop_table(Table::drop().table(Files::Table).to_owned())
            .await?;

        Ok(())
    }
}

#[derive(Iden)]
enum Files {
    #[iden = "files"]
    Table,

    #[iden = "id"]
    Id,

    // match `rename_all="camelCase"` on your EntityModel
    #[iden = "userId"]
    UserId,

    #[iden = "name"]
    Name,

    #[iden = "contentType"]
    ContentType,

    #[iden = "size"]
    Size,

    #[iden = "localPath"]
    LocalPath,

    #[iden = "description"]
    Description,

    #[iden = "url"]
    Url,

    #[iden = "status"]
    Status,

    #[iden = "createdAt"]
    CreatedAt,

    #[iden = "updatedAt"]
    UpdatedAt,

    #[iden = "metadata"]
    Metadata,
}

// minimal idens for FK target
#[derive(Iden)]
enum Users {
    #[iden = "users"]
    Table,
    #[iden = "id"]
    Id,
}
