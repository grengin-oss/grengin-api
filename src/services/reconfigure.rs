// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

use axum::http::HeaderMap;
use reqwest::Url;
use std::{fs, path::Path, process::Command as StdCommand, time::Duration};
use tokio::process::Command;

use crate::{
    auth::{claims::Claims, error::AuthError, permissions::PERMISSION_SYSTEM_MAINTAIN},
    dto::admin_reconfigure_dto::ReconfigureScriptAvailability,
    services::authorization::{AuthorizationService, PermissionScopeMode},
    state::SharedState,
};

pub const RECONFIGURE_TOKEN_HEADER: &str = "x-grengin-reconfigure-token";
pub const DEFAULT_DOMAIN_RECONFIGURE_SCRIPT: &str = "/opt/grengin/scripts/reconfigure-domain.sh";
pub const DEFAULT_DOMAIN_RECONFIGURE_USE_SUDO: bool = true;
pub const DEFAULT_BINARY_UPDATE_SCRIPT: &str = "/opt/grengin/scripts/update-app-binaries.sh";
pub const DEFAULT_BINARY_UPDATE_USE_SUDO: bool = true;
pub const DEFAULT_RELEASE_BASE_URL: &str = "https://releases.grengin.io";

pub fn expected_reconfigure_token() -> Option<String> {
    std::env::var("INSTALLER_RECONFIGURE_TOKEN")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn provided_reconfigure_token(headers: &HeaderMap) -> Option<String> {
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

pub fn configured_release_base_url() -> String {
    std::env::var("GRENGIN_RELEASE_BASE_URL")
        .or_else(|_| std::env::var("RELEASE_BASE_URL"))
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| DEFAULT_RELEASE_BASE_URL.to_string())
}

pub fn requested_release_base_url(provided: Option<&str>) -> String {
    provided
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|v| v.trim_end_matches('/').to_string())
        .unwrap_or_else(configured_release_base_url)
}

#[derive(Debug, PartialEq, Eq)]
pub enum BinaryUpdateRejection {
    InvalidReleaseUrl,
    InsecureReleaseUrl,
    ReleaseOriginNotAllowed,
    ChecksumVerificationRequired,
}

impl BinaryUpdateRejection {
    pub fn message(&self) -> &'static str {
        match self {
            Self::InvalidReleaseUrl => {
                "Invalid release_base_url. Expected an absolute https URL without credentials, query or fragment"
            }
            Self::InsecureReleaseUrl => "release_base_url must use https",
            Self::ReleaseOriginNotAllowed => {
                "release_base_url must be on the configured release origin (GRENGIN_RELEASE_BASE_URL) or the official Grengin release origin"
            }
            Self::ChecksumVerificationRequired => {
                "Checksum verification cannot be disabled for binary updates"
            }
        }
    }
}

pub fn resolve_release_base_url(
    requested: Option<&str>,
    configured: &str,
) -> Result<String, BinaryUpdateRejection> {
    let configured = parse_release_url(configured)?;
    let Some(requested) = requested.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(canonical_release_url(&configured));
    };
    let requested = parse_release_url(requested)?;
    let official = parse_release_url(DEFAULT_RELEASE_BASE_URL)?;
    if requested.origin() != configured.origin() && requested.origin() != official.origin() {
        return Err(BinaryUpdateRejection::ReleaseOriginNotAllowed);
    }
    Ok(canonical_release_url(&requested))
}

fn parse_release_url(value: &str) -> Result<Url, BinaryUpdateRejection> {
    let url = Url::parse(value).map_err(|_| BinaryUpdateRejection::InvalidReleaseUrl)?;
    if url.scheme() != "https" {
        return Err(BinaryUpdateRejection::InsecureReleaseUrl);
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(BinaryUpdateRejection::InvalidReleaseUrl);
    }
    Ok(url)
}

// The root update script hands this to curl; passing the parser's canonical form keeps curl
// from reading a different host out of backslash or userinfo tricks than the one checked here.
fn canonical_release_url(url: &Url) -> String {
    url.as_str().trim_end_matches('/').to_string()
}

pub fn require_checksum_verification(requested: Option<bool>) -> Result<(), BinaryUpdateRejection> {
    match requested {
        Some(false) => Err(BinaryUpdateRejection::ChecksumVerificationRequired),
        _ => Ok(()),
    }
}

pub struct BinaryUpdatePlan {
    pub release_base_url: String,
    pub version: String,
    pub arch: String,
    pub update_installer: bool,
    pub update_api: bool,
    pub update_webapp: bool,
    pub api_service_name: Option<String>,
}

