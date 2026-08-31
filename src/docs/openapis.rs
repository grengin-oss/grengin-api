// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use crate::auth::claims::Claims;
use crate::auth::error::{AuthError, AuthErrorCode, AuthErrorDetailVariant, Error};
use crate::docs::{app_error_catlog::AppErrorCatalogItem, security::ApiSecurityAddon};
use crate::dto::admin_ai::{
    AIEngineConnectionTest, AIEngineCreate, AIEngineDetail, AIEngineModels,
    AIEnginePluginValidationRequest, AIEnginePluginValidationResponse, AIEngineUpdate,
    AIEngineValidation, AiModel, AiModelCapabilities, PluginModel,
};
use crate::dto::admin_department::{
    Department, DepartmentCreate, DepartmentListQuery, DepartmentMembersResponse, DepartmentMove,
    DepartmentTree, DepartmentTreeNode, DepartmentTreeQuery, DepartmentsListResponse,
};
use crate::dto::admin_department_budget::{DepartmentBudgetStatus, SubDepartmentBudgetDto};
use crate::dto::admin_embedding::{EmbeddingConfigResponse, EmbeddingConfigUpdateRequest};
use crate::dto::admin_mcp::{McpAccessDefault, McpServerAccess};
use crate::dto::admin_reconfigure_dto::{
    BinariesUpdateRequest, BinariesUpdateResponse, DomainReconfigureRequest,
    DomainReconfigureResponse, ReconfigureAvailableResponse, ReconfigureScriptAvailability,
};
use crate::dto::admin_roles::{
    PermissionDto, PermissionsResponse, RoleDto, RoleRequest, RoleUpdateRequest, RolesResponse,
    UserRoleAssignmentDto, UserRoleAssignmentInput, UserRoleAssignmentsResponse,
};
use crate::dto::admin_sso_providers::{
    SsoProvider, SsoProviderUpdate, SsoProviderValidationRequest, SsoProviderValidationResponse,
};
use crate::dto::admin_user::{PaginatedUsers, User, UserCreate, UserPatchRequest, UserUpdate};
use crate::dto::analytics::{
    AnalyticsOverview, AnalyticsTimeSeries, DepartmentAnalytics, DepartmentAnalyticsQuery,
    ScopedUserAnalyticsQuery, UserAnalytics,
};
use crate::dto::artifacts::{ArtifactListResponse, ArtifactResponse};
use crate::dto::audit_logs::{
    AuditLogAction, AuditLogEntry, AuditLogExportFormat, AuditLogRedactResponse,
    AuditLogsExportQuery, AuditLogsQuery, AuditLogsResponse,
};
use crate::dto::auth::{AuthInit, AuthToken, RefreshToken, TokenType};
use crate::dto::branding::{Branding, BrandingUpdate};
use crate::dto::chat::{
    ArchiveChatRequest, ConversationResponse, MessageParts, MessageResponse,
    PaginatedConversations, TokenUsage,
};
use crate::dto::chat_stream::{ChatInput, ChatStream};
use crate::dto::common::{PaginationQuery, SortRule};
use crate::dto::files::{Attachment, File, FileResponse, FileUploadRequest};
use crate::dto::mcp::{
    BulkToolAccessUpdate, BulkToolAccessUpdateResponse, McpAccessRule, McpAccessRuleInput,
    McpAuthorize, McpDisconnect, McpEffectiveAccessResponse, McpEffectiveServerAccess,
    McpEffectiveToolAccess, McpOauthCallback, McpResolvedVia, McpServer, McpServerAccessList,
    McpServerAccessUpdate, McpServerCatalogEntry, McpServerCatalogResponse, McpServerCreate,
    McpServerTestResult, McpServerUpdate, McpSyncResult, McpTool, McpToolAccess,
    McpToolAccessUpdate, McpToolExecution, McpToolSummary, McpTools, McpUserConnection,
    McpUserConnections, PaginatedMcpServers, PaginatedMcpToolExecutions,
};
use crate::dto::me::EffectivePermissionsResponse;
use crate::dto::me::MeDepartmentUsersResponse;
use crate::dto::models::{ModelInfo, ProviderInfo};
use crate::dto::notifications::{
    NotificationDto, NotificationsListQuery, NotificationsListResponse,
};
use crate::dto::oauth::AuthCallback;
use crate::dto::projects::{
    AddMcpServerRequest, AddMemberRequest, AddSourceRequest, ArtifactCreateRequest,
    ArtifactUpdateRequest, InstructionsUpdateRequest, LinkProjectRequest, ProjectCreateRequest,
    ProjectDetailResponse, ProjectListQuery, ProjectListResponse, ProjectMcpServerResponse,
    ProjectMemberResponse, ProjectResponse, ProjectSourceResponse, ProjectUpdateRequest,
    ShareProjectResponse, UserSearchItem, UserSearchResponse,
};
use crate::dto::prompts::{
    DepartmentPromptAssignmentCreate, DepartmentPromptAssignmentListQuery,
    DepartmentPromptAssignmentResponse, DepartmentPromptAssignmentUpdate, PromptFeedbackRequest,
    PromptMetricsQuery, PromptMetricsResponse, PromptSource, RolePromptCreate, RolePromptListQuery,
    RolePromptResponse, RolePromptUpdate, SystemPromptResponse, UserPromptPreferenceRequest,
};
use crate::dto::skills::{
    ConversationSkillResponse, LinkSkillRequest, SkillCreateRequest, SkillListQuery,
    SkillListResponse, SkillResponse, SkillToolsConfig, SkillUpdateRequest, UserSkillCreateRequest,
    UserSkillListQuery, UserSkillUpdateRequest,
};
use crate::dto::system_metrics::{
    ContainerMetrics, DatabaseMetrics, DiskMetrics, MachineMetrics, SystemMetricsResponse,
};
use crate::error::{AppError, ErrorDetail, ErrorDetailVariant, ErrorResponse};
use crate::handlers::artifacts;
use crate::handlers::{
    admin_ai, admin_ai_plugins, admin_analytics, admin_audit, admin_department,
    admin_department_budgets, admin_embedding, admin_mcp, admin_prompts, admin_reconfigure,
    admin_roles, admin_sso_provider, admin_system, admin_users, auth, branding, chat, chat_stream,
    file, mcp, me, me_prompts, me_skills, message, models, notifications, oidc, open_error,
    projects, skills,
};
use crate::models::departments::{ActionOnExceed, BudgetPeriod};
use crate::models::mcp_access_policies::{McpAccessType, McpPermission};
use crate::models::mcp_servers::{McpDefaultAccess, McpTransportType};
use crate::models::messages::ChatRole;
use crate::models::users::UserStatus;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    paths(
        artifacts::get_artifact,
        artifacts::list_conversation_artifacts,
        artifacts::delete_artifact,
        auth::handle_refresh_token,
        oidc::oidc_login_start,
        oidc::list_auth_providers,
        oidc::oidc_oauth_callback_get,
        oidc::oidc_oauth_callback_post,
        oidc::azure_mobile_oauth_callback_get,
        oidc::azure_mobile_oauth_callback_post,
        chat::get_chat_by_id,
        chat::get_chats,
        chat::delete_chat_by_id,
        chat::update_chat_by_id,
        chat_stream::handle_chat_stream_doc,
        chat_stream::handle_chat_stream_path_doc,
        chat_stream::cancel_chat_stream,
        message::delete_chat_message_by_id,
        message::edit_chat_message_by_id_and_stream,
        admin_users::add_new_user,
        admin_users::get_users,
        admin_users::update_user,
        admin_users::delete_user,
        admin_users::get_user_by_id,
        admin_users::patch_user_status,
        branding::get_branding,
        branding::get_admin_branding,
        branding::update_branding,
        admin_embedding::get_embedding_config,
        admin_embedding::update_embedding_config,
        admin_ai::get_ai_engines,
        admin_ai::update_ai_engines_by_key,
        admin_ai::get_ai_engines_by_key,
        admin_ai::validate_ai_engines_by_key,
        admin_ai::delete_ai_engines_api_key_key,
        admin_ai::get_ai_engine_models_by_key,
        admin_ai_plugins::get_ai_engine_plugin_schema,
        admin_ai_plugins::validate_ai_engine_plugin,
        admin_ai_plugins::create_ai_engine,
        admin_ai_plugins::delete_ai_engine,
        admin_ai_plugins::test_ai_engine_connection,
        admin_sso_provider::get_sso_providers,
        admin_sso_provider::create_sso_provider,
        admin_sso_provider::get_sso_provider_by_id,
        admin_sso_provider::validate_sso_provider_by_id,
        admin_sso_provider::update_sso_provider_by_id,
        admin_sso_provider::delete_sso_provider_by_id,
        file::get_file_by_id,
        file::get_files,
        file::delete_file_by_id,
        file::download_file,
        file::upload_file,
        models::get_list_models,
        open_error::get_app_error_catalog,
        open_error::get_auth_error_catalog,
        admin_analytics::get_analytics_overview,
        admin_analytics::get_user_analytics,
        admin_analytics::get_timeseries_analytics,
        admin_analytics::get_department_analytics,
        admin_audit::get_audit_logs,
        admin_audit::get_audit_actions,
        admin_audit::export_audit_logs,
        admin_audit::redact_audit_logs_for_user,
        admin_system::get_system_metrics,
        admin_department::create_department,
        admin_department::update_department,
        admin_department::delete_department,
        admin_department::get_department_by_id,
        admin_department::list_departments,
        admin_department::get_departments_tree,
        admin_department::move_department,
        admin_department::add_users_in_department,
        admin_department::remove_users_from_department,
        admin_department::get_users_from_department,
        admin_roles::get_permissions,
        admin_roles::list_roles,
        admin_roles::create_role,
        admin_roles::get_role_by_id,
        admin_roles::update_role,
        admin_roles::delete_role,
        admin_roles::list_user_role_assignments,
        admin_roles::assign_role_to_user,
        admin_roles::remove_role_from_user,
        admin_mcp::get_mcp_server_access,
        admin_mcp::update_mcp_server_access,
        admin_mcp::update_mcp_server_default,
        admin_mcp::create_mcp_access_rule,
        admin_mcp::delete_mcp_access_rule,
        admin_prompts::list_role_prompts,
        admin_prompts::get_role_prompt,
        admin_prompts::create_role_prompt,
        admin_prompts::update_role_prompt,
        admin_prompts::delete_role_prompt,
        admin_prompts::list_department_prompts,
        admin_prompts::assign_department_prompt,
        admin_prompts::update_department_prompt,
        admin_prompts::delete_department_prompt,
        admin_prompts::get_prompt_metrics,
        admin_reconfigure::get_reconfigure_available,
        admin_reconfigure::reconfigure_domain,
        admin_reconfigure::update_binaries,
        me_prompts::get_my_system_prompt,
        me_prompts::set_my_system_prompt,
        me_prompts::reset_my_system_prompt,
        me_prompts::submit_prompt_feedback,
        mcp::list_mcp_servers,
        mcp::list_public_mcp_servers,
        mcp::create_mcp_server,
        mcp::get_mcp_server,
        mcp::update_mcp_server,
        mcp::delete_mcp_server,
        mcp::test_mcp_server,
        mcp::sync_mcp_server_tools,
        mcp::list_mcp_server_executions,
        mcp::get_mcp_server_tools_access,
        mcp::update_mcp_server_tools_access,
        mcp::get_mcp_tool_access,
        mcp::update_mcp_tool_access,
        mcp::list_mcp_tools,
        mcp::list_mcp_connections,
        mcp::authorize_mcp_connection,
        mcp::disconnect_mcp_connection,
        mcp::mcp_oauth_callback,
        mcp::get_mcp_effective_access,
        me::get_my_permissions,
        notifications::list_my_notifications,
        notifications::mark_notification_read,
        notifications::stream_my_notifications,
        me::get_my_administered_department_analytics,
        me::get_my_administered_department_user_analytics,
        me::get_my_administered_department_members,
        me::get_my_administered_departments_list,
        me::get_my_administered_departments_tree,
        admin_department_budgets::get_department_budget,
        projects::list_projects,
        projects::create_project,
        projects::get_project,
        projects::get_project_detail,
        projects::update_project,
        projects::delete_project,
        projects::add_project_member,
        projects::remove_project_member,
        projects::list_project_members,
        projects::search_users_for_project,
        projects::update_project_instructions,
        projects::add_project_source,
        projects::delete_project_source,
        projects::list_project_artifacts,
        projects::add_project_artifact,
        projects::get_project_artifact,
        projects::update_project_artifact,
        projects::delete_project_artifact,
        projects::share_project,
        projects::link_project_to_conversation,
        projects::unlink_project_from_conversation,
        projects::list_project_mcp_servers,
        projects::add_project_mcp_server,
        projects::remove_project_mcp_server,
        skills::list_skills,
        skills::get_skill,
        skills::create_skill,
        skills::update_skill,
        skills::delete_skill,
        skills::list_conversation_skill_links,
        skills::link_skill,
        skills::unlink_skill,
        me_skills::list_my_skills,
        me_skills::get_my_skill,
        me_skills::create_my_skill,
        me_skills::update_my_skill,
        me_skills::delete_my_skill,
    ),
    components(
        schemas(
            ArtifactResponse,
            ArtifactListResponse,
            AuthInit,
            AuthToken,
            TokenType,
            UserStatus,
            ChatRole,
            Claims,
            ErrorResponse,
            ErrorDetail,
            ErrorDetailVariant,
            ArchiveChatRequest,
            MessageResponse,
            ConversationResponse,
            PaginatedConversations,
            File,
            MessageParts,
            TokenUsage,
            ChatStream,
            ChatInput,
            Attachment,
            AuthCallback,
            SortRule,
            PaginationQuery,
            PaginatedUsers,
            UserUpdate,
            UserCreate,
            Branding,
            BrandingUpdate,
            User,
            UserPatchRequest,
            AIEngineDetail,
            AIEngineUpdate,
            EmbeddingConfigResponse,
            EmbeddingConfigUpdateRequest,
            FileResponse,
            FileUploadRequest,
            PluginModel,
            ProviderInfo,
            ModelInfo,
            AIEngineValidation,
            AIEngineModels,
            AiModel,
            AiModelCapabilities,
            SsoProvider,
            SsoProviderUpdate,
            SsoProviderValidationRequest,
            SsoProviderValidationResponse,
            AuthError,
            AppError,
            AuthErrorCode,
            AuthErrorDetailVariant,
            Error,
            RefreshToken,
            AppErrorCatalogItem,
            DepartmentListQuery,
            Department,
            DepartmentsListResponse,
            DepartmentCreate,
            DepartmentMembersResponse,
            MeDepartmentUsersResponse,
            NotificationDto,
            NotificationsListResponse,
            NotificationsListQuery,
            AnalyticsOverview,
            DepartmentAnalytics,
            DepartmentAnalyticsQuery,
            AnalyticsTimeSeries,
            UserAnalytics,
            AuditLogsQuery,
            AuditLogsResponse,
            AuditLogEntry,
            AuditLogAction,
            AuditLogsExportQuery,
            AuditLogExportFormat,
            AuditLogRedactResponse,
            ScopedUserAnalyticsQuery,
            BudgetPeriod,
            ActionOnExceed,
            DepartmentTreeQuery,
            DepartmentTreeNode,
            DepartmentTree,
            DepartmentMove,
            SubDepartmentBudgetDto,
            DepartmentBudgetStatus,
            PermissionDto,
            PermissionsResponse,
            RoleDto,
            RoleRequest,
            RoleUpdateRequest,
            RolesResponse,
            UserRoleAssignmentDto,
            UserRoleAssignmentInput,
            UserRoleAssignmentsResponse,
            McpAccessDefault,
            McpServerAccess,
            McpServer,
            McpServerCreate,
            McpServerUpdate,
            PaginatedMcpServers,
            McpServerTestResult,
            McpSyncResult,
            McpTool,
            McpToolSummary,
            McpTools,
            McpToolExecution,
            PaginatedMcpToolExecutions,
            McpServerCatalogEntry,
            McpServerCatalogResponse,
            McpAccessRule,
            McpAccessRuleInput,
            McpServerAccessList,
            McpServerAccessUpdate,
            McpToolAccess,
            McpToolAccessUpdate,
            BulkToolAccessUpdate,
            BulkToolAccessUpdateResponse,
            McpUserConnection,
            McpUserConnections,
            McpAuthorize,
            McpDisconnect,
            McpOauthCallback,
            EffectivePermissionsResponse,
            McpEffectiveAccessResponse,
            McpEffectiveServerAccess,
            McpEffectiveToolAccess,
            McpResolvedVia,
            McpDefaultAccess,
            McpTransportType,
            McpAccessType,
            McpPermission,
            RolePromptResponse,
            RolePromptCreate,
            RolePromptUpdate,
            RolePromptListQuery,
            DepartmentPromptAssignmentResponse,
            DepartmentPromptAssignmentCreate,
            DepartmentPromptAssignmentUpdate,
            DepartmentPromptAssignmentListQuery,
            PromptMetricsResponse,
            PromptMetricsQuery,
            SystemPromptResponse,
            UserPromptPreferenceRequest,
            PromptFeedbackRequest,
            PromptSource,
            DomainReconfigureRequest,
            DomainReconfigureResponse,
            BinariesUpdateRequest,
            BinariesUpdateResponse,
            ReconfigureScriptAvailability,
            ReconfigureAvailableResponse,
            SystemMetricsResponse,
            MachineMetrics,
            DiskMetrics,
            ContainerMetrics,
            DatabaseMetrics,
            ProjectCreateRequest,
            ProjectUpdateRequest,
            ProjectListQuery,
            ProjectResponse,
            ProjectListResponse,
            ProjectDetailResponse,
            ProjectSourceResponse,
            AddMemberRequest,
            ProjectMemberResponse,
            UserSearchItem,
            UserSearchResponse,
            InstructionsUpdateRequest,
            AddSourceRequest,
            ArtifactCreateRequest,
            ArtifactUpdateRequest,
            ShareProjectResponse,
            LinkProjectRequest,
            ProjectMcpServerResponse,
            AddMcpServerRequest,
            SkillToolsConfig,
            SkillCreateRequest,
            SkillUpdateRequest,
            SkillListQuery,
            SkillResponse,
            SkillListResponse,
            LinkSkillRequest,
            ConversationSkillResponse,
            UserSkillCreateRequest,
            UserSkillUpdateRequest,
            UserSkillListQuery,
            AIEngineCreate,
            AIEnginePluginValidationRequest,
            AIEnginePluginValidationResponse,
            AIEngineConnectionTest,
        )
    ),
    tags(
        (name = "auth", description = "Authentication & user endpoints"),
        (name = "branding", description = "Branding configuration endpoints"),
        (name = "admin", description = "Admin endpoints"),
        (name = "me", description = "Current user permissions"),
        (name = "mcp", description = "MCP tools & connections"),
        (name = "root", description = "Root / health"),
        (name = "skills", description = "Skills management & conversation skill links"),
        (name = "artifacts", description = "Chat artifact retrieval & deletion"),
    ),
    modifiers(
        &ApiSecurityAddon
    )
)]
pub struct ApiDoc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_plugin_schemas_use_snake_case() {
        let document = serde_json::to_value(ApiDoc::openapi()).expect("OpenAPI JSON");
        let schemas = &document["components"]["schemas"];

