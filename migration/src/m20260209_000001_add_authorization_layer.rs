// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{DatabaseBackend, Statement};
use uuid::Uuid;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .add_column(
                        ColumnDef::new(Users::IsIndependent)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .add_column(
                        ColumnDef::new(Users::EffectivePermissions)
                            .json_binary()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Permissions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Permissions::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Permissions::Domain).string().not_null())
                    .col(ColumnDef::new(Permissions::Action).string().not_null())
                    .col(
                        ColumnDef::new(Permissions::IsScopeable)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Permissions::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Permissions::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-permissions-domain-action")
                    .table(Permissions::Table)
                    .col(Permissions::Domain)
                    .col(Permissions::Action)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Roles::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Roles::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Roles::Name).string().not_null())
                    .col(ColumnDef::new(Roles::IsSystem).boolean().not_null())
                    .col(
                        ColumnDef::new(Roles::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Roles::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-roles-name")
                    .table(Roles::Table)
                    .col(Roles::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(RolePermissions::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(RolePermissions::RoleId).uuid().not_null())
                    .col(
                        ColumnDef::new(RolePermissions::PermissionId)
                            .uuid()
                            .not_null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(RolePermissions::RoleId)
                            .col(RolePermissions::PermissionId),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-role-permissions-role")
                    .from(RolePermissions::Table, RolePermissions::RoleId)
                    .to(Roles::Table, Roles::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;
        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-role-permissions-permission")
                    .from(RolePermissions::Table, RolePermissions::PermissionId)
                    .to(Permissions::Table, Permissions::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(UserRoleAssignments::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(UserRoleAssignments::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(UserRoleAssignments::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserRoleAssignments::RoleId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserRoleAssignments::ScopeDepartmentId)
                            .uuid()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(UserRoleAssignments::AssignedBy)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserRoleAssignments::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(UserRoleAssignments::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-user-role-assignments")
                    .table(UserRoleAssignments::Table)
                    .col(UserRoleAssignments::UserId)
                    .col(UserRoleAssignments::RoleId)
                    .col(UserRoleAssignments::ScopeDepartmentId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE UNIQUE INDEX IF NOT EXISTS uq_user_role_assignments_orgwide ON user_role_assignments ("userId","roleId") WHERE "scopeDepartmentId" IS NULL;"#,
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-user-role-assignments-user")
                    .table(UserRoleAssignments::Table)
                    .col(UserRoleAssignments::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-user-role-assignments-role")
                    .table(UserRoleAssignments::Table)
                    .col(UserRoleAssignments::RoleId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-user-role-assignments-user")
                    .from(UserRoleAssignments::Table, UserRoleAssignments::UserId)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-user-role-assignments-role")
                    .from(UserRoleAssignments::Table, UserRoleAssignments::RoleId)
                    .to(Roles::Table, Roles::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-user-role-assignments-scope-dept")
                    .from(
                        UserRoleAssignments::Table,
                        UserRoleAssignments::ScopeDepartmentId,
                    )
                    .to(Departments::Table, Departments::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-user-role-assignments-assigned-by")
                    .from(UserRoleAssignments::Table, UserRoleAssignments::AssignedBy)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(McpServers::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(McpServers::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(McpServers::Name).string().not_null())
                    .col(
                        ColumnDef::new(McpServers::AccessDefault)
                            .string()
                            .not_null()
                            .default("allow"),
                    )
                    .col(
                        ColumnDef::new(McpServers::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(McpServers::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-mcp-servers-name")
                    .table(McpServers::Table)
                    .col(McpServers::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(McpServerAccessRules::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(McpServerAccessRules::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(McpServerAccessRules::ServerId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(McpServerAccessRules::SubjectType)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(McpServerAccessRules::SubjectId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(McpServerAccessRules::RuleType)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(McpServerAccessRules::CreatedBy)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(McpServerAccessRules::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq-mcp-access-rules")
                    .table(McpServerAccessRules::Table)
                    .col(McpServerAccessRules::ServerId)
                    .col(McpServerAccessRules::SubjectType)
                    .col(McpServerAccessRules::SubjectId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-mcp-access-rules-server")
                    .from(McpServerAccessRules::Table, McpServerAccessRules::ServerId)
                    .to(McpServers::Table, McpServers::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-mcp-access-rules-created-by")
                    .from(McpServerAccessRules::Table, McpServerAccessRules::CreatedBy)
                    .to(Users::Table, Users::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .on_update(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(AuthAuditEvents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AuthAuditEvents::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AuthAuditEvents::Event).string().not_null())
                    .col(ColumnDef::new(AuthAuditEvents::ActorId).uuid().null())
                    .col(
                        ColumnDef::new(AuthAuditEvents::Payload)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AuthAuditEvents::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        self.seed_permissions_and_roles(manager).await?;
        self.migrate_legacy_users(manager).await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AuthAuditEvents::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(McpServerAccessRules::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(McpServers::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(UserRoleAssignments::Table).to_owned())
            .await?;
        manager
            .get_connection()
            .execute_unprepared(r#"DROP INDEX IF EXISTS uq_user_role_assignments_orgwide;"#)
            .await?;
        manager
            .drop_table(Table::drop().table(RolePermissions::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Roles::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Permissions::Table).to_owned())
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Users::Table)
                    .drop_column(Users::EffectivePermissions)
                    .drop_column(Users::IsIndependent)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

impl Migration {
    async fn seed_permissions_and_roles(&self, manager: &SchemaManager<'_>) -> Result<(), DbErr> {
        let permissions = vec![
            ("ai_platform", "manage", false),
            ("ai_platform", "view", false),
            ("users", "view", true),
            ("users", "manage", true),
            ("analytics", "view", true),
            ("audit_logs", "view", false),
            ("budget", "view", true),
            ("budget", "allocate", true),
            ("mcp_servers", "view", false),
            ("mcp_servers", "admin", false),
            ("mcp_servers", "delegate", true),
            ("departments", "view", true),
            ("departments", "manage", true),
            ("sso_providers", "view", false),
            ("sso_providers", "manage", false),
            ("roles", "view", false),
            ("roles", "manage", false),
            ("roles", "assign", true),
        ];

        let mut permission_ids = std::collections::HashMap::new();
        let mut insert_permissions = Query::insert();
        insert_permissions.into_table(Permissions::Table).columns([
            Permissions::Id,
            Permissions::Domain,
            Permissions::Action,
            Permissions::IsScopeable,
            Permissions::CreatedAt,
            Permissions::UpdatedAt,
        ]);
        for (domain, action, is_scopeable) in permissions {
            let id = Uuid::new_v4();
            permission_ids.insert(format!("{domain}:{action}"), id);
            insert_permissions.values_panic([
                id.into(),
                domain.into(),
                action.into(),
                is_scopeable.into(),
                Expr::current_timestamp().into(),
                Expr::current_timestamp().into(),
            ]);
        }
        manager.exec_stmt(insert_permissions).await?;

        let roles = vec![
            ("Super Admin", true),
            ("HR Admin", true),
            ("Finance Admin", true),
            ("IT Admin", true),
            ("Department Admin", true),
            ("User", true),
            ("Observer", true),
        ];

        let mut role_ids = std::collections::HashMap::new();
        let mut insert_roles = Query::insert();
        insert_roles.into_table(Roles::Table).columns([
            Roles::Id,
            Roles::Name,
            Roles::IsSystem,
            Roles::CreatedAt,
            Roles::UpdatedAt,
        ]);
        for (name, is_system) in roles {
            let id = Uuid::new_v4();
            role_ids.insert(name.to_string(), id);
            insert_roles.values_panic([
                id.into(),
                name.into(),
                is_system.into(),
                Expr::current_timestamp().into(),
                Expr::current_timestamp().into(),
            ]);
        }
        manager.exec_stmt(insert_roles).await?;

        let mut role_permissions: Vec<(Uuid, Uuid)> = Vec::new();

        if let Some(role_id) = role_ids.get("Super Admin") {
            for permission_id in permission_ids.values() {
                role_permissions.push((*role_id, *permission_id));
            }
        }

        if let Some(role_id) = role_ids.get("HR Admin") {
            for key in [
                "users:view",
                "users:manage",
                "departments:view",
                "roles:view",
                "roles:assign",
            ] {
                if let Some(permission_id) = permission_ids.get(key) {
                    role_permissions.push((*role_id, *permission_id));
                }
            }
        }

        if let Some(role_id) = role_ids.get("Finance Admin") {
            for key in [
                "budget:view",
                "budget:allocate",
                "analytics:view",
                "departments:view",
            ] {
                if let Some(permission_id) = permission_ids.get(key) {
                    role_permissions.push((*role_id, *permission_id));
                }
            }
        }

        if let Some(role_id) = role_ids.get("IT Admin") {
            for key in [
                "ai_platform:manage",
                "mcp_servers:view",
                "mcp_servers:admin",
                "mcp_servers:delegate",
                "sso_providers:view",
                "sso_providers:manage",
            ] {
                if let Some(permission_id) = permission_ids.get(key) {
                    role_permissions.push((*role_id, *permission_id));
                }
            }
        }

        if let Some(role_id) = role_ids.get("Department Admin") {
            for key in [
                "departments:view",
                "departments:manage",
                "users:view",
                "users:manage",
                "analytics:view",
                "budget:view",
                "budget:allocate",
                "mcp_servers:delegate",
            ] {
                if let Some(permission_id) = permission_ids.get(key) {
                    role_permissions.push((*role_id, *permission_id));
                }
            }
        }

        if let Some(role_id) = role_ids.get("Observer") {
            for key in ["analytics:view", "departments:view", "users:view"] {
                if let Some(permission_id) = permission_ids.get(key) {
                    role_permissions.push((*role_id, *permission_id));
                }
            }
        }

        if !role_permissions.is_empty() {
            let mut insert_role_permissions = Query::insert();
            insert_role_permissions
                .into_table(RolePermissions::Table)
                .columns([RolePermissions::RoleId, RolePermissions::PermissionId]);
            for (role_id, permission_id) in role_permissions {
                insert_role_permissions.values_panic([role_id.into(), permission_id.into()]);
            }
            manager.exec_stmt(insert_role_permissions).await?;
        }

        Ok(())
    }

    async fn migrate_legacy_users(&self, manager: &SchemaManager<'_>) -> Result<(), DbErr> {
        if !self.legacy_role_column_exists(manager).await? {
            return Ok(());
        }

        let mut role_ids = std::collections::HashMap::new();
        let role_rows = manager
            .get_connection()
            .query_all(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT \"id\", \"name\" FROM roles",
            ))
            .await?;

        for row in role_rows {
            let id: Uuid = row.try_get("", "id")?;
            let name: String = row.try_get("", "name")?;
            role_ids.insert(name, id);
        }

        let super_admin_role_id = match role_ids.get("Super Admin") {
            Some(id) => *id,
            None => return Ok(()),
        };
        let observer_role_id = match role_ids.get("Observer") {
            Some(id) => *id,
            None => return Ok(()),
        };

        let admin_users = manager
            .get_connection()
            .query_all(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT \"id\" FROM users WHERE \"role\" IN ('admin','superadmin')",
            ))
            .await?;
        let mut admin_values = Vec::new();
        for row in admin_users {
            let user_id: Uuid = row.try_get("", "id")?;
            let assignment_id = Uuid::new_v4();
            admin_values.push(format!(
                "('{}','{}','{}',NULL,'{}', NOW(), NOW())",
                assignment_id, user_id, super_admin_role_id, user_id
            ));
        }

        if !admin_values.is_empty() {
            let insert_admins = format!(
                "INSERT INTO user_role_assignments (\"id\", \"userId\", \"roleId\", \"scopeDepartmentId\", \"assignedBy\", \"createdAt\", \"updatedAt\") VALUES {} ON CONFLICT (\"userId\", \"roleId\", \"scopeDepartmentId\") DO NOTHING",
                admin_values.join(",")
            );
            manager
                .get_connection()
                .execute_unprepared(&insert_admins)
                .await?;
        }

        let observer_users = manager
            .get_connection()
            .query_all(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT \"id\" FROM users WHERE \"role\" = 'observer'",
            ))
            .await?;
        let mut observer_values = Vec::new();
        for row in observer_users {
            let user_id: Uuid = row.try_get("", "id")?;
            let assignment_id = Uuid::new_v4();
            observer_values.push(format!(
                "('{}','{}','{}',NULL,'{}', NOW(), NOW())",
                assignment_id, user_id, observer_role_id, user_id
            ));
        }

        if !observer_values.is_empty() {
            let insert_observers = format!(
                "INSERT INTO user_role_assignments (\"id\", \"userId\", \"roleId\", \"scopeDepartmentId\", \"assignedBy\", \"createdAt\", \"updatedAt\") VALUES {} ON CONFLICT (\"userId\", \"roleId\", \"scopeDepartmentId\") DO NOTHING",
                observer_values.join(",")
            );
            manager
                .get_connection()
                .execute_unprepared(&insert_observers)
                .await?;
        }

        Ok(())
    }

    async fn legacy_role_column_exists(&self, manager: &SchemaManager<'_>) -> Result<bool, DbErr> {
        let rows = manager
            .get_connection()
            .query_all(Statement::from_string(
                DatabaseBackend::Postgres,
                "SELECT 1 FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'users' AND column_name = 'role'",
            ))
            .await?;
        Ok(!rows.is_empty())
    }
}

#[derive(DeriveIden)]
enum Users {
    #[sea_orm(iden = "users")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "isIndependent")]
    IsIndependent,
    #[sea_orm(iden = "effectivePermissions")]
    EffectivePermissions,
}

#[derive(DeriveIden)]
enum Departments {
    #[sea_orm(iden = "departments")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
}

#[derive(DeriveIden)]
enum Permissions {
    #[sea_orm(iden = "permissions")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "domain")]
    Domain,
    #[sea_orm(iden = "action")]
    Action,
    #[sea_orm(iden = "isScopeable")]
    IsScopeable,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Roles {
    #[sea_orm(iden = "roles")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "isSystem")]
    IsSystem,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum RolePermissions {
    #[sea_orm(iden = "role_permissions")]
    Table,
    #[sea_orm(iden = "roleId")]
    RoleId,
    #[sea_orm(iden = "permissionId")]
    PermissionId,
}

#[derive(DeriveIden)]
enum UserRoleAssignments {
    #[sea_orm(iden = "user_role_assignments")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "userId")]
    UserId,
    #[sea_orm(iden = "roleId")]
    RoleId,
    #[sea_orm(iden = "scopeDepartmentId")]
    ScopeDepartmentId,
    #[sea_orm(iden = "assignedBy")]
    AssignedBy,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum McpServers {
    #[sea_orm(iden = "mcp_servers")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "name")]
    Name,
    #[sea_orm(iden = "accessDefault")]
    AccessDefault,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
    #[sea_orm(iden = "updatedAt")]
    UpdatedAt,
}

#[derive(DeriveIden)]
enum McpServerAccessRules {
    #[sea_orm(iden = "mcp_server_access_rules")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "serverId")]
    ServerId,
    #[sea_orm(iden = "subjectType")]
    SubjectType,
    #[sea_orm(iden = "subjectId")]
    SubjectId,
    #[sea_orm(iden = "ruleType")]
    RuleType,
    #[sea_orm(iden = "createdBy")]
    CreatedBy,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}

#[derive(DeriveIden)]
enum AuthAuditEvents {
    #[sea_orm(iden = "auth_audit_events")]
    Table,
    #[sea_orm(iden = "id")]
    Id,
    #[sea_orm(iden = "event")]
    Event,
    #[sea_orm(iden = "actorId")]
    ActorId,
    #[sea_orm(iden = "payload")]
    Payload,
    #[sea_orm(iden = "createdAt")]
    CreatedAt,
}
