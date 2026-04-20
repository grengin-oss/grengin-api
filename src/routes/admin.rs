use crate::{
    handlers::{
        admin_ai::{
            delete_ai_engines_api_key_key, get_ai_engine_models_by_key, get_ai_engines,
            get_ai_engines_by_key, update_ai_engines_by_key, validate_ai_engines_by_key,
        },
        admin_analytics::{
            get_analytics_overview, get_department_analytics, get_timeseries_analytics,
            get_user_analytics,
        },
        admin_audit::{export_audit_logs, get_audit_logs, redact_audit_logs_for_user},
        admin_department::{
            add_users_in_department, create_department, delete_department, get_department_by_id,
            get_departments_tree, get_users_from_department, list_departments, move_department,
            remove_users_from_department, update_department,
        },
        admin_department_budgets::get_department_budget,
        admin_embedding::{get_embedding_config, update_embedding_config},
        admin_mcp::{
            create_mcp_access_rule, delete_mcp_access_rule, get_mcp_server_access,
            update_mcp_server_access, update_mcp_server_default,
        },
        admin_prompts::{
            assign_department_prompt, create_role_prompt, delete_department_prompt,
            delete_role_prompt, get_prompt_metrics, get_role_prompt, list_department_prompts,
            list_role_prompts, update_department_prompt, update_role_prompt,
        },
        admin_roles::{
            assign_role_to_user, create_role, delete_role, get_permissions, get_role_by_id,
            list_roles, list_user_role_assignments, remove_role_from_user, update_role,
        },
        admin_sso_provider::{
            delete_sso_provider_by_id, get_sso_provider_by_id, get_sso_providers,
            update_sso_provider_by_id,
        },
        admin_users::{
            add_new_user, delete_user, get_user_by_id, get_users, patch_user_status, update_user,
        },
        branding::{get_admin_branding, update_branding},
    },
    state::SharedState,
};
use axum::{
    routing::{delete, get, patch, post, put},
    Router,
};

pub fn admin_routes() -> Router<SharedState> {
    Router::new()
        .route("/admin/users", get(get_users).post(add_new_user))
        .route(
            "/admin/users/{user_id}",
            put(update_user).delete(delete_user).get(get_user_by_id),
        )
        .route("/admin/users/{user_id}/status", patch(patch_user_status))
        .route(
            "/admin/branding",
            get(get_admin_branding).put(update_branding),
        )
        .route(
            "/admin/embedding-config",
            get(get_embedding_config).put(update_embedding_config),
        )
        .route("/admin/ai-engines", get(get_ai_engines))
        .route(
            "/admin/ai-engines/{engine_key}",
            put(update_ai_engines_by_key).get(get_ai_engines_by_key),
        )
        .route(
            "/admin/ai-engines/{engine-key}/validate",
            post(validate_ai_engines_by_key),
        )
        .route(
            "/admin/ai-engines/{engine-key}/api-key",
            delete(delete_ai_engines_api_key_key),
        )
        .route(
            "/admin/ai-engines/{engine-key}/models",
            get(get_ai_engine_models_by_key),
        )
        .route("/admin/sso-providers", get(get_sso_providers))
        .route(
            "/admin/sso-providers/{provider_id}",
            put(update_sso_provider_by_id)
                .delete(delete_sso_provider_by_id)
                .get(get_sso_provider_by_id),
        )
        .route("/admin/analytics/overview", get(get_analytics_overview))
        .route("/admin/analytics/users", get(get_user_analytics))
        .route(
            "/admin/analytics/departments",
            get(get_department_analytics),
        )
        .route("/admin/analytics/timeseries", get(get_timeseries_analytics))
        .route("/admin/audit-logs", get(get_audit_logs))
        .route("/admin/audit-logs/export", get(export_audit_logs))
        .route(
            "/admin/audit-logs/redact/{user_id}",
            post(redact_audit_logs_for_user),
        )
        .route("/api/admin/audit-logs", get(get_audit_logs))
        .route(
            "/admin/departments",
            get(list_departments).post(create_department),
        )
        .route("/admin/departments/tree", get(get_departments_tree))
        .route(
            "/admin/departments/{department_id}",
            get(get_department_by_id)
                .put(update_department)
                .delete(delete_department),
        )
        .route(
            "/admin/departments/{department_id}/move",
            post(move_department),
        )
        .route(
            "/admin/departments/{department_id}/budget",
            get(get_department_budget),
        )
        .route(
            "/admin/departments/{department_id}/members",
            post(add_users_in_department)
                .get(get_users_from_department)
                .delete(remove_users_from_department),
        )
        .route("/admin/permissions", get(get_permissions))
        .route("/admin/roles", get(list_roles).post(create_role))
        .route(
            "/admin/roles/{role_id}",
            get(get_role_by_id).put(update_role).delete(delete_role),
        )
        .route(
            "/admin/users/{user_id}/roles",
            get(list_user_role_assignments).post(assign_role_to_user),
        )
        .route(
            "/admin/users/{user_id}/roles/{assignment_id}",
            delete(remove_role_from_user),
        )
        .route(
            "/admin/mcp-servers/{server_id}/access",
            get(get_mcp_server_access).put(update_mcp_server_access),
        )
        .route(
            "/admin/mcp-servers/{server_id}/access/default",
            put(update_mcp_server_default),
        )
        .route(
            "/admin/mcp-servers/{server_id}/access/rules",
            post(create_mcp_access_rule),
        )
        .route(
            "/admin/mcp-servers/{server_id}/access/rules/{rule_id}",
            delete(delete_mcp_access_rule),
        )
        .route(
            "/admin/role-prompts",
            get(list_role_prompts).post(create_role_prompt),
        )
        .route(
            "/admin/role-prompts/{prompt_id}",
            get(get_role_prompt)
                .put(update_role_prompt)
                .delete(delete_role_prompt),
        )
        .route(
            "/admin/department-prompts",
            get(list_department_prompts).post(assign_department_prompt),
        )
        .route(
            "/admin/department-prompts/{assignment_id}",
            put(update_department_prompt).delete(delete_department_prompt),
        )
        .route("/admin/prompt-metrics", get(get_prompt_metrics))
}
