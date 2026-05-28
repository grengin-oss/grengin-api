use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditLogAction {
    Login,
    ConversationCreated,
    ConversationUpdated,
    ConversationDeleted,
    MessageSent,
    MessageUpdated,
    MessageDeleted,
    MessageStreamCancelled,
    FileUploaded,
    NotificationMarkedRead,
    UserSystemPromptSet,
    UserSystemPromptReset,
    UserPromptFeedbackSubmitted,
    McpConnectionAuthorized,
    McpConnectionDisconnected,
    AdminUserCreated,
    AdminUserUpdated,
    AdminUserStatusUpdated,
    AdminUserDeleted,
    AdminBrandingUpdated,
    AdminEmbeddingConfigUpdated,
    AdminAiEngineUpdated,
    AdminAiEngineApiKeyRemovedOrRotated,
    AdminSsoProviderUpdated,
    AdminSsoProviderDeleted,
    AdminDepartmentCreated,
    AdminDepartmentUpdated,
    AdminDepartmentDeleted,
    AdminDepartmentMoved,
    AdminDepartmentMembersAdded,
    AdminDepartmentMembersRemoved,
    AdminRoleCreated,
    AdminRoleUpdated,
    AdminRoleDeleted,
    AdminRoleAssignedToUser,
    AdminRoleRemovedFromUser,
    AdminMcpServerCreated,
    AdminMcpServerUpdated,
    AdminMcpServerDeleted,
    AdminMcpServerToolsSynced,
    AdminMcpServerAccessUpdated,
    AdminMcpServerDefaultAccessUpdated,
    AdminMcpAccessRuleCreated,
    AdminMcpAccessRuleDeleted,
    AdminMcpServerToolsAccessUpdated,
    AdminMcpToolAccessUpdated,
    AdminRolePromptCreated,
    AdminRolePromptUpdated,
    AdminRolePromptDeleted,
    AdminDepartmentPromptAssigned,
    AdminDepartmentPromptUpdated,
    AdminDepartmentPromptDeleted,
    AdminAuditLogsRedactedForUser,
    AdminReconfigureStarted,
    AdminDomainReconfigured,
    AdminBinariesUpdated,
}

