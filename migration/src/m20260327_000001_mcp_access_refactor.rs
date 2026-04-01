use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_table("mcp_access_policies").await? {
            if !manager
                .has_column("mcp_access_policies", "inheritDepartments")
                .await?
            {
                manager
                    .alter_table(
                        Table::alter()
                            .table(Alias::new("mcp_access_policies"))
                            .add_column(
                                ColumnDef::new(Alias::new("inheritDepartments"))
                                    .boolean()
                                    .not_null()
                                    .default(true),
                            )
                            .to_owned(),
                    )
                    .await?;
            }
        }

        if manager.has_table("mcp_servers").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"UPDATE "mcp_servers"
                       SET "defaultAccess" = 'all_users'
                       WHERE "defaultAccess" IN ('allow_all','allow');"#,
                )
                .await?;
            manager
                .get_connection()
                .execute_unprepared(
                    r#"UPDATE "mcp_servers"
                       SET "defaultAccess" = 'explicit_only'
                       WHERE "defaultAccess" IN ('deny_all','deny');"#,
                )
                .await?;
        }

        if manager.has_table("mcp_server_access_rules").await?
            && manager.has_table("mcp_access_policies").await?
        {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"INSERT INTO "mcp_access_policies"
                        ("id","targetType","serverId","toolId","accessType","permission","roleName","departmentId","userId","inheritDepartments","inheritFromServer","createdAt","createdBy")
                       SELECT
                        r."id",
                        'server',
                        r."serverId",
                        NULL,
                        CASE r."subjectType"
                          WHEN 'user' THEN 'user'
                          WHEN 'department' THEN 'department'
                          ELSE 'user'
                        END,
                        CASE r."ruleType"
                          WHEN 'allow' THEN 'full'
                          ELSE 'denied'
                        END,
                        NULL,
                        CASE WHEN r."subjectType"='department' THEN r."subjectId" ELSE NULL END,
                        CASE WHEN r."subjectType"='user' THEN r."subjectId" ELSE NULL END,
                        true,
                        NULL,
                        r."createdAt",
                        r."createdBy"
                       FROM "mcp_server_access_rules" r
                       WHERE r."subjectType" IN ('user','department')
                       ON CONFLICT ("id") DO NOTHING;"#,
                )
                .await?;

            manager
                .get_connection()
                .execute_unprepared(
                    r#"INSERT INTO "mcp_access_policies"
                        ("id","targetType","serverId","toolId","accessType","permission","roleName","departmentId","userId","inheritDepartments","inheritFromServer","createdAt","createdBy")
                       SELECT
                        r."id",
                        'server',
                        r."serverId",
                        NULL,
                        'role',
                        CASE r."ruleType"
                          WHEN 'allow' THEN 'full'
                          ELSE 'denied'
                        END,
                        roles."name",
                        NULL,
                        NULL,
                        true,
                        NULL,
                        r."createdAt",
                        r."createdBy"
                       FROM "mcp_server_access_rules" r
                       JOIN "roles" roles ON roles."id" = r."subjectId"
                       WHERE r."subjectType" = 'role'
                       ON CONFLICT ("id") DO NOTHING;"#,
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if manager.has_table("mcp_servers").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    r#"UPDATE "mcp_servers"
                       SET "defaultAccess" = 'allow_all'
                       WHERE "defaultAccess" = 'all_users';"#,
                )
                .await?;
            manager
                .get_connection()
                .execute_unprepared(
                    r#"UPDATE "mcp_servers"
                       SET "defaultAccess" = 'deny_all'
                       WHERE "defaultAccess" = 'admin_only';"#,
                )
                .await?;
        }

        if manager
            .has_column("mcp_access_policies", "inheritDepartments")
            .await?
        {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("mcp_access_policies"))
                        .drop_column(Alias::new("inheritDepartments"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}
