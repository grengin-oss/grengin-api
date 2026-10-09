// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use std::{collections::BTreeSet, net::SocketAddr};

use axum::{
    extract::{ConnectInfo, MatchedPath, Request, State},
    http::{HeaderMap, Method, header},
    middleware::Next,
    response::Response,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    auth::claims::{Claiming, Claims},
    models::{
        ai_engines, branding, conversations, department_prompt_assignments, departments,
        embedding_configs, mcp_access_policies, mcp_connections, mcp_server_access_rules,
        mcp_servers, mcp_tools, messages, notifications, role_prompts, roles,
        user_prompt_preferences, user_role_assignments, users,
    },
    services::audit_logs::{AuditLogCreate, record_audit_log},
    state::SharedState,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapshotTarget {
    None,
    User,
    Department,
    DepartmentMembers,
    Role,
    UserRolesByUser,
    SsoProvider,
    BrandingSingleton,
    EmbeddingConfigSingleton,
    AiEngineByKey,
    Conversation,
    Message,
    Notification,
    RolePrompt,
    DepartmentPromptAssignment,
    McpServer,
    McpServerPolicies,
    McpServerRules,
    McpToolPolicies,
    McpConnectionByActorServer,
    UserPromptPreferenceByActor,
    McpToolsByServer,
}

impl SnapshotTarget {
    fn content(self) -> SnapshotContent {
        match self {
            SnapshotTarget::Conversation | SnapshotTarget::Message => SnapshotContent::Chat,
            SnapshotTarget::RolePrompt | SnapshotTarget::UserPromptPreferenceByActor => {
                SnapshotContent::Prompt
            }
            SnapshotTarget::None
            | SnapshotTarget::User
            | SnapshotTarget::Department
            | SnapshotTarget::DepartmentMembers
            | SnapshotTarget::Role
            | SnapshotTarget::UserRolesByUser
            | SnapshotTarget::SsoProvider
            | SnapshotTarget::BrandingSingleton
            | SnapshotTarget::EmbeddingConfigSingleton
            | SnapshotTarget::AiEngineByKey
            | SnapshotTarget::Notification
            | SnapshotTarget::DepartmentPromptAssignment
            | SnapshotTarget::McpServer
            | SnapshotTarget::McpServerPolicies
            | SnapshotTarget::McpServerRules
            | SnapshotTarget::McpToolPolicies
            | SnapshotTarget::McpConnectionByActorServer
            | SnapshotTarget::McpToolsByServer => SnapshotContent::Configuration,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SnapshotContent {
    Configuration,
    Chat,
    Prompt,
}

struct RedactedSnapshots {
    before: Option<Value>,
    after: Option<Value>,
    changed_fields: Vec<String>,
}

#[derive(Clone, Copy)]
struct AuditRouteAction {
    action: &'static str,
    resource_type: Option<&'static str>,
    resource_param: Option<&'static str>,
    snapshot_target: SnapshotTarget,
}

pub async fn audit_log_middleware(
    State(app_state): State<SharedState>,
    req: Request,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let request_path = req.uri().path().to_string();
    let query = req.uri().query().map(redact_query);
    let matched_path = req
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .map(ToOwned::to_owned);
    let route_path = matched_path.clone().unwrap_or_else(|| request_path.clone());
    if route_path == "/auth/refresh" {
        return next.run(req).await;
    }

    let route_action = resolve_audit_action(&method, &route_path);
    let should_fallback_log = route_action.is_none() && is_mutation_method(&method);
    if route_action.is_none() && !should_fallback_log {
        return next.run(req).await;
    }

    let user_id = extract_user_id(req.headers());
    let user_agent = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned);
    let peer_ip = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip());
    let ip_address = app_state
        .settings
        .server
        .trusted_proxies
        .client_ip(peer_ip, req.headers())
        .map(|ip| ip.to_string());

    let resource_id = route_action.and_then(|action| {
        action
            .resource_param
            .and_then(|param| extract_path_param(&route_path, &request_path, param))
    });

    let action_name = route_action
        .map(|a| a.action.to_string())
        .unwrap_or_else(|| generic_mutation_action_name(&method, &route_path));

    let resource_type = route_action
        .and_then(|a| a.resource_type)
        .map(ToOwned::to_owned);

    let before_snapshot = if let Some(action) = route_action {
        fetch_snapshot(
            &app_state,
            action.snapshot_target,
            resource_id.as_deref(),
            user_id,
        )
        .await
    } else {
        None
    };

    let response = next.run(req).await;
    if !response.status().is_success() {
        return response;
    }

    let status_code = response.status().as_u16();
    let db = app_state.database.clone();
    let app_state_for_after = app_state.clone();
    tokio::spawn(async move {
        let after_snapshot = if let Some(action) = route_action {
            fetch_snapshot(
                &app_state_for_after,
                action.snapshot_target,
                resource_id.as_deref(),
                user_id,
            )
            .await
        } else {
            None
        };
        let content = route_action
            .map(|action| action.snapshot_target.content())
            .unwrap_or(SnapshotContent::Configuration);
        let snapshots = redact_snapshots(before_snapshot, after_snapshot, content);
        let details = json!({
            "method": method.as_str(),
            "route": route_path,
            "path": request_path,
            "query": query,
            "status_code": status_code,
            "success": true,
            "before": snapshots.before,
            "after": snapshots.after,
            "changed_fields": snapshots.changed_fields,
        });
        if let Err(err) = record_audit_log(
            &db,
            AuditLogCreate {
                user_id,
                action: action_name,
                resource_type,
                resource_id,
                details: Some(details),
                ip_address,
                user_agent,
            },
        )
        .await
        {
            eprintln!("audit log insert error: {err}");
        }
    });

    response
}

async fn fetch_snapshot(
    app_state: &SharedState,
    target: SnapshotTarget,
    resource_id: Option<&str>,
    actor_user_id: Option<Uuid>,
) -> Option<Value> {
    let db = &app_state.database;
    match target {
        SnapshotTarget::None => None,
        SnapshotTarget::User => {
            let id = parse_uuid(resource_id)?;
            users::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::Department => {
            let id = parse_uuid(resource_id)?;
            departments::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::DepartmentMembers => {
            let id = parse_uuid(resource_id)?;
            let rows = users::Entity::find()
                .filter(users::Column::DepartmentId.eq(id))
                .all(db)
                .await
                .ok()?;
            Some(json!({
                "count": rows.len(),
                "items": rows
            }))
        }
        SnapshotTarget::Role => {
            let id = parse_uuid(resource_id)?;
            roles::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::UserRolesByUser => {
            let id = parse_uuid(resource_id)?;
            let rows = user_role_assignments::Entity::find()
                .filter(user_role_assignments::Column::UserId.eq(id))
                .all(db)
                .await
                .ok()?;
            Some(json!({
                "count": rows.len(),
                "items": rows
            }))
        }
        SnapshotTarget::SsoProvider => {
            let id = parse_uuid(resource_id)?;
            crate::models::sso_providers::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::BrandingSingleton => branding::Entity::find()
            .order_by_desc(branding::Column::UpdatedAt)
            .one(db)
            .await
            .ok()
            .flatten()
            .and_then(|model| serde_json::to_value(model).ok()),
        SnapshotTarget::EmbeddingConfigSingleton => embedding_configs::Entity::find()
            .order_by_desc(embedding_configs::Column::UpdatedAt)
            .one(db)
            .await
            .ok()
            .flatten()
            .and_then(|model| serde_json::to_value(model).ok()),
        SnapshotTarget::AiEngineByKey => {
            let key = resource_id?;
            ai_engines::Entity::find()
                .filter(ai_engines::Column::EngineKey.eq(key.to_string()))
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::Conversation => {
            let id = parse_uuid(resource_id)?;
            conversations::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::Message => {
            let id = parse_uuid(resource_id)?;
            messages::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::Notification => {
            let id = parse_uuid(resource_id)?;
            notifications::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::RolePrompt => {
            let id = parse_uuid(resource_id)?;
            role_prompts::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::DepartmentPromptAssignment => {
            let id = parse_uuid(resource_id)?;
            department_prompt_assignments::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::McpServer => {
            let id = parse_uuid(resource_id)?;
            mcp_servers::Entity::find_by_id(id)
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::McpServerPolicies => {
            let id = parse_uuid(resource_id)?;
            let rows = mcp_access_policies::Entity::find()
                .filter(mcp_access_policies::Column::ServerId.eq(id))
                .all(db)
                .await
                .ok()?;
            Some(json!({
                "count": rows.len(),
                "items": rows
            }))
        }
        SnapshotTarget::McpServerRules => {
            let id = parse_uuid(resource_id)?;
            let rows = mcp_server_access_rules::Entity::find()
                .filter(mcp_server_access_rules::Column::ServerId.eq(id))
                .all(db)
                .await
                .ok()?;
            Some(json!({
                "count": rows.len(),
                "items": rows
            }))
        }
        SnapshotTarget::McpToolPolicies => {
            let id = parse_uuid(resource_id)?;
            let rows = mcp_access_policies::Entity::find()
                .filter(mcp_access_policies::Column::ToolId.eq(id))
                .all(db)
                .await
                .ok()?;
            Some(json!({
                "count": rows.len(),
                "items": rows
            }))
        }
        SnapshotTarget::McpConnectionByActorServer => {
            let id = parse_uuid(resource_id)?;
            let user_id = actor_user_id?;
            mcp_connections::Entity::find()
                .filter(mcp_connections::Column::ServerId.eq(id))
                .filter(mcp_connections::Column::UserId.eq(user_id))
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::UserPromptPreferenceByActor => {
            let user_id = actor_user_id?;
            user_prompt_preferences::Entity::find()
                .filter(user_prompt_preferences::Column::UserId.eq(user_id))
                .one(db)
                .await
                .ok()
                .flatten()
                .and_then(|model| serde_json::to_value(model).ok())
        }
        SnapshotTarget::McpToolsByServer => {
            let id = parse_uuid(resource_id)?;
            let rows = mcp_tools::Entity::find()
                .filter(mcp_tools::Column::ServerId.eq(id))
                .all(db)
                .await
                .ok()?;
            Some(json!({
                "count": rows.len(),
                "items": rows
            }))
        }
    }
}

// Diff before redacting so a changed title or rotated key still shows up in changed_fields.
fn redact_snapshots(
    before: Option<Value>,
    after: Option<Value>,
    content: SnapshotContent,
) -> RedactedSnapshots {
    let changed_fields = compute_changed_fields(before.as_ref(), after.as_ref());
    RedactedSnapshots {
        before: before.map(|value| sanitize_value(value, content)),
        after: after.map(|value| sanitize_value(value, content)),
        changed_fields,
    }
}

fn compute_changed_fields(before: Option<&Value>, after: Option<&Value>) -> Vec<String> {
    match (before, after) {
        (None, None) => Vec::new(),
        (Some(b), Some(a)) if b == a => Vec::new(),
        (Some(Value::Object(before_obj)), Some(Value::Object(after_obj))) => {
            let mut keys = BTreeSet::new();
            for key in before_obj.keys() {
                keys.insert(key.to_string());
            }
            for key in after_obj.keys() {
                keys.insert(key.to_string());
            }
            keys.into_iter()
                .filter(|key| before_obj.get(key) != after_obj.get(key))
                .collect()
        }
        _ => vec!["_state".to_string()],
    }
}

fn sanitize_value(value: Value, content: SnapshotContent) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if is_sensitive_key(&key, &value, content) {
                    out.insert(key, Value::String("[REDACTED]".to_string()));
                } else {
                    out.insert(key, sanitize_value(value, content));
                }
            }
            Value::Object(out)
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| sanitize_value(value, content))
                .collect(),
        ),
        other => other,
    }
}

fn redact_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| {
            let key = pair.split_once('=').map_or(pair, |(key, _)| key);
            let normalized = normalize_key(key);
            // OAuth callbacks carry the authorization code or a signed proxy assertion here.
            if is_credential_key(&normalized) || matches!(normalized.as_str(), "code" | "assertion")
            {
                format!("{key}=[REDACTED]")
            } else {
                pair.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn is_sensitive_key(key: &str, value: &Value, content: SnapshotContent) -> bool {
    let key = normalize_key(key);
    let is_token_count = key.ends_with("tokens") && value.is_number();
    (!is_token_count && is_credential_key(&key))
        || (content == SnapshotContent::Chat && is_chat_content_key(&key))
        || (content == SnapshotContent::Prompt && is_prompt_content_key(&key))
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn is_credential_key(key: &str) -> bool {
    [
        "password",
        "secret",
        "token",
        "apikey",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|sensitive| key.contains(sensitive))
}

// Titles and file names are derived from or supplied with the chat, so they are content too.
fn is_chat_content_key(key: &str) -> bool {
    matches!(
        key,
        "messagecontent" | "toolscalls" | "toolsresults" | "title" | "name" | "base64"
    )
}

// System and custom prompts can carry personal or confidential text, so only their
// labels and flags are kept.
fn is_prompt_content_key(key: &str) -> bool {
    matches!(key, "prompttext" | "customprompttext" | "variables")
}

fn parse_uuid(value: Option<&str>) -> Option<Uuid> {
    value.and_then(|id| Uuid::parse_str(id).ok())
}

fn is_mutation_method(method: &Method) -> bool {
    matches!(method.as_str(), "POST" | "PUT" | "PATCH" | "DELETE")
}

fn generic_mutation_action_name(method: &Method, route: &str) -> String {
    let route_key = route
        .trim_matches('/')
        .replace('/', ".")
        .replace('{', "")
        .replace('}', "");
    format!(
        "mutation.{}.{}",
        method.as_str().to_ascii_lowercase(),
        route_key
    )
}

fn resolve_audit_action(method: &Method, route: &str) -> Option<AuditRouteAction> {
    match (method.as_str(), route) {
        ("GET", "/auth/{provider}/callback") | ("POST", "/auth/{provider}/callback") => {
            Some(AuditRouteAction {
                action: "login",
                resource_type: Some("auth"),
                resource_param: Some("provider"),
                snapshot_target: SnapshotTarget::None,
            })
        }
        ("GET", "/auth/azure/mobile/callback") | ("POST", "/auth/azure/mobile/callback") => {
            Some(AuditRouteAction {
                action: "login",
                resource_type: Some("auth"),
                resource_param: None,
                snapshot_target: SnapshotTarget::None,
            })
        }

        ("POST", "/chat/stream") => Some(AuditRouteAction {
            action: "conversation_created",
            resource_type: Some("conversation"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/chat/stream/{chat_id}") => Some(AuditRouteAction {
            action: "message_sent",
            resource_type: Some("conversation"),
            resource_param: Some("chat_id"),
            snapshot_target: SnapshotTarget::Conversation,
        }),
        ("POST", "/chat/stream/{message_id}/cancel") => Some(AuditRouteAction {
            action: "message_stream_cancelled",
            resource_type: Some("message"),
            resource_param: Some("message_id"),
            snapshot_target: SnapshotTarget::Message,
        }),
        ("PUT", "/chat/{chat_id}") => Some(AuditRouteAction {
            action: "conversation_updated",
            resource_type: Some("conversation"),
            resource_param: Some("chat_id"),
            snapshot_target: SnapshotTarget::Conversation,
        }),
        ("DELETE", "/chat/{chat_id}") => Some(AuditRouteAction {
            action: "conversation_deleted",
            resource_type: Some("conversation"),
            resource_param: Some("chat_id"),
            snapshot_target: SnapshotTarget::Conversation,
        }),
        ("PATCH", "/chat/{chat_id}/message/{message_id}/stream") => Some(AuditRouteAction {
            action: "message_updated",
            resource_type: Some("message"),
            resource_param: Some("message_id"),
            snapshot_target: SnapshotTarget::Message,
        }),
        ("DELETE", "/chat/{chat_id}/message/{message_id}") => Some(AuditRouteAction {
            action: "message_deleted",
            resource_type: Some("message"),
            resource_param: Some("message_id"),
            snapshot_target: SnapshotTarget::Message,
        }),

        ("DELETE", "/artifacts/{id}") => Some(AuditRouteAction {
            action: "artifact_deleted",
            resource_type: Some("artifact"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/files") => Some(AuditRouteAction {
            action: "file_uploaded",
            resource_type: Some("file"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),

        ("POST", "/me/notifications/{notification_id}/read") => Some(AuditRouteAction {
            action: "notification_marked_read",
            resource_type: Some("notification"),
            resource_param: Some("notification_id"),
            snapshot_target: SnapshotTarget::Notification,
        }),
        ("PUT", "/me/system-prompt") => Some(AuditRouteAction {
            action: "user_system_prompt_set",
            resource_type: Some("user_prompt_preference"),
            resource_param: None,
            snapshot_target: SnapshotTarget::UserPromptPreferenceByActor,
        }),
        ("DELETE", "/me/system-prompt") => Some(AuditRouteAction {
            action: "user_system_prompt_reset",
            resource_type: Some("user_prompt_preference"),
            resource_param: None,
            snapshot_target: SnapshotTarget::UserPromptPreferenceByActor,
        }),
        ("POST", "/me/system-prompt/feedback") => Some(AuditRouteAction {
            action: "user_prompt_feedback_submitted",
            resource_type: Some("prompt_feedback"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),

        ("POST", "/mcp/connections/{server_id}/authorize") => Some(AuditRouteAction {
            action: "mcp_connection_authorized",
            resource_type: Some("mcp_connection"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpConnectionByActorServer,
        }),
        ("POST", "/mcp/connections/{server_id}/disconnect") => Some(AuditRouteAction {
            action: "mcp_connection_disconnected",
            resource_type: Some("mcp_connection"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpConnectionByActorServer,
        }),

        ("POST", "/admin/users") => Some(AuditRouteAction {
            action: "admin_user_created",
            resource_type: Some("user"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/admin/users/{user_id}") => Some(AuditRouteAction {
            action: "admin_user_updated",
            resource_type: Some("user"),
            resource_param: Some("user_id"),
            snapshot_target: SnapshotTarget::User,
        }),
        ("PATCH", "/admin/users/{user_id}/status") => Some(AuditRouteAction {
            action: "admin_user_status_updated",
            resource_type: Some("user"),
            resource_param: Some("user_id"),
            snapshot_target: SnapshotTarget::User,
        }),
        ("DELETE", "/admin/users/{user_id}") => Some(AuditRouteAction {
            action: "admin_user_deleted",
            resource_type: Some("user"),
            resource_param: Some("user_id"),
            snapshot_target: SnapshotTarget::User,
        }),
        ("PUT", "/admin/branding") => Some(AuditRouteAction {
            action: "admin_branding_updated",
            resource_type: Some("branding"),
            resource_param: None,
            snapshot_target: SnapshotTarget::BrandingSingleton,
        }),
        ("PUT", "/admin/embedding-config") => Some(AuditRouteAction {
            action: "admin_embedding_config_updated",
            resource_type: Some("embedding_config"),
            resource_param: None,
            snapshot_target: SnapshotTarget::EmbeddingConfigSingleton,
        }),
        ("PUT", "/admin/ai-engines/{engine_key}") => Some(AuditRouteAction {
            action: "admin_ai_engine_updated",
            resource_type: Some("ai_engine"),
            resource_param: Some("engine_key"),
            snapshot_target: SnapshotTarget::AiEngineByKey,
        }),
        ("DELETE", "/admin/ai-engines/{engine_key}/api-key") => Some(AuditRouteAction {
            action: "admin_ai_engine_api_key_removed_or_rotated",
            resource_type: Some("ai_engine"),
            resource_param: Some("engine_key"),
            snapshot_target: SnapshotTarget::AiEngineByKey,
        }),
        ("PUT", "/admin/sso-providers/{provider_id}") => Some(AuditRouteAction {
            action: "admin_sso_provider_updated",
            resource_type: Some("sso_provider"),
            resource_param: Some("provider_id"),
            snapshot_target: SnapshotTarget::SsoProvider,
        }),
        ("DELETE", "/admin/sso-providers/{provider_id}") => Some(AuditRouteAction {
            action: "admin_sso_provider_deleted",
            resource_type: Some("sso_provider"),
            resource_param: Some("provider_id"),
            snapshot_target: SnapshotTarget::SsoProvider,
        }),
        ("POST", "/admin/departments") => Some(AuditRouteAction {
            action: "admin_department_created",
            resource_type: Some("department"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/admin/departments/{department_id}") => Some(AuditRouteAction {
            action: "admin_department_updated",
            resource_type: Some("department"),
            resource_param: Some("department_id"),
            snapshot_target: SnapshotTarget::Department,
        }),
        ("DELETE", "/admin/departments/{department_id}") => Some(AuditRouteAction {
            action: "admin_department_deleted",
            resource_type: Some("department"),
            resource_param: Some("department_id"),
            snapshot_target: SnapshotTarget::Department,
        }),
        ("POST", "/admin/departments/{department_id}/move") => Some(AuditRouteAction {
            action: "admin_department_moved",
            resource_type: Some("department"),
            resource_param: Some("department_id"),
            snapshot_target: SnapshotTarget::Department,
        }),
        ("POST", "/admin/departments/{department_id}/members") => Some(AuditRouteAction {
            action: "admin_department_members_added",
            resource_type: Some("department_members"),
            resource_param: Some("department_id"),
            snapshot_target: SnapshotTarget::DepartmentMembers,
        }),
        ("DELETE", "/admin/departments/{department_id}/members") => Some(AuditRouteAction {
            action: "admin_department_members_removed",
            resource_type: Some("department_members"),
            resource_param: Some("department_id"),
            snapshot_target: SnapshotTarget::DepartmentMembers,
        }),
        ("POST", "/admin/roles") => Some(AuditRouteAction {
            action: "admin_role_created",
            resource_type: Some("role"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/admin/roles/{role_id}") => Some(AuditRouteAction {
            action: "admin_role_updated",
            resource_type: Some("role"),
            resource_param: Some("role_id"),
            snapshot_target: SnapshotTarget::Role,
        }),
        ("DELETE", "/admin/roles/{role_id}") => Some(AuditRouteAction {
            action: "admin_role_deleted",
            resource_type: Some("role"),
            resource_param: Some("role_id"),
            snapshot_target: SnapshotTarget::Role,
        }),
        ("POST", "/admin/users/{user_id}/roles") => Some(AuditRouteAction {
            action: "admin_role_assigned_to_user",
            resource_type: Some("user_roles"),
            resource_param: Some("user_id"),
            snapshot_target: SnapshotTarget::UserRolesByUser,
        }),
        ("DELETE", "/admin/users/{user_id}/roles/{assignment_id}") => Some(AuditRouteAction {
            action: "admin_role_removed_from_user",
            resource_type: Some("user_roles"),
            resource_param: Some("user_id"),
            snapshot_target: SnapshotTarget::UserRolesByUser,
        }),

        ("POST", "/admin/mcp-servers") => Some(AuditRouteAction {
            action: "admin_mcp_server_created",
            resource_type: Some("mcp_server"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/admin/mcp-servers/{server_id}") => Some(AuditRouteAction {
            action: "admin_mcp_server_updated",
            resource_type: Some("mcp_server"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpServer,
        }),
        ("DELETE", "/admin/mcp-servers/{server_id}") => Some(AuditRouteAction {
            action: "admin_mcp_server_deleted",
            resource_type: Some("mcp_server"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpServer,
        }),
        ("POST", "/admin/mcp-servers/{server_id}/sync-tools") => Some(AuditRouteAction {
            action: "admin_mcp_server_tools_synced",
            resource_type: Some("mcp_tools"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpToolsByServer,
        }),
        ("PUT", "/admin/mcp-servers/{server_id}/access") => Some(AuditRouteAction {
            action: "admin_mcp_server_access_updated",
            resource_type: Some("mcp_server_access"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpServerPolicies,
        }),
        ("PUT", "/admin/mcp-servers/{server_id}/access/default") => Some(AuditRouteAction {
            action: "admin_mcp_server_default_access_updated",
            resource_type: Some("mcp_server"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpServer,
        }),
        ("POST", "/admin/mcp-servers/{server_id}/access/rules") => Some(AuditRouteAction {
            action: "admin_mcp_access_rule_created",
            resource_type: Some("mcp_access_rule"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpServerRules,
        }),
        ("DELETE", "/admin/mcp-servers/{server_id}/access/rules/{rule_id}") => {
            Some(AuditRouteAction {
                action: "admin_mcp_access_rule_deleted",
                resource_type: Some("mcp_access_rule"),
                resource_param: Some("server_id"),
                snapshot_target: SnapshotTarget::McpServerRules,
            })
        }
        ("PUT", "/admin/mcp-servers/{server_id}/tools/access") => Some(AuditRouteAction {
            action: "admin_mcp_server_tools_access_updated",
            resource_type: Some("mcp_server_access"),
            resource_param: Some("server_id"),
            snapshot_target: SnapshotTarget::McpServerPolicies,
        }),
        ("PUT", "/admin/mcp-tools/{tool_id}/access") => Some(AuditRouteAction {
            action: "admin_mcp_tool_access_updated",
            resource_type: Some("mcp_tool_access"),
            resource_param: Some("tool_id"),
            snapshot_target: SnapshotTarget::McpToolPolicies,
        }),

        ("POST", "/admin/role-prompts") => Some(AuditRouteAction {
            action: "admin_role_prompt_created",
            resource_type: Some("role_prompt"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/admin/role-prompts/{prompt_id}") => Some(AuditRouteAction {
            action: "admin_role_prompt_updated",
            resource_type: Some("role_prompt"),
            resource_param: Some("prompt_id"),
            snapshot_target: SnapshotTarget::RolePrompt,
        }),
        ("DELETE", "/admin/role-prompts/{prompt_id}") => Some(AuditRouteAction {
            action: "admin_role_prompt_deleted",
            resource_type: Some("role_prompt"),
            resource_param: Some("prompt_id"),
            snapshot_target: SnapshotTarget::RolePrompt,
        }),
        ("POST", "/admin/department-prompts") => Some(AuditRouteAction {
            action: "admin_department_prompt_assigned",
            resource_type: Some("department_prompt_assignment"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/admin/department-prompts/{assignment_id}") => Some(AuditRouteAction {
            action: "admin_department_prompt_updated",
            resource_type: Some("department_prompt_assignment"),
            resource_param: Some("assignment_id"),
            snapshot_target: SnapshotTarget::DepartmentPromptAssignment,
        }),
        ("DELETE", "/admin/department-prompts/{assignment_id}") => Some(AuditRouteAction {
            action: "admin_department_prompt_deleted",
            resource_type: Some("department_prompt_assignment"),
            resource_param: Some("assignment_id"),
            snapshot_target: SnapshotTarget::DepartmentPromptAssignment,
        }),

        ("POST", "/admin/audit-logs/redact/{user_id}") => Some(AuditRouteAction {
            action: "admin_audit_logs_redacted_for_user",
            resource_type: Some("audit_logs"),
            resource_param: Some("user_id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/admin/reconfigure/domain") => Some(AuditRouteAction {
            action: "admin_domain_reconfigured",
            resource_type: Some("system_configuration"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/admin/reconfigure/binaries") => Some(AuditRouteAction {
            action: "admin_binaries_updated",
            resource_type: Some("system_configuration"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/api/admin/reconfigure/domain") => Some(AuditRouteAction {
            action: "admin_domain_reconfigured",
            resource_type: Some("system_configuration"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/api/admin/reconfigure/binaries") => Some(AuditRouteAction {
            action: "admin_binaries_updated",
            resource_type: Some("system_configuration"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/projects") => Some(AuditRouteAction {
            action: "project_created",
            resource_type: Some("project"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PATCH", "/projects/{id}") => Some(AuditRouteAction {
            action: "project_updated",
            resource_type: Some("project"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("DELETE", "/projects/{id}") => Some(AuditRouteAction {
            action: "project_deleted",
            resource_type: Some("project"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/projects/{id}/instructions") => Some(AuditRouteAction {
            action: "project_instructions_updated",
            resource_type: Some("project"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/projects/{id}/members") => Some(AuditRouteAction {
            action: "project_member_added",
            resource_type: Some("project_member"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("DELETE", "/projects/{id}/members/{user_id}") => Some(AuditRouteAction {
            action: "project_member_removed",
            resource_type: Some("project_member"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/projects/{id}/sources") => Some(AuditRouteAction {
            action: "project_source_added",
            resource_type: Some("project_source"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("DELETE", "/projects/{id}/sources/{source_id}") => Some(AuditRouteAction {
            action: "project_source_removed",
            resource_type: Some("project_source"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/projects/{id}/artifacts") => Some(AuditRouteAction {
            action: "project_artifact_added",
            resource_type: Some("project_source"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/projects/{id}/share") => Some(AuditRouteAction {
            action: "project_shared",
            resource_type: Some("project"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/conversations/{conversation_id}/projects") => Some(AuditRouteAction {
            action: "project_linked_to_conversation",
            resource_type: Some("conversation_project"),
            resource_param: Some("conversation_id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("DELETE", "/conversations/{conversation_id}/projects/{project_id}") => {
            Some(AuditRouteAction {
                action: "project_unlinked_from_conversation",
                resource_type: Some("conversation_project"),
                resource_param: Some("conversation_id"),
                snapshot_target: SnapshotTarget::None,
            })
        }
        ("POST", "/me/skills") => Some(AuditRouteAction {
            action: "user_skill_created",
            resource_type: Some("skill"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/me/skills/{id}") => Some(AuditRouteAction {
            action: "user_skill_updated",
            resource_type: Some("skill"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("DELETE", "/me/skills/{id}") => Some(AuditRouteAction {
            action: "user_skill_deleted",
            resource_type: Some("skill"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/admin/skills") => Some(AuditRouteAction {
            action: "skill_created",
            resource_type: Some("skill"),
            resource_param: None,
            snapshot_target: SnapshotTarget::None,
        }),
        ("PUT", "/admin/skills/{id}") => Some(AuditRouteAction {
            action: "skill_updated",
            resource_type: Some("skill"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("DELETE", "/admin/skills/{id}") => Some(AuditRouteAction {
            action: "skill_deleted",
            resource_type: Some("skill"),
            resource_param: Some("id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("POST", "/conversations/{conversation_id}/skills") => Some(AuditRouteAction {
            action: "skill_linked_to_conversation",
            resource_type: Some("conversation_skill"),
            resource_param: Some("conversation_id"),
            snapshot_target: SnapshotTarget::None,
        }),
        ("DELETE", "/conversations/{conversation_id}/skills/{skill_id}") => {
            Some(AuditRouteAction {
                action: "skill_unlinked_from_conversation",
                resource_type: Some("conversation_skill"),
                resource_param: Some("conversation_id"),
                snapshot_target: SnapshotTarget::None,
            })
        }
        _ => None,
    }
}

fn extract_path_param(route: &str, path: &str, param_name: &str) -> Option<String> {
    let route_parts: Vec<&str> = route.trim_matches('/').split('/').collect();
    let path_parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if route_parts.len() != path_parts.len() {
        return None;
    }
    for (route_part, path_part) in route_parts.iter().zip(path_parts.iter()) {
        let Some(name) = route_part
            .strip_prefix('{')
            .and_then(|s| s.strip_suffix('}'))
        else {
            continue;
        };
        if name == param_name {
            return Some((*path_part).to_string());
        }
    }
    None
}

fn extract_user_id(headers: &HeaderMap) -> Option<Uuid> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))?;
    Claims::from_token_string(token)
        .ok()
        .map(|claims| claims.user_id)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{SnapshotContent, SnapshotTarget, redact_query, redact_snapshots, sanitize_value};

    const REDACTED: &str = "[REDACTED]";

    fn message_snapshot() -> Value {
        json!({
            "id": "5f0c3c2e-1111-4222-8333-944445555666",
            "role": "assistant",
            "messageContent": "Here is the patient summary",
            "modelName": "gpt-4o",
            "requestTokens": 120,
            "responseTokens": 80,
            "totalTokens": 200,
            "toolsCalls": [{"name": "search_records", "arguments": "{\"patient\":\"Jane\"}"}],
            "toolsResults": [{"output": "Jane Doe, DOB 1980-01-01"}],
            "metadata": {
                "webSearch": false,
                "cachedInputTokens": 10,
                "files": [{"id": "f1", "name": "jane_doe_labs.pdf", "type": "application/pdf", "base64": "JVBERi0x"}],
                "artifacts": [{"id": "a1", "title": "Jane's treatment plan", "content_type": "text/markdown"}]
            }
        })
    }

    #[test]
    fn message_snapshot_redacts_text_tool_calls_and_tool_results() {
        let out = sanitize_value(message_snapshot(), SnapshotContent::Chat);
        assert_eq!(out["messageContent"], REDACTED);
        assert_eq!(out["toolsCalls"], REDACTED);
        assert_eq!(out["toolsResults"], REDACTED);
    }

    #[test]
    fn message_snapshot_redacts_attachment_names_inline_data_and_artifact_titles() {
        let out = sanitize_value(message_snapshot(), SnapshotContent::Chat);
        assert_eq!(out["metadata"]["files"][0]["name"], REDACTED);
        assert_eq!(out["metadata"]["files"][0]["base64"], REDACTED);
        assert_eq!(out["metadata"]["files"][0]["type"], "application/pdf");
        assert_eq!(out["metadata"]["artifacts"][0]["title"], REDACTED);
        assert_eq!(
            out["metadata"]["artifacts"][0]["content_type"],
            "text/markdown"
        );
    }

    #[test]
    fn message_snapshot_keeps_token_counts_and_model_metadata() {
        let out = sanitize_value(message_snapshot(), SnapshotContent::Chat);
        assert_eq!(out["requestTokens"], 120);
        assert_eq!(out["responseTokens"], 80);
        assert_eq!(out["totalTokens"], 200);
        assert_eq!(out["metadata"]["cachedInputTokens"], 10);
        assert_eq!(out["modelName"], "gpt-4o");
        assert_eq!(out["role"], "assistant");
    }

    #[test]
    fn conversation_snapshot_redacts_title_but_keeps_usage() {
        let snapshot = json!({
            "title": "Divorce settlement questions",
            "modelName": "gpt-4o",
            "totalTokens": 4200,
            "messageCount": 6,
            "metadata": {"titleGenerationUsage": {"model": "gpt-4o", "inputTokens": 30, "outputTokens": 8}}
        });
        let out = sanitize_value(snapshot, SnapshotContent::Chat);
        assert_eq!(out["title"], REDACTED);
        assert_eq!(out["totalTokens"], 4200);
        assert_eq!(out["messageCount"], 6);
        assert_eq!(out["metadata"]["titleGenerationUsage"]["inputTokens"], 30);
        assert_eq!(out["metadata"]["titleGenerationUsage"]["model"], "gpt-4o");
    }

    #[test]
    fn configuration_snapshot_keeps_titles_and_names() {
        let notification = json!({"title": "Budget 80% used", "body": "Engineering is at 80%"});
        let department = json!({"name": "Engineering", "description": "Platform team"});
        assert_eq!(
            sanitize_value(notification.clone(), SnapshotContent::Configuration),
            notification
        );
        assert_eq!(
            sanitize_value(department.clone(), SnapshotContent::Configuration),
            department
        );
    }

    #[test]
    fn credentials_are_redacted_in_every_snapshot() {
        let snapshot = json!({
            "password": "hash",
            "mfaSecret": "totp",
            "clientSecret": "s3cret",
            "apiKey": "sk-live",
            "accessToken": "at",
            "refreshToken": "rt",
            "cookie": "session=1",
            "connectionConfig": {
                "headers": {"Authorization": "Bearer x", "X-Api-Key": "k"},
                "env": {"GITHUB_TOKEN": "ghp"}
            }
        });
        for content in [
            SnapshotContent::Configuration,
            SnapshotContent::Chat,
            SnapshotContent::Prompt,
        ] {
            let out = sanitize_value(snapshot.clone(), content);
            for key in [
                "password",
                "mfaSecret",
                "clientSecret",
                "apiKey",
                "accessToken",
                "refreshToken",
                "cookie",
            ] {
                assert_eq!(out[key], REDACTED, "{key} must be redacted");
            }
            let config = &out["connectionConfig"];
            assert_eq!(config["headers"]["Authorization"], REDACTED);
            assert_eq!(config["headers"]["X-Api-Key"], REDACTED);
            assert_eq!(config["env"]["GITHUB_TOKEN"], REDACTED);
        }
    }

    #[test]
    fn token_named_text_values_stay_redacted() {
        let snapshot = json!({"bearerTokens": "abc.def", "maxTokens": 4096});
        let out = sanitize_value(snapshot, SnapshotContent::Configuration);
        assert_eq!(out["bearerTokens"], REDACTED);
        assert_eq!(out["maxTokens"], 4096);
    }

    #[test]
    fn changed_fields_report_redacted_changes_without_their_values() {
        let before = json!({"title": "Old private title", "apiKey": "old", "pinned": false});
        let after = json!({"title": "New private title", "apiKey": "new", "pinned": false});
        let out = redact_snapshots(Some(before), Some(after), SnapshotContent::Chat);
        assert_eq!(out.changed_fields, vec!["apiKey", "title"]);
        let before = out.before.expect("before snapshot");
        let after = out.after.expect("after snapshot");
        assert_eq!(before["title"], REDACTED);
        assert_eq!(after["title"], REDACTED);
        assert_eq!(after["apiKey"], REDACTED);
        assert_eq!(after["pinned"], false);
    }

    #[test]
    fn oauth_callback_query_hides_code_and_assertion() {
        assert_eq!(
            redact_query("code=4%2F0Ab&state=xyz&assertion=eyJhbGci.eyJzdWIi.sig"),
            "code=[REDACTED]&state=xyz&assertion=[REDACTED]"
        );
    }

    #[test]
    fn credential_named_query_params_are_redacted_and_others_kept() {
        assert_eq!(
            redact_query("access_token=abc&page=2&archived&api-key=k"),
            "access_token=[REDACTED]&page=2&archived&api-key=[REDACTED]"
        );
    }

    #[test]
    fn only_conversation_and_message_snapshots_hold_chat_content() {
        assert_eq!(
            SnapshotTarget::Conversation.content(),
            SnapshotContent::Chat
        );
        assert_eq!(SnapshotTarget::Message.content(), SnapshotContent::Chat);
        assert_eq!(
            SnapshotTarget::Notification.content(),
            SnapshotContent::Configuration
        );
        assert_eq!(
            SnapshotTarget::DepartmentPromptAssignment.content(),
            SnapshotContent::Configuration
        );
    }

    #[test]
    fn role_and_user_prompt_snapshots_hold_prompt_content() {
        assert_eq!(
            SnapshotTarget::RolePrompt.content(),
            SnapshotContent::Prompt
        );
        assert_eq!(
            SnapshotTarget::UserPromptPreferenceByActor.content(),
            SnapshotContent::Prompt
        );
    }

    #[test]
    fn role_prompt_snapshot_redacts_prompt_text_and_variables_but_keeps_labels() {
        let snapshot = json!({
            "id": "4f1c",
            "name": "HR assistant",
            "promptText": "Employee Jane Doe is on medical leave; never mention it.",
            "variables": {"employee": "Jane Doe"},
            "isSystem": false,
            "usageCount": 3
        });
        let out = sanitize_value(snapshot, SnapshotContent::Prompt);
        assert_eq!(out["promptText"], REDACTED);
        assert_eq!(out["variables"], REDACTED);
        assert_eq!(out["name"], "HR assistant");
        assert_eq!(out["isSystem"], false);
        assert_eq!(out["usageCount"], 3);
    }

    #[test]
    fn user_prompt_preference_redacts_custom_text_but_keeps_selection() {
        let snapshot = json!({
            "promptId": "9b2e",
            "customPromptText": "I am recovering from surgery, keep answers short.",
            "isActive": true
        });
        let out = sanitize_value(snapshot, SnapshotContent::Prompt);
        assert_eq!(out["customPromptText"], REDACTED);
        assert_eq!(out["promptId"], "9b2e");
        assert_eq!(out["isActive"], true);
    }

    #[test]
    fn prompt_text_changes_are_reported_without_their_values() {
        let before = json!({"name": "Support", "promptText": "Old instructions"});
        let after = json!({"name": "Support", "promptText": "New instructions"});
        let out = redact_snapshots(Some(before), Some(after), SnapshotContent::Prompt);
        assert_eq!(out.changed_fields, vec!["promptText"]);
        assert_eq!(out.after.expect("after snapshot")["promptText"], REDACTED);
    }

    #[test]
    fn configuration_snapshots_keep_fields_named_like_prompt_text() {
        let out = sanitize_value(
            json!({"variables": {"region": "eu"}}),
            SnapshotContent::Configuration,
        );
        assert_eq!(out["variables"]["region"], "eu");
    }
}