impl BinaryUpdatePlan {
    pub fn script_args(&self) -> Vec<String> {
        let mut args = vec![
            "--release-base-url".to_string(),
            self.release_base_url.clone(),
            "--version".to_string(),
            self.version.clone(),
        ];
        if self.arch != "auto" {
            args.push("--arch".to_string());
            args.push(self.arch.clone());
        }
        if !self.update_installer {
            args.push("--skip-installer".to_string());
        }
        if !self.update_api {
            args.push("--skip-api".to_string());
        }
        if !self.update_webapp {
            args.push("--skip-webapp".to_string());
        }
        if let Some(service_name) = &self.api_service_name {
            args.push("--api-service-name".to_string());
            args.push(service_name.clone());
        }
        args
    }
}

pub fn domain_reconfigure_script_path() -> String {
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

pub fn domain_reconfigure_use_sudo() -> bool {
    parse_bool_env(
        "DOMAIN_RECONFIGURE_USE_SUDO",
        DEFAULT_DOMAIN_RECONFIGURE_USE_SUDO,
    )
}

pub fn binaries_update_script_path() -> String {
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

pub fn binaries_update_use_sudo() -> bool {
    parse_bool_env("BINARY_UPDATE_USE_SUDO", DEFAULT_BINARY_UPDATE_USE_SUDO)
}

pub fn command_exists(command: &str) -> bool {
    if command.contains('/') {
        return Path::new(command).is_file();
    }

    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(command).is_file()))
        .unwrap_or(false)
}

pub fn running_as_root() -> bool {
    StdCommand::new("id")
        .arg("-u")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
        .unwrap_or(false)
}

pub fn script_exists(script_path: &str) -> bool {
    Path::new(script_path).is_file()
}

pub fn script_executable(script_path: &str) -> bool {
    let path = Path::new(script_path);
    if !path.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|m| (m.permissions().mode() & 0o111) != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        true
    }
}

