use std::collections::{HashMap, HashSet};

use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use crate::{
    auth::{error::AuthError, permissions::PERMISSION_MCP_ADMIN},
    dto::mcp::McpResolvedVia,
    error::AppError,
    models::{
        departments, mcp_access_policies,
        mcp_access_policies::{McpAccessTarget, McpAccessType, McpPermission},
        mcp_servers, mcp_tools, roles, user_role_assignments, users,
    },
    services::authorization::{AuthorizationService, PermissionScopeMode, is_path_within_scope},
};

#[derive(Clone, Debug)]
pub struct McpAccessContext {
    pub user_id: Uuid,
    pub department_id: Option<Uuid>,
    pub department_path: Option<String>,
    pub department_depth: Option<i32>,
    pub role_ids: HashSet<Uuid>,
    pub role_names: HashSet<String>,
    pub is_admin: bool,
}

#[derive(Clone, Debug)]
pub struct ResolvedAccess {
    pub permission: McpPermission,
    pub resolved_via: McpResolvedVia,
    pub priority: i32,
}

pub async fn build_access_context(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
) -> Result<McpAccessContext, AppError> {
    let user = users::Entity::find_by_id(user_id)
        .one(db)
        .await
        .map_err(|e| {
            eprintln!("mcp access user lookup error: {e}");
            AppError::DbTimeout
        })?
        .ok_or(AppError::ResourceNotFound)?;

    let (department_path, department_depth) = if let Some(dept_id) = user.department_id {
        #[derive(Debug, sea_orm::FromQueryResult)]
        struct DeptPathRowDb {
            #[sea_orm(from_alias = "path")]
            path: String,
            #[sea_orm(from_alias = "depth")]
            depth: i32,
        }
        let row = departments::Entity::find_by_id(dept_id)
            .select_only()
            .column_as(Expr::cust("path::text"), "path")
            .column_as(departments::Column::Depth, "depth")
            .into_model::<DeptPathRowDb>()
            .one(db)
            .await
            .map_err(|e| {
                eprintln!("department lookup error: {e}");
                AppError::DbTimeout
            })?;
        match row {
            Some(row) => (Some(row.path), Some(row.depth)),
            None => (None, None),
        }
    } else {
        (None, None)
    };

    let role_ids = user_role_assignments::Entity::find()
        .select_only()
        .column(user_role_assignments::Column::RoleId)
        .filter(user_role_assignments::Column::UserId.eq(user_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("role id lookup error: {e}");
            AppError::DbTimeout
        })?
        .into_iter()
        .collect::<HashSet<_>>();

    let role_names = if role_ids.is_empty() {
        HashSet::new()
    } else {
        roles::Entity::find()
            .select_only()
            .column(roles::Column::Name)
            .filter(roles::Column::Id.is_in(role_ids.iter().copied()))
            .into_tuple::<String>()
            .all(db)
            .await
            .map_err(|e| {
                eprintln!("role name lookup error: {e}");
                AppError::DbTimeout
            })?
            .into_iter()
            .collect::<HashSet<_>>()
    };

    let authz = AuthorizationService::new(db);
    let is_admin = authz
        .user_has_permission(
            user_id,
            PERMISSION_MCP_ADMIN,
            None,
            PermissionScopeMode::RequireOrgWide,
        )
        .await
        .map_err(|e| {
            eprintln!("mcp admin permission lookup error: {e:?}");
            AppError::DbTimeout
        })?;

    Ok(McpAccessContext {
        user_id,
        department_id: user.department_id,
        department_path,
        department_depth,
        role_ids,
        role_names,
        is_admin,
    })
}

pub async fn load_server_rules(
    db: &sea_orm::DatabaseConnection,
    server_ids: &[Uuid],
) -> Result<Vec<mcp_access_policies::Model>, AppError> {
    if server_ids.is_empty() {
        return Ok(Vec::new());
    }
    mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Server))
        .filter(mcp_access_policies::Column::ServerId.is_in(server_ids.iter().copied()))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("mcp server rules lookup error: {e}");
            AppError::DbTimeout
        })
}

