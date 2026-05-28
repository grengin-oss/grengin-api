use axum::{Json, extract::State, http::HeaderMap};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Command as StdCommand, time::Duration};
use tokio::process::Command;
use utoipa::ToSchema;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::{PERMISSION_ROLES_MANAGE, ROLE_SUPER_ADMIN},
    },
    services::authorization::{AuthorizationService, PermissionScopeMode},
    state::SharedState,
};

const DEFAULT_INSTALLER_INTERNAL_URL: &str = "http://127.0.0.1:3000";
const RECONFIGURE_TOKEN_HEADER: &str = "x-grengin-reconfigure-token";
const DEFAULT_DOMAIN_RECONFIGURE_SCRIPT: &str = "/opt/grengin/scripts/reconfigure-domain.sh";
const DEFAULT_DOMAIN_RECONFIGURE_USE_SUDO: bool = true;
const DEFAULT_BINARY_UPDATE_SCRIPT: &str = "/opt/grengin/scripts/update-app-binaries.sh";
const DEFAULT_BINARY_UPDATE_USE_SUDO: bool = true;
const DEFAULT_RELEASE_BASE_URL: &str = "https://releases.grengin.io";

#[derive(Debug, Deserialize, Default, ToSchema)]
pub struct ReconfigureStartRequest {
    pub preserve_database: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DomainReconfigureRequest {
    pub domain: String,
    /// `letsencrypt`, `selfsigned`, or `none`
    pub ssl_mode: Option<String>,
    pub email: Option<String>,
    pub self_signed_days: Option<u16>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BinariesUpdateRequest {
    /// Release version path segment (`latest`, `v1.2.3`, etc.)
    pub version: Option<String>,
    /// Release host root (for example `https://releases.example.com`)
    pub release_base_url: Option<String>,
    /// `x86_64` or `aarch64` (default: auto-detect host architecture in script)
    pub arch: Option<String>,
    pub update_installer: Option<bool>,
    pub update_api: Option<bool>,
    pub update_webapp: Option<bool>,
    pub verify_checksums: Option<bool>,
    pub api_service_name: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct InstallerReconfigureResponse {
    success: bool,
    message: String,
    next_step: Option<String>,
    detected_public_url: Option<String>,
    preserve_database: bool,
    #[serde(default)]
    warnings: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReconfigureStartResponse {
    pub success: bool,
    pub message: String,
    pub next_step: Option<String>,
    pub detected_public_url: Option<String>,
    pub preserve_database: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct DomainReconfigureResponse {
    pub success: bool,
    pub message: String,
    pub domain: String,
    pub ssl_mode: String,
    pub redirect_url: String,
    pub script_path: String,
    #[serde(default)]
    pub output: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BinariesUpdateResponse {
    pub success: bool,
    pub message: String,
    pub version: String,
    pub release_base_url: String,
    pub arch: String,
    pub update_installer: bool,
    pub update_api: bool,
    pub update_webapp: bool,
    pub verify_checksums: bool,
    pub script_path: String,
    #[serde(default)]
    pub output: Vec<String>,
}

fn installer_internal_url() -> String {
    std::env::var("INSTALLER_INTERNAL_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_INSTALLER_INTERNAL_URL.to_string())
}

fn release_base_url(default: &str, provided: Option<&str>) -> String {
    if let Some(value) = provided
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.trim_end_matches('/').to_string())
    {
        return value;
    }

    std::env::var("GRENGIN_RELEASE_BASE_URL")
        .or_else(|_| std::env::var("RELEASE_BASE_URL"))
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn expected_reconfigure_token() -> Option<String> {
    std::env::var("INSTALLER_RECONFIGURE_TOKEN")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn provided_reconfigure_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(RECONFIGURE_TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_bool_env(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_ascii_lowercase())
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(default)
}

fn domain_reconfigure_script_path() -> String {
    if let Some(explicit) = std::env::var("DOMAIN_RECONFIGURE_SCRIPT")
        .or_else(|_| std::env::var("GRENGIN_DOMAIN_RECONFIGURE_SCRIPT"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return explicit;
    }

    for fallback_local in [
        "deploy/scripts/reconfigure-domain.sh",
        "../deploy/scripts/reconfigure-domain.sh",
    ] {
        if Path::new(fallback_local).exists() {
            return fallback_local.to_string();
        }
    }

    DEFAULT_DOMAIN_RECONFIGURE_SCRIPT.to_string()
}

fn domain_reconfigure_use_sudo() -> bool {
    parse_bool_env(
        "DOMAIN_RECONFIGURE_USE_SUDO",
        DEFAULT_DOMAIN_RECONFIGURE_USE_SUDO,
    )
}

fn binaries_update_script_path() -> String {
    if let Some(explicit) = std::env::var("BINARY_UPDATE_SCRIPT")
        .or_else(|_| std::env::var("GRENGIN_BINARY_UPDATE_SCRIPT"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return explicit;
    }

    for fallback_local in [
        "deploy/scripts/update-app-binaries.sh",
        "../deploy/scripts/update-app-binaries.sh",
    ] {
        if Path::new(fallback_local).exists() {
            return fallback_local.to_string();
        }
    }

    DEFAULT_BINARY_UPDATE_SCRIPT.to_string()
}

fn binaries_update_use_sudo() -> bool {
    parse_bool_env("BINARY_UPDATE_USE_SUDO", DEFAULT_BINARY_UPDATE_USE_SUDO)
}

fn command_exists(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).is_file();
    }

    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(command).is_file()))
        .unwrap_or(false)
}

fn running_as_root() -> bool {
    StdCommand::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
        .unwrap_or(false)
}

fn resolve_script_sudo_usage(request_use_sudo: bool, env_prefix: &str) -> Result<bool, String> {
    if !request_use_sudo {
        return Ok(false);
    }

    if running_as_root() {
        return Ok(false);
    }

    if command_exists("sudo") {
        return Ok(true);
    }

    Err(format!(
        "sudo is required for {env_prefix}_USE_SUDO=true, but sudo is not available and the API process is not running as root"
    ))
}

fn should_retry_without_sudo(stderr: &[u8]) -> bool {
    let lowered = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    lowered.contains("no new privileges")
        || lowered.contains("setresuid")
        || lowered.contains("setreuid")
        || lowered.contains("operation not permitted")
}

async fn run_with_timeout(
    mut cmd: Command,
    timeout_seconds: u64,
    context: &str,
    script_path: &str,
) -> Result<std::process::Output, AuthError> {
    tokio::time::timeout(Duration::from_secs(timeout_seconds), cmd.output())
        .await
        .map_err(|_| {
            eprintln!("{context} timed out: script={script_path}");
            AuthError::ServiceTemporarilyUnavailable
        })?
        .map_err(|e| {
            eprintln!("{context} execution failed: script={script_path}; err={e}");
            AuthError::ServiceTemporarilyUnavailable
        })
}

async fn run_script_command(
    script_path: &str,
    args: &[String],
    use_sudo: bool,
    timeout_seconds: u64,
    context: &str,
) -> Result<std::process::Output, AuthError> {
    let build_command = |with_sudo: bool| {
        let mut cmd = if with_sudo {
            let mut command = Command::new("sudo");
            command.arg("-n").arg(script_path);
            command
        } else {
            Command::new(script_path)
        };

        for arg in args {
            cmd.arg(arg);
        }
        cmd.kill_on_drop(true);
        cmd
    };

    let output = run_with_timeout(
        build_command(use_sudo),
        timeout_seconds,
        context,
        script_path,
    )
    .await?;

    if use_sudo && !output.status.success() && should_retry_without_sudo(&output.stderr) {
        eprintln!(
            "{context} failed with sudo no-new-privileges restriction; retrying without sudo: script={script_path}"
        );
        return run_with_timeout(
            build_command(false),
            timeout_seconds,
            context,
            script_path,
        )
        .await;
    }

    Ok(output)
}

fn is_valid_domain(domain: &str) -> bool {
    if domain.is_empty()
        || domain.starts_with('.')
        || domain.ends_with('.')
        || !domain.contains('.')
    {
        return false;
    }
    domain
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
}

fn normalized_ssl_mode(mode: Option<&str>) -> Option<String> {
    let normalized = mode
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "letsencrypt".to_string());
    if matches!(normalized.as_str(), "letsencrypt" | "selfsigned" | "none") {
        Some(normalized)
    } else {
        None
    }
}

fn normalized_release_version(value: Option<&str>) -> Option<String> {
    let version = value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("latest");

    if version.contains('/')
        || version
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
    {
        return None;
    }

    Some(version.to_string())
}

fn normalized_arch(value: Option<&str>) -> Option<String> {
    let normalized = value
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty());

    match normalized.as_deref() {
        None => Some("auto".to_string()),
        Some("x86_64") => Some("x86_64".to_string()),
        Some("aarch64") => Some("aarch64".to_string()),
        _ => None,
    }
}

fn summarize_output(stdout: &[u8], stderr: &[u8]) -> Vec<String> {
    let mut lines = Vec::new();
    let stdout_text = String::from_utf8_lossy(stdout);
    let stderr_text = String::from_utf8_lossy(stderr);

    for line in stdout_text.lines().chain(stderr_text.lines()) {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            lines.push(trimmed.to_string());
        }
        if lines.len() >= 25 {
            break;
        }
    }

    lines
}

async fn ensure_super_admin(
    claims: &Claims,
    app_state: &SharedState,
    headers: &HeaderMap,
) -> Result<(), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_ROLES_MANAGE,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

    let is_super_admin = authz
        .user_has_role_name(claims.user_id, ROLE_SUPER_ADMIN)
        .await?;
    if !is_super_admin {
        return Err(AuthError::PermissionDenied);
    }

    if let Some(expected_token) = expected_reconfigure_token() {
        let Some(provided_token) = provided_reconfigure_token(headers) else {
            return Err(AuthError::PermissionDenied);
        };
        if provided_token != expected_token {
            return Err(AuthError::PermissionDenied);
        }
    }

    Ok(())
}

#[utoipa::path(
    post,
    path = "/admin/reconfigure/start",
    tag = "admin",
    request_body = ReconfigureStartRequest,
    responses(
       (status = 200, body = ReconfigureStartResponse),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 403, content_type = "application/json", body = Error, description = "Forbidden - Super Admin role required"),
       (status = 503, content_type = "application/json", body = Error, description = "Service unavailable"),
    )
)]
pub async fn start_reconfigure(
    claims: Claims,
    State(app_state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<ReconfigureStartRequest>,
) -> Result<(StatusCode, Json<ReconfigureStartResponse>), AuthError> {
    ensure_super_admin(&claims, &app_state, &headers).await?;

    let preserve_database = request.preserve_database.unwrap_or(true);
    let installer_url = format!("{}/api/reconfigure/start", installer_internal_url());

    let response = app_state
        .req_client
        .post(installer_url)
        .json(&serde_json::json!({
            "preserve_database": preserve_database,
        }))
        .send()
        .await
        .map_err(|e| {
            eprintln!("reconfigure trigger request failed: {e}");
            AuthError::ServiceTemporarilyUnavailable
        })?;

    let upstream_status = response.status();
    let body = response.text().await.map_err(|e| {
        eprintln!("reconfigure trigger response read failed: {e}");
        AuthError::ServiceTemporarilyUnavailable
    })?;

    let parsed: InstallerReconfigureResponse = serde_json::from_str(&body).map_err(|e| {
        eprintln!("reconfigure trigger response parse failed: {e}; body={body}");
        AuthError::ServiceTemporarilyUnavailable
    })?;

    let status = if upstream_status.is_success() && parsed.success {
        StatusCode::OK
    } else {
        StatusCode::BAD_GATEWAY
    };

    Ok((
        status,
        Json(ReconfigureStartResponse {
            success: parsed.success,
            message: parsed.message,
            next_step: parsed.next_step,
            detected_public_url: parsed.detected_public_url,
            preserve_database: parsed.preserve_database,
            warnings: parsed.warnings,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/reconfigure/domain",
    tag = "admin",
    request_body = DomainReconfigureRequest,
    responses(
       (status = 200, body = DomainReconfigureResponse),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 403, content_type = "application/json", body = Error, description = "Forbidden - Super Admin role required"),
       (status = 503, content_type = "application/json", body = Error, description = "Service unavailable"),
    )
)]
pub async fn reconfigure_domain(
    claims: Claims,
    State(app_state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<DomainReconfigureRequest>,
) -> Result<(StatusCode, Json<DomainReconfigureResponse>), AuthError> {
    ensure_super_admin(&claims, &app_state, &headers).await?;

    let domain = request.domain.trim().to_ascii_lowercase();
    if !is_valid_domain(&domain) {
        return Ok((
            StatusCode::OK,
            Json(DomainReconfigureResponse {
                success: false,
                message: "Invalid domain format".to_string(),
                domain,
                ssl_mode: request
                    .ssl_mode
                    .clone()
                    .unwrap_or_else(|| "letsencrypt".to_string()),
                redirect_url: String::new(),
                script_path: domain_reconfigure_script_path(),
                output: vec![],
            }),
        ));
    }

    let Some(ssl_mode) = normalized_ssl_mode(request.ssl_mode.as_deref()) else {
        return Ok((
            StatusCode::OK,
            Json(DomainReconfigureResponse {
                success: false,
                message: "Invalid ssl_mode. Allowed: letsencrypt, selfsigned, none".to_string(),
                domain,
                ssl_mode: request.ssl_mode.unwrap_or_default(),
                redirect_url: String::new(),
                script_path: domain_reconfigure_script_path(),
                output: vec![],
            }),
        ));
    };

    if ssl_mode == "letsencrypt"
        && request
            .email
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .is_none()
    {
        return Ok((
            StatusCode::OK,
            Json(DomainReconfigureResponse {
                success: false,
                message: "Email is required when ssl_mode is letsencrypt".to_string(),
                domain,
                ssl_mode,
                redirect_url: String::new(),
                script_path: domain_reconfigure_script_path(),
                output: vec![],
            }),
        ));
    }

    let script_path = domain_reconfigure_script_path();
    let use_sudo =
        match resolve_script_sudo_usage(domain_reconfigure_use_sudo(), "DOMAIN_RECONFIGURE") {
            Ok(value) => value,
            Err(message) => {
                return Ok((
                    StatusCode::OK,
                    Json(DomainReconfigureResponse {
                        success: false,
                        message,
                        domain,
                        ssl_mode,
                        redirect_url: String::new(),
                        script_path,
                        output: vec![],
                    }),
                ))
            }
        };

    let mut script_args = vec![
        "--domain".to_string(),
        domain.clone(),
        "--ssl-mode".to_string(),
        ssl_mode.clone(),
    ];
    if let Some(email) = request
        .email
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        script_args.push("--email".to_string());
        script_args.push(email.to_string());
    }
    if let Some(days) = request.self_signed_days {
        script_args.push("--self-signed-days".to_string());
        script_args.push(days.to_string());
    }

    let output = run_script_command(
        &script_path,
        &script_args,
        use_sudo,
        900,
        "domain reconfigure script",
    )
    .await?;

    let script_lines = summarize_output(&output.stdout, &output.stderr);
    let redirect_url = if ssl_mode == "none" {
        format!("http://{domain}")
    } else {
        format!("https://{domain}")
    };

    let (success, message) = if output.status.success() {
        (true, "Domain reconfigured successfully".to_string())
    } else {
        (
            false,
            format!(
                "Domain reconfiguration script failed with status {}",
                output.status
            ),
        )
    };

    Ok((
        StatusCode::OK,
        Json(DomainReconfigureResponse {
            success,
            message,
            domain,
            ssl_mode,
            redirect_url,
            script_path,
            output: script_lines,
        }),
    ))
}

#[utoipa::path(
    post,
    path = "/admin/reconfigure/binaries",
    tag = "admin",
    request_body = BinariesUpdateRequest,
    responses(
       (status = 200, body = BinariesUpdateResponse),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 403, content_type = "application/json", body = Error, description = "Forbidden - Super Admin role required"),
       (status = 503, content_type = "application/json", body = Error, description = "Service unavailable"),
    )
)]
pub async fn update_binaries(
    claims: Claims,
    State(app_state): State<SharedState>,
    headers: HeaderMap,
    Json(request): Json<BinariesUpdateRequest>,
) -> Result<(StatusCode, Json<BinariesUpdateResponse>), AuthError> {
    ensure_super_admin(&claims, &app_state, &headers).await?;

    let script_path = binaries_update_script_path();
    let Some(version) = normalized_release_version(request.version.as_deref()) else {
        return Ok((
            StatusCode::OK,
            Json(BinariesUpdateResponse {
                success: false,
                message: "Invalid version. Allowed chars: [A-Za-z0-9._-] and no '/'".to_string(),
                version: request.version.unwrap_or_default(),
                release_base_url: release_base_url(
                    DEFAULT_RELEASE_BASE_URL,
                    request.release_base_url.as_deref(),
                ),
                arch: request.arch.unwrap_or_default(),
                update_installer: request.update_installer.unwrap_or(false),
                update_api: request.update_api.unwrap_or(true),
                update_webapp: request.update_webapp.unwrap_or(true),
                verify_checksums: request.verify_checksums.unwrap_or(true),
                script_path,
                output: vec![],
            }),
        ));
    };

    let Some(arch) = normalized_arch(request.arch.as_deref()) else {
        return Ok((
            StatusCode::OK,
            Json(BinariesUpdateResponse {
                success: false,
                message: "Invalid arch. Allowed: x86_64, aarch64".to_string(),
                version,
                release_base_url: release_base_url(
                    DEFAULT_RELEASE_BASE_URL,
                    request.release_base_url.as_deref(),
                ),
                arch: request.arch.unwrap_or_default(),
                update_installer: request.update_installer.unwrap_or(false),
                update_api: request.update_api.unwrap_or(true),
                update_webapp: request.update_webapp.unwrap_or(true),
                verify_checksums: request.verify_checksums.unwrap_or(true),
                script_path,
                output: vec![],
            }),
        ));
    };

    let release_base_url = release_base_url(
        DEFAULT_RELEASE_BASE_URL,
        request.release_base_url.as_deref(),
    );
    let update_installer = request.update_installer.unwrap_or(false);
    let update_api = request.update_api.unwrap_or(true);
    let update_webapp = request.update_webapp.unwrap_or(true);
    let verify_checksums = request.verify_checksums.unwrap_or(true);

    if !update_installer && !update_api && !update_webapp {
        return Ok((
            StatusCode::OK,
            Json(BinariesUpdateResponse {
                success: false,
                message: "Nothing to update. Enable at least one of update_installer/update_api/update_webapp".to_string(),
                version,
                release_base_url,
                arch,
                update_installer,
                update_api,
                update_webapp,
                verify_checksums,
                script_path,
                output: vec![],
            }),
        ));
    }

    let use_sudo = match resolve_script_sudo_usage(binaries_update_use_sudo(), "BINARY_UPDATE") {
        Ok(value) => value,
        Err(message) => {
            return Ok((
                StatusCode::OK,
                Json(BinariesUpdateResponse {
                    success: false,
                    message,
                    version,
                    release_base_url,
                    arch,
                    update_installer,
                    update_api,
                    update_webapp,
                    verify_checksums,
                    script_path,
                    output: vec![],
                }),
            ))
        }
    };

    let mut script_args = vec![
        "--release-base-url".to_string(),
        release_base_url.clone(),
        "--version".to_string(),
        version.clone(),
    ];
    if arch != "auto" {
        script_args.push("--arch".to_string());
        script_args.push(arch.clone());
    }
    if !update_installer {
        script_args.push("--skip-installer".to_string());
    }
    if !update_api {
        script_args.push("--skip-api".to_string());
    }
    if !update_webapp {
        script_args.push("--skip-webapp".to_string());
    }
    if !verify_checksums {
        script_args.push("--skip-checksum".to_string());
    }
    if let Some(service_name) = request
        .api_service_name
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        script_args.push("--api-service-name".to_string());
        script_args.push(service_name.to_string());
    }

    let output = run_script_command(
        &script_path,
        &script_args,
        use_sudo,
        1800,
        "binary update script",
    )
    .await?;

    let script_lines = summarize_output(&output.stdout, &output.stderr);
    let (success, message) = if output.status.success() {
        (
            true,
            "Application binaries updated successfully".to_string(),
        )
    } else {
        (
            false,
            format!("Binary update script failed with status {}", output.status),
        )
    };

    Ok((
        StatusCode::OK,
        Json(BinariesUpdateResponse {
            success,
            message,
            version,
            release_base_url,
            arch,
            update_installer,
            update_api,
            update_webapp,
            verify_checksums,
            script_path,
            output: script_lines,
        }),
    ))
}