pub fn resolve_script_sudo_usage(request_use_sudo: bool, env_prefix: &str) -> Result<bool, String> {
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

pub fn should_retry_without_sudo(stderr: &[u8]) -> bool {
    let lowered = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    lowered.contains("no new privileges")
        || lowered.contains("setresuid")
        || lowered.contains("setreuid")
        || lowered.contains("operation not permitted")
}

pub async fn run_with_timeout(
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

pub async fn run_script_command(
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
        return run_with_timeout(build_command(false), timeout_seconds, context, script_path).await;
    }

    Ok(output)
}

pub fn is_valid_domain(domain: &str) -> bool {
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

pub fn normalized_ssl_mode(mode: Option<&str>) -> Option<String> {
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

pub fn normalized_release_version(value: Option<&str>) -> Option<String> {
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

pub fn normalized_arch(value: Option<&str>) -> Option<String> {
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

pub fn summarize_output(stdout: &[u8], stderr: &[u8]) -> Vec<String> {
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

pub fn build_script_availability(
    script_path: String,
    requested_use_sudo: bool,
    env_prefix: &str,
) -> ReconfigureScriptAvailability {
    let exists = script_exists(&script_path);
    let executable = script_executable(&script_path);

    let (available, effective_use_sudo, reason) = if !exists {
        (
            false,
            false,
            Some(format!("Script not found at path: {script_path}")),
        )
    } else if !executable {
        (
            false,
            false,
            Some(format!("Script is not executable: {script_path}")),
        )
    } else {
        match resolve_script_sudo_usage(requested_use_sudo, env_prefix) {
            Ok(value) => (true, value, None),
            Err(message) => (false, false, Some(message)),
        }
    };

    ReconfigureScriptAvailability {
        script_path,
        exists,
        executable,
        requested_use_sudo,
        effective_use_sudo,
        available,
        reason,
    }
}

pub async fn ensure_system_maintainer(
    claims: &Claims,
    app_state: &SharedState,
    headers: &HeaderMap,
) -> Result<(), AuthError> {
    let authz = AuthorizationService::new(&app_state.database);
    authz
        .ensure_permission(
            claims.user_id,
            PERMISSION_SYSTEM_MAINTAIN,
            None,
            PermissionScopeMode::RequireOrgWide,
            None,
        )
        .await?;

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

#[cfg(test)]
mod tests {
    use super::*;

    const MIRROR: &str = "https://mirror.example.com/grengin";

    fn plan(release_base_url: &str) -> BinaryUpdatePlan {
        BinaryUpdatePlan {
            release_base_url: release_base_url.to_string(),
            version: "v1.2.3".to_string(),
            arch: "auto".to_string(),
            update_installer: true,
            update_api: true,
            update_webapp: true,
            api_service_name: None,
        }
    }

    #[test]
    fn release_url_defaults_to_the_configured_base() {
        assert_eq!(
            resolve_release_base_url(None, DEFAULT_RELEASE_BASE_URL),
            Ok("https://releases.grengin.io".to_string())
        );
        assert_eq!(
            resolve_release_base_url(Some("  "), MIRROR),
            Ok(MIRROR.to_string())
        );
    }

    #[test]
    fn release_url_on_the_configured_origin_is_accepted() {
        assert_eq!(
            resolve_release_base_url(Some("https://mirror.example.com/other/"), MIRROR),
            Ok("https://mirror.example.com/other".to_string())
        );
    }

    #[test]
    fn official_release_origin_is_accepted_when_a_mirror_is_configured() {
        assert_eq!(
            resolve_release_base_url(Some("https://releases.grengin.io"), MIRROR),
            Ok("https://releases.grengin.io".to_string())
        );
    }

    #[test]
    fn plain_http_release_url_is_rejected() {
        assert_eq!(
            resolve_release_base_url(Some("http://releases.grengin.io"), DEFAULT_RELEASE_BASE_URL),
            Err(BinaryUpdateRejection::InsecureReleaseUrl)
        );
    }

    #[test]
    fn insecure_configured_release_base_is_rejected() {
        assert_eq!(
            resolve_release_base_url(None, "http://mirror.example.com"),
            Err(BinaryUpdateRejection::InsecureReleaseUrl)
        );
    }

    #[test]
    fn look_alike_and_foreign_release_hosts_are_rejected() {
        for candidate in [
            "https://evil.example.com",
            "https://releases.grengin.io.evil.com",
            "https://evilreleases.grengin.io",
            "https://releases.grengin.io:8443",
        ] {
            assert_eq!(
                resolve_release_base_url(Some(candidate), DEFAULT_RELEASE_BASE_URL),
                Err(BinaryUpdateRejection::ReleaseOriginNotAllowed),
                "{candidate}"
            );
        }
    }

    #[test]
    fn userinfo_query_and_fragment_release_urls_are_rejected() {
        for candidate in [
            "https://releases.grengin.io@evil.com",
            "https://user:pass@releases.grengin.io",
            "https://releases.grengin.io/?x=1",
            "https://releases.grengin.io/#frag",
        ] {
            assert_eq!(
                resolve_release_base_url(Some(candidate), DEFAULT_RELEASE_BASE_URL),
                Err(BinaryUpdateRejection::InvalidReleaseUrl),
                "{candidate}"
            );
        }
    }

    #[test]
    fn unparseable_release_url_is_rejected() {
        assert_eq!(
            resolve_release_base_url(Some("releases.grengin.io"), DEFAULT_RELEASE_BASE_URL),
            Err(BinaryUpdateRejection::InvalidReleaseUrl)
        );
    }

    #[test]
    fn release_url_reaches_the_script_in_canonical_form() {
        assert_eq!(
            resolve_release_base_url(
                Some(r"HTTPS://Releases.Grengin.IO\@evil.com"),
                DEFAULT_RELEASE_BASE_URL
            ),
            Ok("https://releases.grengin.io/@evil.com".to_string())
        );
    }

    #[test]
    fn checksum_verification_defaults_on_and_cannot_be_disabled() {
        assert_eq!(require_checksum_verification(None), Ok(()));
        assert_eq!(require_checksum_verification(Some(true)), Ok(()));
        assert_eq!(
            require_checksum_verification(Some(false)),
            Err(BinaryUpdateRejection::ChecksumVerificationRequired)
        );
    }

    #[test]
    fn update_script_is_never_told_to_skip_checksums() {
        let mut plan = plan(DEFAULT_RELEASE_BASE_URL);
        plan.update_installer = false;
        plan.update_api = false;
        plan.arch = "aarch64".to_string();
        plan.api_service_name = Some("grengin".to_string());
        let args = plan.script_args();
        assert!(!args.iter().any(|arg| arg == "--skip-checksum"));
        assert_eq!(
            args,
            [
                "--release-base-url",
                DEFAULT_RELEASE_BASE_URL,
                "--version",
                "v1.2.3",
                "--arch",
                "aarch64",
                "--skip-installer",
                "--skip-api",
                "--api-service-name",
                "grengin",
            ]
        );
    }

    #[test]
    fn auto_arch_is_left_to_the_script() {
        let args = plan(DEFAULT_RELEASE_BASE_URL).script_args();
        assert!(!args.iter().any(|arg| arg == "--arch"));
    }
}