pub async fn load_tool_rules(
    db: &sea_orm::DatabaseConnection,
    tool_ids: &[Uuid],
) -> Result<Vec<mcp_access_policies::Model>, AppError> {
    if tool_ids.is_empty() {
        return Ok(Vec::new());
    }
    mcp_access_policies::Entity::find()
        .filter(mcp_access_policies::Column::TargetType.eq(McpAccessTarget::Tool))
        .filter(mcp_access_policies::Column::ToolId.is_in(tool_ids.iter().copied()))
        .all(db)
        .await
        .map_err(|e| {
            eprintln!("mcp tool rules lookup error: {e}");
            AppError::DbTimeout
        })
}

async fn resolve_access_from_rules(
    db: &sea_orm::DatabaseConnection,
    ctx: &McpAccessContext,
    rules: &[mcp_access_policies::Model],
) -> Result<Option<ResolvedAccess>, AppError> {
    if let Some(rule) = rules
        .iter()
        .filter(|rule| rule.access_type == McpAccessType::User)
        .filter(|rule| rule.user_id == Some(ctx.user_id))
        .max_by_key(|rule| rule.created_at)
    {
        return Ok(Some(ResolvedAccess {
            permission: rule.permission,
            resolved_via: McpResolvedVia::UserRule,
            priority: 300,
        }));
    }

    if let (Some(user_dept_id), Some(user_path), Some(user_depth)) = (
        ctx.department_id,
        ctx.department_path.as_ref(),
        ctx.department_depth,
    ) {
        let dept_rules: Vec<&mcp_access_policies::Model> = rules
            .iter()
            .filter(|rule| rule.access_type == McpAccessType::Department)
            .filter(|rule| rule.department_id.is_some())
            .collect();
        if !dept_rules.is_empty() {
            #[derive(Debug, sea_orm::FromQueryResult)]
            struct DeptPathRowDb {
                id: Uuid,
                #[sea_orm(from_alias = "path")]
                path: String,
                #[sea_orm(from_alias = "depth")]
                depth: i32,
            }

            let dept_ids: Vec<Uuid> = dept_rules
                .iter()
                .filter_map(|rule| rule.department_id)
                .collect();
            let dept_paths = departments::Entity::find()
                .select_only()
                .column(departments::Column::Id)
                .column_as(Expr::cust("path::text"), "path")
                .column_as(departments::Column::Depth, "depth")
                .filter(departments::Column::Id.is_in(dept_ids))
                .into_model::<DeptPathRowDb>()
                .all(db)
                .await
                .map_err(|e| {
                    eprintln!("department paths lookup error: {e}");
                    AppError::DbTimeout
                })?;

            let mut dept_lookup: HashMap<Uuid, (String, i32)> = HashMap::new();
            for row in dept_paths {
                dept_lookup.insert(row.id, (row.path, row.depth));
            }

            let mut best_rule: Option<(&mcp_access_policies::Model, i32)> = None;
            for rule in dept_rules {
                let Some(dept_id) = rule.department_id else {
                    continue;
                };
                let Some((path, depth)) = dept_lookup.get(&dept_id) else {
                    continue;
                };
                let is_direct = dept_id == user_dept_id;
                if is_direct || (rule.inherit_departments && is_path_within_scope(path, user_path))
                {
                    match best_rule {
                        Some((_, current_depth)) if current_depth >= *depth => {}
                        _ => best_rule = Some((rule, *depth)),
                    }
                }
            }

            if let Some((rule, depth)) = best_rule {
                let depth_diff = (user_depth - depth).max(0);
                let resolved_via = if rule.department_id == Some(user_dept_id) {
                    McpResolvedVia::DepartmentRule
                } else {
                    McpResolvedVia::DepartmentInherited
                };
                return Ok(Some(ResolvedAccess {
                    permission: rule.permission,
                    resolved_via,
                    priority: 200 - depth_diff,
                }));
            }
        }
    }

    if !ctx.role_ids.is_empty() || !ctx.role_names.is_empty() {
        let mut saw_denied = false;
        let mut saw_full = false;
        let mut saw_read_only = false;
        for rule in rules
            .iter()
            .filter(|rule| rule.access_type == McpAccessType::Role)
        {
            if let Some(role_id) = rule.role_id {
                if !ctx.role_ids.contains(&role_id) {
                    continue;
                }
            } else if let Some(role_name) = rule.role_name.as_ref() {
                if !ctx.role_names.contains(role_name) {
                    continue;
                }
            } else {
                continue;
            }
            match rule.permission {
                McpPermission::Denied => saw_denied = true,
                McpPermission::Full => saw_full = true,
                McpPermission::ReadOnly => saw_read_only = true,
            }
        }
        if saw_denied || saw_full || saw_read_only {
            let permission = if saw_denied {
                McpPermission::Denied
            } else if saw_full {
                McpPermission::Full
            } else {
                McpPermission::ReadOnly
            };
            return Ok(Some(ResolvedAccess {
                permission,
                resolved_via: McpResolvedVia::RoleRule,
                priority: 100,
            }));
        }
    }

    Ok(None)
}

