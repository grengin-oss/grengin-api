use axum::{Json, extract::State, http::HeaderMap};
use reqwest::StatusCode;

use crate::{
    auth::{
        claims::Claims,
        error::{AuthError, Error},
        permissions::{PERMISSION_ROLES_MANAGE, ROLE_SUPER_ADMIN},
    },
    dto::admin_reconfigure_dto::{
        BinariesUpdateRequest, BinariesUpdateResponse, DomainReconfigureRequest,
        DomainReconfigureResponse, ReconfigureAvailableResponse, ReconfigureScriptAvailability,
    },
    services::{
        authorization::{AuthorizationService, PermissionScopeMode},
        reconfigure::{self, DEFAULT_RELEASE_BASE_URL},
    },
    state::SharedState,
};

fn build_script_availability(
    script_path: String,
    requested_use_sudo: bool,
    env_prefix: &str,
) -> ReconfigureScriptAvailability {
    let exists = reconfigure::script_exists(&script_path);
    let executable = reconfigure::script_executable(&script_path);

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
        match reconfigure::resolve_script_sudo_usage(requested_use_sudo, env_prefix) {
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

    if let Some(expected_token) = reconfigure::expected_reconfigure_token() {
        let Some(provided_token) = reconfigure::provided_reconfigure_token(headers) else {
            return Err(AuthError::PermissionDenied);
        };
        if provided_token != expected_token {
            return Err(AuthError::PermissionDenied);
        }
    }

    Ok(())
}

#[utoipa::path(
    get,
    path = "/admin/reconfigure/available",
    tag = "admin",
    responses(
       (status = 200, body = ReconfigureAvailableResponse),
       (status = 401, content_type = "application/json", body = Error, description = "Invalid/expired token (code=6103)"),
       (status = 403, content_type = "application/json", body = Error, description = "Forbidden - Super Admin role required"),
    )
)]
pub async fn get_reconfigure_available(
    claims: Claims,
    State(app_state): State<SharedState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<ReconfigureAvailableResponse>), AuthError> {
    ensure_super_admin(&claims, &app_state, &headers).await?;

    let domain = build_script_availability(
        reconfigure::domain_reconfigure_script_path(),
        reconfigure::domain_reconfigure_use_sudo(),
        "DOMAIN_RECONFIGURE",
    );
    let binaries = build_script_availability(
        reconfigure::binaries_update_script_path(),
        reconfigure::binaries_update_use_sudo(),
        "BINARY_UPDATE",
    );

    let success = domain.available && binaries.available;
    let message = if success {
        "Reconfigure scripts are available".to_string()
    } else {
        "One or more reconfigure scripts are unavailable".to_string()
    };

    Ok((
        StatusCode::OK,
        Json(ReconfigureAvailableResponse {
            success,
            message,
            running_as_root: reconfigure::running_as_root(),
            sudo_available: reconfigure::command_exists("sudo"),
            domain,
            binaries,
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

    let script_path = reconfigure::domain_reconfigure_script_path();
    let domain = request.domain.trim().to_ascii_lowercase();
    if !reconfigure::is_valid_domain(&domain) {
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
                script_path,
                output: vec![],
            }),
        ));
    }

    let Some(ssl_mode) = reconfigure::normalized_ssl_mode(request.ssl_mode.as_deref()) else {
        return Ok((
            StatusCode::OK,
            Json(DomainReconfigureResponse {
                success: false,
                message: "Invalid ssl_mode. Allowed: letsencrypt, selfsigned, none".to_string(),
                domain,
                ssl_mode: request.ssl_mode.unwrap_or_default(),
                redirect_url: String::new(),
                script_path,
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
                script_path,
                output: vec![],
            }),
        ));
    }

    let use_sudo =
        match reconfigure::resolve_script_sudo_usage(reconfigure::domain_reconfigure_use_sudo(), "DOMAIN_RECONFIGURE") {
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

    let output = reconfigure::run_script_command(
        &script_path,
        &script_args,
        use_sudo,
        900,
        "domain reconfigure script",
    )
    .await?;

    let script_lines = reconfigure::summarize_output(&output.stdout, &output.stderr);
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

    let script_path = reconfigure::binaries_update_script_path();
    let Some(version) = reconfigure::normalized_release_version(request.version.as_deref()) else {
        return Ok((
            StatusCode::OK,
            Json(BinariesUpdateResponse {
                success: false,
                message: "Invalid version. Allowed chars: [A-Za-z0-9._-] and no '/'".to_string(),
                version: request.version.unwrap_or_default(),
                release_base_url: reconfigure::release_base_url(
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

    let Some(arch) = reconfigure::normalized_arch(request.arch.as_deref()) else {
        return Ok((
            StatusCode::OK,
            Json(BinariesUpdateResponse {
                success: false,
                message: "Invalid arch. Allowed: x86_64, aarch64".to_string(),
                version,
                release_base_url: reconfigure::release_base_url(
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

    let release_base_url = reconfigure::release_base_url(
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

    let use_sudo =
        match reconfigure::resolve_script_sudo_usage(reconfigure::binaries_update_use_sudo(), "BINARY_UPDATE") {
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

    let output = reconfigure::run_script_command(
        &script_path,
        &script_args,
        use_sudo,
        1800,
        "binary update script",
    )
    .await?;

    let script_lines = reconfigure::summarize_output(&output.stdout, &output.stderr);
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
