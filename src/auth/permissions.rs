// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

#[derive(Debug, Clone, Copy)]
pub struct PermissionDefinition {
    pub domain: &'static str,
    pub action: &'static str,
    pub is_scopeable: bool,
}

pub const PERMISSIONS: &[PermissionDefinition] = &[
    PermissionDefinition {
        domain: "ai_platform",
        action: "manage",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "ai_platform",
        action: "view",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "users",
        action: "view",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "users",
        action: "manage",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "analytics",
        action: "view",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "audit_logs",
        action: "view",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "budget",
        action: "view",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "budget",
        action: "allocate",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "mcp_servers",
        action: "view",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "mcp_servers",
        action: "admin",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "mcp_servers",
        action: "delegate",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "departments",
        action: "view",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "departments",
        action: "manage",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "sso_providers",
        action: "view",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "sso_providers",
        action: "manage",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "roles",
        action: "view",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "roles",
        action: "manage",
        is_scopeable: false,
    },
    PermissionDefinition {
        domain: "roles",
        action: "assign",
        is_scopeable: true,
    },
    PermissionDefinition {
        domain: "system",
        action: "maintain",
        is_scopeable: false,
    },
];

pub const ROLE_SUPER_ADMIN: &str = "Super Admin";
pub const ROLE_HR_ADMIN: &str = "HR Admin";
pub const ROLE_FINANCE_ADMIN: &str = "Finance Admin";
pub const ROLE_IT_ADMIN: &str = "IT Admin";
pub const ROLE_DEPARTMENT_ADMIN: &str = "Department Admin";
pub const ROLE_USER: &str = "User";
pub const ROLE_OBSERVER: &str = "Observer";

pub const PERMISSION_AI_PLATFORM_MANAGE: &str = "ai_platform:manage";
pub const PERMISSION_AI_PLATFORM_VIEW: &str = "ai_platform:view";
pub const PERMISSION_USERS_VIEW: &str = "users:view";
pub const PERMISSION_USERS_MANAGE: &str = "users:manage";
pub const PERMISSION_ANALYTICS_VIEW: &str = "analytics:view";
pub const PERMISSION_AUDIT_LOGS_VIEW: &str = "audit_logs:view";
pub const PERMISSION_BUDGET_VIEW: &str = "budget:view";
pub const PERMISSION_BUDGET_ALLOCATE: &str = "budget:allocate";
pub const PERMISSION_MCP_VIEW: &str = "mcp_servers:view";
pub const PERMISSION_MCP_ADMIN: &str = "mcp_servers:admin";
pub const PERMISSION_MCP_DELEGATE: &str = "mcp_servers:delegate";
pub const PERMISSION_DEPARTMENTS_VIEW: &str = "departments:view";
pub const PERMISSION_DEPARTMENTS_MANAGE: &str = "departments:manage";
pub const PERMISSION_SSO_PROVIDERS_VIEW: &str = "sso_providers:view";
pub const PERMISSION_SSO_PROVIDERS_MANAGE: &str = "sso_providers:manage";
pub const PERMISSION_ROLES_VIEW: &str = "roles:view";
pub const PERMISSION_ROLES_MANAGE: &str = "roles:manage";
pub const PERMISSION_ROLES_ASSIGN: &str = "roles:assign";
pub const PERMISSION_SYSTEM_MAINTAIN: &str = "system:maintain";
pub const PERMISSION_SKILLS_VIEW: &str = "skills:view";
pub const PERMISSION_SKILLS_MANAGE: &str = "skills:manage";

pub fn permission_key(domain: &str, action: &str) -> String {
    format!("{domain}:{action}")
}

pub fn split_permission_key(permission: &str) -> Option<(&str, &str)> {
    permission.split_once(':')
}