        let validation_request = &schemas["AIEnginePluginValidationRequest"]["properties"];
        assert!(validation_request.get("plugin_config").is_some());
        assert!(validation_request.get("pluginConfig").is_none());

        let create_request = &schemas["AIEngineCreate"]["properties"];
        assert!(create_request.get("plugin_config").is_some());
        assert!(create_request.get("is_enabled").is_some());
        assert!(create_request.get("pluginConfig").is_none());
        assert!(create_request.get("isEnabled").is_none());

        let update_request = &schemas["AIEngineUpdate"]["properties"];
        assert!(update_request.get("display_name").is_some());
        assert!(update_request.get("api_key").is_some());
        assert!(update_request.get("default_image_gen_model").is_some());
        assert!(update_request.get("displayName").is_none());
        assert!(update_request.get("apiKey").is_none());

        let detail_response = &schemas["AIEngineDetail"]["properties"];
        assert!(detail_response.get("engine_key").is_some());
        assert!(detail_response.get("plugin_version").is_some());
        assert!(detail_response.get("api_key_configured").is_some());
        assert!(detail_response.get("engineKey").is_none());
        assert!(detail_response.get("pluginVersion").is_none());

        let plugin_config = &schemas["PluginConfig"]["properties"];
        assert!(plugin_config.get("baseUrlOverride").is_some());
        assert!(plugin_config.get("allowInsecureHttp").is_some());
        assert!(plugin_config.get("allowPrivateNetwork").is_some());
        assert!(plugin_config.get("base_url_override").is_none());