impl AuditLogAction {
    pub fn all() -> Vec<Self> {
        vec![
            AuditLogAction::Login,
            AuditLogAction::ConversationCreated,
            AuditLogAction::ConversationUpdated,
            AuditLogAction::ConversationDeleted,
            AuditLogAction::MessageSent,
            AuditLogAction::MessageUpdated,
            AuditLogAction::MessageDeleted,
            AuditLogAction::MessageStreamCancelled,
            AuditLogAction::FileUploaded,
            AuditLogAction::NotificationMarkedRead,
            AuditLogAction::UserSystemPromptSet,
            AuditLogAction::UserSystemPromptReset,
            AuditLogAction::UserPromptFeedbackSubmitted,
            AuditLogAction::McpConnectionAuthorized,
            AuditLogAction::McpConnectionDisconnected,
            AuditLogAction::AdminUserCreated,
            AuditLogAction::AdminUserUpdated,
            AuditLogAction::AdminUserStatusUpdated,
            AuditLogAction::AdminUserDeleted,
            AuditLogAction::AdminBrandingUpdated,
            AuditLogAction::AdminEmbeddingConfigUpdated,
            AuditLogAction::AdminAiEngineUpdated,
            AuditLogAction::AdminAiEngineApiKeyRemovedOrRotated,
            AuditLogAction::AdminSsoProviderUpdated,
            AuditLogAction::AdminSsoProviderDeleted,
            AuditLogAction::AdminDepartmentCreated,
            AuditLogAction::AdminDepartmentUpdated,
            AuditLogAction::AdminDepartmentDeleted,
            AuditLogAction::AdminDepartmentMoved,
            AuditLogAction::AdminDepartmentMembersAdded,
            AuditLogAction::AdminDepartmentMembersRemoved,
            AuditLogAction::AdminRoleCreated,
            AuditLogAction::AdminRoleUpdated,
            AuditLogAction::AdminRoleDeleted,
            AuditLogAction::AdminRoleAssignedToUser,
            AuditLogAction::AdminRoleRemovedFromUser,
            AuditLogAction::AdminMcpServerCreated,
            AuditLogAction::AdminMcpServerUpdated,
            AuditLogAction::AdminMcpServerDeleted,
            AuditLogAction::AdminMcpServerToolsSynced,
            AuditLogAction::AdminMcpServerAccessUpdated,
            AuditLogAction::AdminMcpServerDefaultAccessUpdated,
            AuditLogAction::AdminMcpAccessRuleCreated,
            AuditLogAction::AdminMcpAccessRuleDeleted,
            AuditLogAction::AdminMcpServerToolsAccessUpdated,
            AuditLogAction::AdminMcpToolAccessUpdated,
            AuditLogAction::AdminRolePromptCreated,
            AuditLogAction::AdminRolePromptUpdated,
            AuditLogAction::AdminRolePromptDeleted,
            AuditLogAction::AdminDepartmentPromptAssigned,
            AuditLogAction::AdminDepartmentPromptUpdated,
            AuditLogAction::AdminDepartmentPromptDeleted,
            AuditLogAction::AdminAuditLogsRedactedForUser,
            AuditLogAction::AdminReconfigureStarted,
            AuditLogAction::AdminDomainReconfigured,
            AuditLogAction::AdminBinariesUpdated,
        ]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AuditLogAction::Login => "login",
            AuditLogAction::ConversationCreated => "conversation_created",
            AuditLogAction::ConversationUpdated => "conversation_updated",
            AuditLogAction::ConversationDeleted => "conversation_deleted",
            AuditLogAction::MessageSent => "message_sent",
            AuditLogAction::MessageUpdated => "message_updated",
            AuditLogAction::MessageDeleted => "message_deleted",
            AuditLogAction::MessageStreamCancelled => "message_stream_cancelled",
            AuditLogAction::FileUploaded => "file_uploaded",
            AuditLogAction::NotificationMarkedRead => "notification_marked_read",
            AuditLogAction::UserSystemPromptSet => "user_system_prompt_set",
            AuditLogAction::UserSystemPromptReset => "user_system_prompt_reset",
            AuditLogAction::UserPromptFeedbackSubmitted => "user_prompt_feedback_submitted",
            AuditLogAction::McpConnectionAuthorized => "mcp_connection_authorized",
            AuditLogAction::McpConnectionDisconnected => "mcp_connection_disconnected",
            AuditLogAction::AdminUserCreated => "admin_user_created",
            AuditLogAction::AdminUserUpdated => "admin_user_updated",
            AuditLogAction::AdminUserStatusUpdated => "admin_user_status_updated",
            AuditLogAction::AdminUserDeleted => "admin_user_deleted",
            AuditLogAction::AdminBrandingUpdated => "admin_branding_updated",
            AuditLogAction::AdminEmbeddingConfigUpdated => "admin_embedding_config_updated",
            AuditLogAction::AdminAiEngineUpdated => "admin_ai_engine_updated",
            AuditLogAction::AdminAiEngineApiKeyRemovedOrRotated => {
                "admin_ai_engine_api_key_removed_or_rotated"
            }
            AuditLogAction::AdminSsoProviderUpdated => "admin_sso_provider_updated",
            AuditLogAction::AdminSsoProviderDeleted => "admin_sso_provider_deleted",
            AuditLogAction::AdminDepartmentCreated => "admin_department_created",
            AuditLogAction::AdminDepartmentUpdated => "admin_department_updated",
            AuditLogAction::AdminDepartmentDeleted => "admin_department_deleted",
            AuditLogAction::AdminDepartmentMoved => "admin_department_moved",
            AuditLogAction::AdminDepartmentMembersAdded => "admin_department_members_added",
            AuditLogAction::AdminDepartmentMembersRemoved => "admin_department_members_removed",
            AuditLogAction::AdminRoleCreated => "admin_role_created",
            AuditLogAction::AdminRoleUpdated => "admin_role_updated",
            AuditLogAction::AdminRoleDeleted => "admin_role_deleted",
            AuditLogAction::AdminRoleAssignedToUser => "admin_role_assigned_to_user",
            AuditLogAction::AdminRoleRemovedFromUser => "admin_role_removed_from_user",
            AuditLogAction::AdminMcpServerCreated => "admin_mcp_server_created",
            AuditLogAction::AdminMcpServerUpdated => "admin_mcp_server_updated",
            AuditLogAction::AdminMcpServerDeleted => "admin_mcp_server_deleted",
            AuditLogAction::AdminMcpServerToolsSynced => "admin_mcp_server_tools_synced",
            AuditLogAction::AdminMcpServerAccessUpdated => "admin_mcp_server_access_updated",
            AuditLogAction::AdminMcpServerDefaultAccessUpdated => {
                "admin_mcp_server_default_access_updated"
            }
            AuditLogAction::AdminMcpAccessRuleCreated => "admin_mcp_access_rule_created",
            AuditLogAction::AdminMcpAccessRuleDeleted => "admin_mcp_access_rule_deleted",
            AuditLogAction::AdminMcpServerToolsAccessUpdated => {
                "admin_mcp_server_tools_access_updated"
            }
            AuditLogAction::AdminMcpToolAccessUpdated => "admin_mcp_tool_access_updated",
            AuditLogAction::AdminRolePromptCreated => "admin_role_prompt_created",
            AuditLogAction::AdminRolePromptUpdated => "admin_role_prompt_updated",
            AuditLogAction::AdminRolePromptDeleted => "admin_role_prompt_deleted",
            AuditLogAction::AdminDepartmentPromptAssigned => "admin_department_prompt_assigned",
            AuditLogAction::AdminDepartmentPromptUpdated => "admin_department_prompt_updated",
            AuditLogAction::AdminDepartmentPromptDeleted => "admin_department_prompt_deleted",
            AuditLogAction::AdminAuditLogsRedactedForUser => "admin_audit_logs_redacted_for_user",
            AuditLogAction::AdminReconfigureStarted => "admin_reconfigure_started",
            AuditLogAction::AdminDomainReconfigured => "admin_domain_reconfigured",
            AuditLogAction::AdminBinariesUpdated => "admin_binaries_updated",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditLogExportFormat {
    Json,
    Csv,
}

#[derive(Debug, Clone, Deserialize, IntoParams, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogsQuery {
    pub user_id: Option<Uuid>,
    pub action: Option<AuditLogAction>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub page: Option<u64>,
    pub limit: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, IntoParams, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogsExportQuery {
    pub user_id: Option<Uuid>,
    pub action: Option<AuditLogAction>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub format: Option<AuditLogExportFormat>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogEntry {
    pub id: Uuid,
    pub user_id: Option<Uuid>,
    pub action: String,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub details: Option<serde_json::Value>,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogsResponse {
    pub items: Vec<AuditLogEntry>,
    pub total: u64,
    pub page: u64,
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditLogRedactResponse {
    pub user_id: Uuid,
    pub redacted_count: u64,
}
