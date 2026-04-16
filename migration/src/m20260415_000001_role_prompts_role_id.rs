use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum RolePrompts {
    #[sea_orm(iden = "role_prompts")]
    Table,
    #[sea_orm(iden = "role")]
    Role,
    #[sea_orm(iden = "roleId")]
    RoleId,
}

#[derive(DeriveIden)]
enum Roles {
    #[sea_orm(iden = "roles")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("role_prompts").await? {
            return Ok(());
        }

        if !manager.has_column("role_prompts", "roleId").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(RolePrompts::Table)
                        .add_column(ColumnDef::new(RolePrompts::RoleId).uuid().null())
                        .to_owned(),
                )
                .await?;
        }

        if manager.has_column("role_prompts", "role").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    UPDATE "role_prompts" rp
                    SET "roleId" = COALESCE(
                        (SELECT r."id" FROM "roles" r WHERE r."name" = rp."role" LIMIT 1),
                        (SELECT r."id" FROM "roles" r WHERE r."name" = 'User' LIMIT 1),
                        (SELECT r."id" FROM "roles" r ORDER BY r."createdAt" ASC LIMIT 1)
                    )
                    WHERE rp."roleId" IS NULL;
                    "#,
                )
                .await?;
        } else {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    UPDATE "role_prompts" rp
                    SET "roleId" = COALESCE(
                        (SELECT r."id" FROM "roles" r WHERE r."name" = 'User' LIMIT 1),
                        (SELECT r."id" FROM "roles" r ORDER BY r."createdAt" ASC LIMIT 1)
                    )
                    WHERE rp."roleId" IS NULL;
                    "#,
                )
                .await?;
        }

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                DO $$
                BEGIN
                    IF EXISTS (SELECT 1 FROM "role_prompts" WHERE "roleId" IS NULL) THEN
                        RAISE EXCEPTION 'role_prompts.roleId backfill failed';
                    END IF;
                END $$;
                "#,
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(RolePrompts::Table)
                    .modify_column(ColumnDef::new(RolePrompts::RoleId).uuid().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-role-prompts-role-id")
                    .from(RolePrompts::Table, RolePrompts::RoleId)
                    .to(Roles::Table, Roles::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_role_prompts_roleId")
                    .table(RolePrompts::Table)
                    .col(RolePrompts::RoleId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_role_prompts_role")
                    .table(RolePrompts::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;

        if manager.has_column("role_prompts", "role").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(RolePrompts::Table)
                        .drop_column(RolePrompts::Role)
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_table("role_prompts").await? {
            return Ok(());
        }

        if !manager.has_column("role_prompts", "role").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(RolePrompts::Table)
                        .add_column(ColumnDef::new(RolePrompts::Role).string().null())
                        .to_owned(),
                )
                .await?;
        }

        if manager.has_column("role_prompts", "roleId").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"
                    UPDATE "role_prompts" rp
                    SET "role" = COALESCE(
                        (SELECT r."name" FROM "roles" r WHERE r."id" = rp."roleId" LIMIT 1),
                        'User'
                    )
                    WHERE rp."role" IS NULL;
                    "#,
                )
                .await?;
        }

        manager
            .alter_table(
                Table::alter()
                    .table(RolePrompts::Table)
                    .modify_column(ColumnDef::new(RolePrompts::Role).string().not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(r#"ALTER TABLE "role_prompts" DROP CONSTRAINT IF EXISTS "fk-role-prompts-role-id";"#)
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_role_prompts_roleId")
                    .table(RolePrompts::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;

        if manager.has_column("role_prompts", "roleId").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(RolePrompts::Table)
                        .drop_column(RolePrompts::RoleId)
                        .to_owned(),
                )
                .await?;
        }

        manager
            .create_index(
                Index::create()
                    .name("idx_role_prompts_role")
                    .table(RolePrompts::Table)
                    .col(RolePrompts::Role)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