        let validation_response = &schemas["AIEnginePluginValidationResponse"]["properties"];
        assert!(validation_response.get("engine_key").is_some());
        assert!(validation_response.get("credential_required").is_some());
        assert!(validation_response.get("engineKey").is_none());

        let connection_response = &schemas["AIEngineConnectionTest"]["properties"];
        assert!(connection_response.get("models_available").is_some());
        assert!(connection_response.get("error_class").is_some());
        assert!(connection_response.get("modelsAvailable").is_none());
        assert!(connection_response.get("errorClass").is_none());

        let model_response = &schemas["AiModel"]["properties"];
        assert!(model_response.get("model_type").is_some());
        assert!(model_response.get("input_token_rate").is_some());
        assert!(
            model_response
                .get("image_cached_input_token_rate")
                .is_some()
        );
        assert!(model_response.get("max_input_tokens").is_some());
        assert!(model_response.get("modelType").is_none());
        assert!(model_response.get("inputTokenRate").is_none());

        let model_capabilities = &schemas["AiModelCapabilities"]["properties"];
        assert!(model_capabilities.get("function_calling").is_some());
        assert!(model_capabilities.get("image_generation").is_some());
        assert!(model_capabilities.get("functionCalling").is_none());

        let paths = &document["paths"];
        assert!(paths.get("/admin/ai-engines/{engine_key}").is_some());
        assert!(paths.get("/admin/ai-engines/{ai_engine_key}").is_none());
        let parameters = &paths["/admin/ai-engines/{engine_key}"]["put"]["parameters"];
        assert_eq!(parameters[0]["name"], "engine_key");
    }
}
