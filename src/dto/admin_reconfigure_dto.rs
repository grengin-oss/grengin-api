use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct DomainReconfigureRequest {
    pub domain: String,
    /// `letsencrypt`, `selfsigned`, or `none`
    pub ssl_mode: Option<String>,
    pub email: Option<String>,
    pub self_signed_days: Option<u16>,
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

#[derive(Debug, Serialize, ToSchema)]
pub struct ReconfigureScriptAvailability {
    pub script_path: String,
    pub exists: bool,
    pub executable: bool,
    pub requested_use_sudo: bool,
    pub effective_use_sudo: bool,
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ReconfigureAvailableResponse {
    pub success: bool,
    pub message: String,
    pub running_as_root: bool,
    pub sudo_available: bool,
    pub domain: ReconfigureScriptAvailability,
    pub binaries: ReconfigureScriptAvailability,
}
