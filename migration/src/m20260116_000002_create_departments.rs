use sea_orm_migration::prelude::*;
use sea_orm_migration::prelude::extension::postgres::PgLTree;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // ✅ Minimal raw SQL just for enabling extension
        manager
            .get_connection()
            .execute_unprepared(r#"CREATE EXTENSION IF NOT EXISTS ltree;"#)
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Departments::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Departments::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(Departments::Name)
                            .string_len(100)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Departments::Description)
                            .string_len(500)
                            .null(),
                    )
                    .col(ColumnDef::new(Departments::ParentId).uuid().null())
                    .col(
                        ColumnDef::new(Departments::Path)
                            .custom(PgLTree)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Departments::Depth)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(Departments::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Departments::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-departments-parent-id")
                    .from(Departments::Table, Departments::ParentId)
                    .to(Departments::Table, Departments::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-departments-parent-id-name")
                    .table(Departments::Table)
                    .col(Departments::ParentId)
                    .col(Departments::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Optional: GiST index for ltree queries (still needs raw SQL, SeaQuery doesn't model it cleanly)
        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE INDEX IF NOT EXISTS idx_departments_path_gist ON departments USING GIST (path);"#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Departments::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,

    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "description")]
    Description,
    #[sea_orm(iden = "parentId")]
    ParentId,
    #[sea_orm(iden = "path")]
    Path,
    #[sea_orm(iden = "depth")]
    Depth,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}