pub async fn resolve_server_access_with_rules(
    db: &sea_orm::DatabaseConnection,
    ctx: &McpAccessContext,
    server: &mcp_servers::Model,
    rules: &[mcp_access_policies::Model],
) -> Result<ResolvedAccess, AppError> {
    if let Some(resolved) = resolve_access_from_rules(db, ctx, rules).await? {
        return Ok(resolved);
    }

    let permission = match server.default_access {
        mcp_servers::McpDefaultAccess::AllUsers => McpPermission::Full,
        mcp_servers::McpDefaultAccess::AdminOnly => {
            if ctx.is_admin {
                McpPermission::Full
            } else {
                McpPermission::Denied
            }
        }
        mcp_servers::McpDefaultAccess::ExplicitOnly => McpPermission::Denied,
    };

    Ok(ResolvedAccess {
        permission,
        resolved_via: McpResolvedVia::ServerDefault,
        priority: 0,
    })
}

pub async fn resolve_tool_access_with_rules(
    db: &sea_orm::DatabaseConnection,
    ctx: &McpAccessContext,
    tool: &mcp_tools::Model,
    server_access: &ResolvedAccess,
    rules: &[mcp_access_policies::Model],
) -> Result<ResolvedAccess, AppError> {
    if tool.inherit_access_from_server {
        return Ok(ResolvedAccess {
            permission: server_access.permission,
            resolved_via: McpResolvedVia::InheritedFromServer,
            priority: server_access.priority,
        });
    }

    if let Some(resolved) = resolve_access_from_rules(db, ctx, rules).await? {
        return Ok(resolved);
    }

    Ok(ResolvedAccess {
        permission: McpPermission::Denied,
        resolved_via: McpResolvedVia::ToolDefault,
        priority: 0,
    })
}

pub async fn resolve_role_reference(
    db: &DatabaseConnection,
    access_type: mcp_access_policies::McpAccessType,
    role_id: Option<Uuid>,
    role_name: Option<String>,
) -> Result<(Option<Uuid>, Option<String>), AuthError> {
    if access_type != mcp_access_policies::McpAccessType::Role {
        return Ok((None, None));
    }

    if let Some(role_id) = role_id {
        let role = roles::Entity::find_by_id(role_id)
            .one(db)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::ResourceNotFound)?;
        return Ok((Some(role.id), Some(role.name)));
    }

    if let Some(role_name) = role_name {
        let role = roles::Entity::find()
            .filter(roles::Column::Name.eq(role_name))
            .one(db)
            .await
            .map_err(|e| {
                eprintln!("role lookup error: {e}");
                AuthError::DbTimeout
            })?
            .ok_or(AuthError::ResourceNotFound)?;
        return Ok((Some(role.id), Some(role.name)));
    }

    Err(AuthError::ResourceNotFound)
}
