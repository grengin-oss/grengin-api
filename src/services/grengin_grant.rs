// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

//! Credentials for an already registered workspace. Keys are loaded once; grants stay in memory.
use super::{
    catalog_download::read_body_capped,
    connector_broker::{hex, random_secret},
};
use crate::models::grengin_service_bindings;
use base64::{Engine, engine::general_purpose::STANDARD};
use llm_plugin::ProviderError;
use openssl::{
    pkey::{Id, PKey, Private},
    sign::Signer,
};
use sea_orm::{ActiveValue::Set, DatabaseConnection, EntityTrait, sea_query::OnConflict};
use serde::{Deserialize, Serialize};
use std::{
    fs::OpenOptions,
    io::Read,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const MAX_BYTES: usize = 64 * 1024;
const REFRESH_PATH: &str = "/v1/connect/refresh";
const INTROSPECT_PATH: &str = "/v1/connect/introspect";
const REFRESH_DOMAIN: &str = "grengin-connect-v2";
const INTROSPECT_DOMAIN: &str = "grengin-introspect-v2";
// A grant the gateway rejects this soon after it was issued points at the identity, not expiry.
const MIN_GRANT_AGE_FOR_REJECTION: Duration = Duration::from_secs(5);
static DATABASE: OnceLock<DatabaseConnection> = OnceLock::new();
type SetupResult<T> = Result<T, &'static str>;

// Neither private documents, proofs, nor cached grants implement Debug.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    schema_version: u32,
    mode: String,
    environment: String,
    service_origin: String,
    instance_uuid: String,
    #[serde(deserialize_with = "required_workspace")]
    workspace_id: Option<String>,
    identity: Keys,
}

fn required_workspace<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Keys {
    ed25519_private_key_pkcs8_base64: String,
    ml_dsa65_private_key_pkcs8_base64: String,
    public_key: String,
    ml_dsa_public_key: String,
}

struct Identity {
    origin: &'static str,
    uuid: String,
    workspace_id: Option<String>,
    ed: PKey<Private>,
    ml: PKey<Private>,
}

impl Identity {
    fn load(path: &Path) -> SetupResult<Self> {
        if !path.is_absolute() {
            return Err("identity path must be absolute");
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
        }
        let file = options
            .open(path)
            .map_err(|_| "identity file is unavailable")?;
        let metadata = file
            .metadata()
            .map_err(|_| "identity file is unavailable")?;
        if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
            return Err("invalid identity file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o7777 != 0o600 || metadata.uid() != unsafe { libc::geteuid() } {
                return Err("identity file must be owned by this user with mode 0600");
            }
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "identity file is unavailable")?;
        Self::parse(&bytes)
    }

    fn parse(bytes: &[u8]) -> SetupResult<Self> {
        if bytes.len() > MAX_BYTES {
            return Err("identity file is too large");
        }
        let document: Document =
            serde_json::from_slice(bytes).map_err(|_| "invalid identity document")?;
        let origin = match document.environment.as_str() {
            "prod" => "https://service.grengin.com",
            "staging" | "dev" => "https://service-staging.grengin.com",
            _ => return Err("invalid identity environment"),
        };
        let valid_workspace = match document.mode.as_str() {
            "standalone" => document.workspace_id.is_none(),
            "managed_cloud" => document.workspace_id.as_deref().is_some_and(|id| {
                id.starts_with("cw_")
                    && id.len() > 3
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            }),
            _ => false,
        };
        if document.schema_version != 1 || document.service_origin != origin || !valid_workspace {
            return Err("invalid identity metadata");
        }
        let uuid =
            uuid::Uuid::parse_str(&document.instance_uuid).map_err(|_| "invalid instance UUID")?;
        if document.instance_uuid.len() != 36
            || uuid.get_version() != Some(uuid::Version::Random)
            || uuid.get_variant() != uuid::Variant::RFC4122
        {
            return Err("invalid instance UUID");
        }
        let ed = private_key(&document.identity.ed25519_private_key_pkcs8_base64)?;
        let ml = private_key(&document.identity.ml_dsa65_private_key_pkcs8_base64)?;
        let ed_public = ed.raw_public_key().map_err(|_| "invalid Ed25519 key")?;
        let ml_public = ml.raw_public_key().map_err(|_| "invalid ML-DSA-65 key")?;
        // Compare the complete canonical SPKI, including ML-DSA-65's algorithm OID.
        // OpenSSL's provider-based ML-DSA keys have no legacy numeric PKey ID.
        let mut ml_spki = vec![
            0x30, 0x82, 0x07, 0xb2, 0x30, 0x0b, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03,
            0x04, 0x03, 0x12, 0x03, 0x82, 0x07, 0xa1, 0x00,
        ];
        ml_spki.extend_from_slice(&ml_public);
        if ed.id() != Id::ED25519
            || ed_public.len() != 32
            || ml_public.len() != 1952
            || ml
                .public_key_to_der()
                .map_err(|_| "invalid ML-DSA-65 key")?
                != ml_spki
        {
            return Err("incorrect identity key algorithm");
        }
        if hex(&ed_public) != document.identity.public_key
            || hex(&ml_public) != document.identity.ml_dsa_public_key
        {
            return Err("identity public and private keys do not match");
        }
        Ok(Self {
            origin,
            uuid: uuid.hyphenated().to_string(),
            workspace_id: document.workspace_id,
            ed,
            ml,
        })
    }

    /// A managed identity belongs to exactly the workspace this runtime was launched for, so a
    /// file copied from another workspace can never authenticate as, or charge, that workspace.
    fn check_workspace(&self, launched_for: Option<&str>) -> SetupResult<()> {
        if self.workspace_id.as_deref() == launched_for {
            Ok(())
        } else {
            Err("identity does not belong to this workspace")
        }
    }

    fn proof(&self, domain: &str) -> SetupResult<Proof> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "invalid system clock")?
            .as_secs();
        let nonce = random_secret().map_err(|_| "identity random generation failed")?;
        let message = format!("{domain}\n{}\n{timestamp}\n{nonce}", self.uuid);
        let sign = |key: &PKey<Private>| {
            Signer::new_without_digest(key)
                .and_then(|mut signer| signer.sign_oneshot_to_vec(message.as_bytes()))
                .map_err(|_| "identity signing failed")
        };
        let ed = sign(&self.ed)?;
        let ml = sign(&self.ml)?;
        if ed.len() != 64 || ml.len() != 3309 {
            return Err("invalid identity signature length");
        }
        Ok(Proof {
            instance_uuid: self.uuid.clone(),
            timestamp,
            nonce,
            signature: hex(&ed),
            ml_dsa_signature: hex(&ml),
        })
    }
}

fn private_key(value: &str) -> SetupResult<PKey<Private>> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| "invalid private key encoding")?;
    if STANDARD.encode(&bytes) != value || bytes.first() != Some(&0x30) || bytes.len() < 2 {
        return Err("invalid private key encoding");
    }
    // d2i accepts trailing bytes; require one definite, minimally encoded DER sequence.
    let (length, header) = if bytes[1] < 128 {
        (bytes[1] as usize, 2)
    } else {
        let count = (bytes[1] & 127) as usize;
        if count == 0 || count > 3 || bytes.len() < 2 + count || bytes[2] == 0 {
            return Err("invalid private key encoding");
        }
        let length = bytes[2..2 + count]
            .iter()
            .fold(0usize, |n, b| (n << 8) | *b as usize);
        if length < 128 {
            return Err("invalid private key encoding");
        }
        (length, 2 + count)
    };
    if header + length != bytes.len() {
        return Err("invalid private key encoding");
    }
    PKey::private_key_from_pkcs8(&bytes).map_err(|_| "invalid PKCS#8 private key")
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    instance_uuid: String,
    timestamp: u64,
    nonce: String,
    signature: String,
    ml_dsa_signature: String,
}

struct Cached {
    bearer: String,
    issued_at: Instant,
    refresh_at: Instant,
    expires_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Failure {
    Setup(&'static str),
    Transport,
    Response,
    Rejected(u16),
    Revoked,
    BindingMismatch,
    BindingUnavailable,
}
impl Failure {
    fn error(self) -> ProviderError {
        match self {
            Self::Setup(message) => setup_error(message),
            Self::Transport => {
                ProviderError::Transport("Grengin credential refresh unavailable".into())
            }
            Self::Response => {
                ProviderError::ResponseMapping("invalid Grengin credential response".into())
            }
            Self::Rejected(status) => ProviderError::HttpStatus {
                status,
                message: "Grengin credential refresh rejected".into(),
            },
            Self::Revoked => {
                ProviderError::Configuration("Grengin workspace connection is revoked".into())
            }
            Self::BindingMismatch => ProviderError::Configuration(
                "Grengin identity belongs to a different service installation".into(),
            ),
            Self::BindingUnavailable => {
                ProviderError::Transport("Grengin connection binding unavailable".into())
            }
        }
    }
    // Only these leave a still-valid grant usable; every other failure drops it (fail closed).
    // No failure is permanent: each one backs off and the next attempt can recover.
    fn transient(self) -> bool {
        matches!(
            self,
            Self::Transport
                | Self::BindingUnavailable
                | Self::Rejected(408 | 409 | 425 | 429 | 500..=599)
        )
    }
}

#[derive(Default)]
struct State {
    grant: Option<Cached>,
    failure: Option<Failure>,
    retry_at: Option<Instant>,
    failures: u32,
    // Confirmed by introspection; kept across grants so a later grant cannot switch it.
    binding: Option<(String, String)>,
}

pub struct GrenginGrant {
    identity: Identity,
    client: reqwest::Client,
    refresh_url: String,
    introspect_url: String,
    service_origin: String,
    database: Option<DatabaseConnection>,
    state: Mutex<State>,
}

fn setup_error(message: &'static str) -> ProviderError {
    ProviderError::Configuration(format!("Grengin setup required: {message}"))
}

impl GrenginGrant {
    /// Called once at startup: the confirmed service binding is persisted here.
    pub fn attach_database(database: DatabaseConnection) {
        let _ = DATABASE.set(database);
    }

    /// One process-wide identity; deliberate key replacement requires a restart.
    pub fn configured() -> Result<&'static Self, ProviderError> {
        static GRANT: OnceLock<SetupResult<GrenginGrant>> = OnceLock::new();
        GRANT
            .get_or_init(|| {
                let path = std::env::var_os("GRENGIN_IDENTITY_FILE")
                    .ok_or("GRENGIN_IDENTITY_FILE is not configured")?;
                let identity = Identity::load(Path::new(&path))?;
                identity.check_workspace(std::env::var("HATCHERY_WORKSPACE_ID").ok().as_deref())?;
                let database = DATABASE
                    .get()
                    .cloned()
                    .ok_or("connection binding storage is unavailable")?;
                Self::new(identity, Some(database))
            })
            .as_ref()
            .map_err(|message| setup_error(message))
    }

    fn new(identity: Identity, database: Option<DatabaseConnection>) -> SetupResult<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| "credential client unavailable")?;
        let refresh_url = format!("{}{REFRESH_PATH}", identity.origin);
        let introspect_url = format!("{}{INTROSPECT_PATH}", identity.origin);
        let service_origin = identity.origin.to_owned();
        Ok(Self {
            identity,
            client,
            refresh_url,
            introspect_url,
            service_origin,
            database,
            state: Mutex::new(State::default()),
        })
    }

    pub(super) fn has_managed_attachment() -> Result<bool, ProviderError> {
        // A variable left blank by a deployment template means no managed attachment.
        let set = |name: &str| std::env::var(name).is_ok_and(|value| !value.trim().is_empty());
        if !set("HATCHERY_WORKSPACE_ID") || !set("GRENGIN_IDENTITY_FILE") {
            return Ok(false);
        }
        Ok(Self::configured()?.identity.workspace_id.is_some())
    }

    pub fn origin(&self) -> &'static str {
        self.identity.origin
    }

    /// Authenticated, non-billable connection check. The result describes remote model
    /// access; an accepted workspace identity can still have access switched off.
    pub async fn status(&self) -> Result<&'static str, ProviderError> {
        Ok(self.workspace().await?.engine_state)
    }

    /// The workspace's service status: model access and balance. Balances the service omits
    /// stay `None`, never zero.
    pub async fn workspace(&self) -> Result<WorkspaceStatus, ProviderError> {
        self.status_at(&format!("{}/v1/infer/status", self.service_origin))
            .await
    }

    /// Switches workspace model access on the service (`PUT /v1/connect/grengin-models`).
    /// Enabling records the administrator's consent version.
    pub async fn set_models_enabled(&self, enabled: bool) -> Result<(), ProviderError> {
        let body = if enabled {
            serde_json::json!({"enabled": true, "consent_version": GRENGIN_CONSENT_VERSION})
        } else {
            serde_json::json!({"enabled": false})
        };
        let url = format!("{}/v1/connect/grengin-models", self.service_origin);
        let response = self
            .send_with_grant(|bearer| self.client.put(&url).bearer_auth(bearer).json(&body))
            .await
            .map_err(|error| match error {
                ProviderError::Transport(_) => {
                    ProviderError::Transport("Grengin model access unavailable".into())
                }
                other => other,
            })?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let bytes = read_body_capped(response, MAX_BYTES)
            .await
            .unwrap_or_default();
        Err(ProviderError::HttpStatus {
            status: status.as_u16(),
            message: service_error_type(&bytes).unwrap_or_else(|| "rejected".into()),
        })
    }

    async fn status_at(&self, url: &str) -> Result<WorkspaceStatus, ProviderError> {
        let response = self
            .send_with_grant(|bearer| self.client.get(url).bearer_auth(bearer))
            .await
            .map_err(|error| match error {
                ProviderError::Transport(_) => {
                    ProviderError::Transport("Grengin workspace status unavailable".into())
                }
                other => other,
            })?;
        if !response.status().is_success() {
            return Err(ProviderError::HttpStatus {
                status: response.status().as_u16(),
                message: "Grengin workspace status rejected".into(),
            });
        }
        let bytes = read_body_capped(response, MAX_BYTES).await.map_err(|_| {
            ProviderError::ResponseMapping("invalid Grengin workspace status".into())
        })?;
        let status = workspace_document(&bytes)?;
        if let Some(expected_workspace) = &self.identity.workspace_id {
            let state = self.state.lock().await;
            if status.workspace_id.as_ref() != Some(expected_workspace)
                || state.binding.as_ref().map(|(id, _)| id) != status.installation_id.as_ref()
                || status.installation_id.is_none()
            {
                return Err(ProviderError::Configuration(
                    "Grengin status does not match the managed identity".into(),
                ));
            }
        }
        Ok(status)
    }

    /// Sends a control request with the current grant. The service refuses an invalid grant
    /// with `invalid_connection_grant` before doing anything, so that exact grant is dropped
    /// and the request is sent once more with a new one; a grant refused straight after issue
    /// backs off instead (see `reject`), which bounds reauthentication.
    async fn send_with_grant<F>(&self, request: F) -> Result<reqwest::Response, ProviderError>
    where
        F: Fn(&str) -> reqwest::RequestBuilder,
    {
        let mut retried = false;
        loop {
            let bearer = self.bearer().await?;
            let response = request(&bearer)
                .send()
                .await
                .map_err(|_| ProviderError::Transport("Grengin service unavailable".into()))?;
            if response.status() != reqwest::StatusCode::UNAUTHORIZED {
                return Ok(response);
            }
            let bytes = read_body_capped(response, MAX_BYTES)
                .await
                .unwrap_or_default();
            let kind = service_error_type(&bytes);
            if kind.as_deref() == Some("invalid_connection_grant") {
                self.reject(&bearer).await;
                if !retried {
                    retried = true;
                    continue;
                }
            }
            return Err(ProviderError::HttpStatus {
                status: 401,
                message: kind.unwrap_or_else(|| "rejected".into()),
            });
        }
    }

    /// Called at every native operation dispatch, including later tool rounds.
    pub async fn bearer(&self) -> Result<String, ProviderError> {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        if let Some(grant) = &state.grant
            && now < grant.refresh_at
        {
            return Ok(grant.bearer.clone());
        }
        if state.retry_at.is_some_and(|retry| now < retry) {
            if let Some(grant) = &state.grant
                && now < grant.expires_at
            {
                return Ok(grant.bearer.clone());
            }
            return Err(state.failure.unwrap_or(Failure::Transport).error());
        }
        // Keep the mutex only for these bounded control requests, never for inference.
        let result = match self.refresh().await {
            Ok(grant) => self
                .confirm(&grant.bearer, &mut state.binding)
                .await
                .map(|()| grant),
            Err(failure) => Err(failure),
        };
        match result {
            Ok(grant) => {
                let bearer = grant.bearer.clone();
                state.grant = Some(grant);
                state.failure = None;
                state.retry_at = None;
                state.failures = 0;
                Ok(bearer)
            }
            Err((failure, retry_after)) => {
                Self::back_off(&mut state, failure, retry_after);
                if failure.transient()
                    && let Some(grant) = &state.grant
                    && Instant::now() < grant.expires_at
                {
                    return Ok(grant.bearer.clone());
                }
                state.grant = None;
                Err(failure.error())
            }
        }
    }

    /// The gateway rejected `bearer` as an invalid connection grant before dispatching anything
    /// upstream. Drop it so the next operation obtains a new one.
    pub async fn reject(&self, bearer: &str) {
        let mut state = self.state.lock().await;
        let Some(grant) = state.grant.as_ref().filter(|grant| grant.bearer == bearer) else {
            return;
        };
        let fresh = grant.issued_at.elapsed() < MIN_GRANT_AGE_FOR_REJECTION;
        state.grant = None;
        if fresh {
            Self::back_off(&mut state, Failure::Rejected(401), Duration::ZERO);
        }
    }

    fn back_off(state: &mut State, failure: Failure, retry_after: Duration) {
        state.failures = state.failures.saturating_add(1);
        let delay = (Duration::from_secs(1u64 << state.failures.min(6)) + jitter(1000))
            .min(Duration::from_secs(60));
        state.retry_at = Some(Instant::now() + delay.max(retry_after));
        state.failure = Some(failure);
    }

    /// Introspects a new grant and holds the workspace to the first installation and
    /// organization the service confirmed for this identity, in memory and in the database.
    async fn confirm(
        &self,
        bearer: &str,
        binding: &mut Option<(String, String)>,
    ) -> Result<(), (Failure, Duration)> {
        let confirmed = self.introspect(bearer).await?;
        if binding.as_ref().is_some_and(|bound| *bound != confirmed) {
            return Err((Failure::BindingMismatch, Duration::ZERO));
        }
        if let Some(database) = &self.database {
            let persisted = self
                .persist_binding(database, &confirmed)
                .await
                .map_err(|()| (Failure::BindingUnavailable, Duration::ZERO))?;
            if persisted != confirmed {
                return Err((Failure::BindingMismatch, Duration::ZERO));
            }
        }
        *binding = Some(confirmed);
        Ok(())
    }

    async fn persist_binding(
        &self,
        database: &DatabaseConnection,
        (installation, organization): &(String, String),
    ) -> Result<(String, String), ()> {
        let uuid = uuid::Uuid::parse_str(&self.identity.uuid).map_err(|_| ())?;
        let row = grengin_service_bindings::ActiveModel {
            instance_uuid: Set(uuid),
            service_origin: Set(self.identity.origin.to_owned()),
            installation_id: Set(installation.clone()),
            organization_id: Set(organization.clone()),
            created_at: Set(chrono::Utc::now()),
        };
        grengin_service_bindings::Entity::insert(row)
            .on_conflict(
                OnConflict::column(grengin_service_bindings::Column::InstanceUuid)
                    .do_nothing()
                    .to_owned(),
            )
            .exec_without_returning(database)
            .await
            .map_err(|_| ())?;
        let stored = grengin_service_bindings::Entity::find_by_id(uuid)
            .one(database)
            .await
            .map_err(|_| ())?
            .ok_or(())?;
        if stored.service_origin != self.identity.origin {
            return Ok((String::new(), String::new()));
        }
        Ok((stored.installation_id, stored.organization_id))
    }

    async fn introspect(&self, bearer: &str) -> Result<(String, String), (Failure, Duration)> {
        let proof = self
            .identity
            .proof(INTROSPECT_DOMAIN)
            .map_err(|e| (Failure::Setup(e), Duration::ZERO))?;
        let response = self
            .client
            .post(&self.introspect_url)
            .bearer_auth(bearer)
            .json(&proof)
            .send()
            .await
            .map_err(|_| (Failure::Transport, Duration::ZERO))?;
        let status = response.status();
        if !status.is_success() {
            return Err((Failure::Rejected(status.as_u16()), retry_after(&response)));
        }
        let bytes = read_body_capped(response, MAX_BYTES)
            .await
            .map_err(|_| (Failure::Response, Duration::ZERO))?;
        #[derive(Deserialize)]
        struct Introspection {
            active: bool,
            status: String,
            installation_id: Option<String>,
            organization_id: Option<String>,
        }
        let value: Introspection =
            serde_json::from_slice(&bytes).map_err(|_| (Failure::Response, Duration::ZERO))?;
        if !value.active || value.status != "connected" {
            return Err((Failure::Revoked, Duration::ZERO));
        }
        let valid = |id: &Option<String>| {
            id.as_deref().is_some_and(|id| {
                !id.is_empty()
                    && id.len() <= 128
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            })
        };
        match (value.installation_id, value.organization_id) {
            (installation, organization) if valid(&installation) && valid(&organization) => Ok((
                installation.unwrap_or_default(),
                organization.unwrap_or_default(),
            )),
            _ => Err((Failure::Response, Duration::ZERO)),
        }
    }

    async fn refresh(&self) -> Result<Cached, (Failure, Duration)> {
        let started = Instant::now();
        let proof = self
            .identity
            .proof(REFRESH_DOMAIN)
            .map_err(|e| (Failure::Setup(e), Duration::ZERO))?;
        let response = self
            .client
            .post(&self.refresh_url)
            .json(&proof)
            .send()
            .await
            .map_err(|_| (Failure::Transport, Duration::ZERO))?;
        let status = response.status();
        if !status.is_success() {
            return Err((Failure::Rejected(status.as_u16()), retry_after(&response)));
        }
        let bytes = read_body_capped(response, MAX_BYTES)
            .await
            .map_err(|error| {
                let failure = if error.downcast_ref::<reqwest::Error>().is_some() {
                    Failure::Transport
                } else {
                    Failure::Response
                };
                (failure, Duration::ZERO)
            })?;
        #[derive(Deserialize)]
        struct Response {
            connection_grant: String,
            expires_in: u64,
        }
        let value: Response =
            serde_json::from_slice(&bytes).map_err(|_| (Failure::Response, Duration::ZERO))?;
        if !(1..=3600).contains(&value.expires_in)
            || value.connection_grant.is_empty()
            || value.connection_grant.len() > 8192
            || !value
                .connection_grant
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
        {
            return Err((Failure::Response, Duration::ZERO));
        }
        let expires_at = started + Duration::from_secs(value.expires_in);
        if Instant::now() >= expires_at {
            return Err((Failure::Response, Duration::ZERO));
        }
        let margin = (Duration::from_secs(300) + jitter(60_000))
            .min(Duration::from_secs(value.expires_in / 2));
        Ok(Cached {
            bearer: value.connection_grant,
            issued_at: Instant::now(),
            refresh_at: expires_at - margin,
            expires_at,
        })
    }
}

fn jitter(max_ms: u32) -> Duration {
    let mut bytes = [0u8; 4];
    let _ = openssl::rand::rand_bytes(&mut bytes);
    Duration::from_millis((u32::from_ne_bytes(bytes) % max_ms) as u64)
}

fn retry_after(response: &reqwest::Response) -> Duration {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(parse_retry_after)
        .unwrap_or_default()
}

fn parse_retry_after(value: &str) -> Duration {
    let seconds = value
        .parse::<u64>()
        .ok()
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|when| (when.timestamp() - chrono::Utc::now().timestamp()).max(0) as u64)
        })
        .unwrap_or(0);
    Duration::from_secs(seconds.min(86_400))
}

pub const GRENGIN_CONSENT_VERSION: &str = "grengin-models-v1";

pub struct WorkspaceStatus {
    pub engine_state: &'static str,
    pub installation_id: Option<String>,
    pub workspace_id: Option<String>,
    pub balance_microdollars: Option<i64>,
    pub available_microdollars: Option<i64>,
    pub low_balance_microdollars: Option<i64>,
    pub balance_url: Option<String>,
    pub top_up_url: Option<String>,
}

fn service_error_type(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let kind = value.pointer("/error/type")?.as_str()?;
    (kind.len() <= 64 && kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'))
        .then(|| kind.to_owned())
}

fn workspace_document(bytes: &[u8]) -> Result<WorkspaceStatus, ProviderError> {
    #[derive(Deserialize)]
    struct Document {
        installation_id: Option<String>,
        workspace_id: Option<String>,
        balance_microdollars: Option<i64>,
        available_microdollars: Option<i64>,
        low_balance_microdollars: Option<i64>,
        balance_url: Option<String>,
        top_up_url: Option<String>,
    }
    let document: Document = serde_json::from_slice(bytes)
        .map_err(|_| ProviderError::ResponseMapping("invalid Grengin workspace status".into()))?;
    let https =
        |url: Option<String>| url.filter(|url| url.starts_with("https://") && url.len() <= 2048);
    Ok(WorkspaceStatus {
        engine_state: workspace_status(bytes)?,
        installation_id: document.installation_id,
        workspace_id: document.workspace_id,
        balance_microdollars: document.balance_microdollars,
        available_microdollars: document.available_microdollars,
        low_balance_microdollars: document.low_balance_microdollars,
        balance_url: https(document.balance_url),
        top_up_url: https(document.top_up_url),
    })
}

fn workspace_status(bytes: &[u8]) -> Result<&'static str, ProviderError> {
    #[derive(Deserialize)]
    struct Status {
        engine_state: String,
    }
    let status: Status = serde_json::from_slice(bytes)
        .map_err(|_| ProviderError::ResponseMapping("invalid Grengin workspace status".into()))?;
    match status.engine_state.as_str() {
        "on" => Ok("on"),
        "low" => Ok("low"),
        "off" => Ok("off"),
        "pending" => Ok("pending"),
        "empty" => Ok("empty"),
        _ => Err(ProviderError::Configuration(
            "Grengin workspace connection is not ready".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::post,
    };
    use foreign_types::ForeignType;
    use openssl::{pkey_ctx::PkeyCtx, sign::Verifier};
    use serde_json::{Value, json};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn status_proves_connection_without_claiming_model_access() {
        assert_eq!(
            workspace_status(br#"{"engine_state":"off"}"#).unwrap(),
            "off"
        );
        assert_eq!(
            workspace_status(br#"{"engine_state":"empty"}"#).unwrap(),
            "empty"
        );
        for response in [
            br#"{"enabled":true}"#.as_slice(),
            br#"{"engine_state":"disconnected"}"#,
            br#"{"engine_state":"unknown"}"#,
        ] {
            assert!(workspace_status(response).is_err());
        }
    }

    fn document() -> Value {
        let ed = PKey::generate_ed25519().unwrap();
        let pointer = unsafe {
            openssl_sys::EVP_PKEY_CTX_new_from_name(
                std::ptr::null_mut(),
                c"ML-DSA-65".as_ptr(),
                std::ptr::null(),
            )
        };
        assert!(!pointer.is_null());
        let mut context: PkeyCtx<()> = unsafe { PkeyCtx::from_ptr(pointer) };
        context.keygen_init().unwrap();
        let ml = context.keygen().unwrap();
        json!({"schema_version":1,"mode":"standalone","environment":"staging",
            "service_origin":"https://service-staging.grengin.com","instance_uuid":uuid::Uuid::new_v4().to_string(),"workspace_id":null,
            "identity":{"ed25519_private_key_pkcs8_base64":STANDARD.encode(ed.private_key_to_pkcs8().unwrap()),
                "ml_dsa65_private_key_pkcs8_base64":STANDARD.encode(ml.private_key_to_pkcs8().unwrap()),
                "public_key":hex(&ed.raw_public_key().unwrap()),"ml_dsa_public_key":hex(&ml.raw_public_key().unwrap())}})
    }

    #[test]
    #[ignore = "requires the website Worker checkout, workerd and loopback access"]
    fn rust_refresh_is_accepted_by_the_website_worker() {
        use std::{
            io::Write,
            process::{Command, Stdio},
        };
        let doc = document();
        let identity = Identity::parse(&serde_json::to_vec(&doc).unwrap()).unwrap();
        let fixture = json!({
            "proof": identity.proof(REFRESH_DOMAIN).unwrap(),
            "publicKey": doc["identity"]["public_key"],
            "mlDsaPublicKey": doc["identity"]["ml_dsa_public_key"],
        });
        let mut child = Command::new("node")
            .arg("tests/ai-engines/grengin-refresh.mjs")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&fixture).unwrap())
            .unwrap();
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn identity_rejects_wrong_metadata_keys_and_unknown_or_missing_fields() {
        let document = document();
        assert!(Identity::parse(&serde_json::to_vec(&document).unwrap()).is_ok());
        for (pointer, value) in [
            ("/schema_version", json!(2)),
            ("/environment", json!("prod")),
            (
                "/service_origin",
                json!("https://service-staging.grengin.com/"),
            ),
            ("/instance_uuid", json!(uuid::Uuid::nil().to_string())),
            ("/workspace_id", json!("cw_unexpected")),
            ("/identity/public_key", json!("00".repeat(32))),
            (
                "/identity/ml_dsa65_private_key_pkcs8_base64",
                document["identity"]["ed25519_private_key_pkcs8_base64"].clone(),
            ),
        ] {
            let mut changed = document.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert!(Identity::parse(&serde_json::to_vec(&changed).unwrap()).is_err());
        }
        let mut changed = document.clone();
        changed.as_object_mut().unwrap().remove("workspace_id");
        assert!(Identity::parse(&serde_json::to_vec(&changed).unwrap()).is_err());
        let mut changed = document;
        changed["unknown"] = json!(true);
        assert!(Identity::parse(&serde_json::to_vec(&changed).unwrap()).is_err());
        assert!(Identity::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn identity_file_requires_0600_and_rejects_symlinks() {
        use std::{
            io::Write,
            os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
        };
        let path = std::env::temp_dir().join(format!("grengin-grant-{}", uuid::Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        file.write_all(&serde_json::to_vec(&document()).unwrap())
            .unwrap();
        assert!(Identity::load(&path).is_ok());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Identity::load(&path).is_err());
        let link = path.with_extension("link");
        symlink(&path, &link).unwrap();
        assert!(Identity::load(&link).is_err());
        assert!(Identity::load(Path::new("relative.json")).is_err());
        std::fs::remove_file(link).unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[tokio::test]
    async fn control_requests_drop_a_rejected_grant_and_reauthenticate_once() {
        let bytes = serde_json::to_vec(&document()).unwrap();
        let issued = Arc::new(AtomicUsize::new(0));
        let refresh_count = issued.clone();
        let refresh = post(move || {
            let issued = refresh_count.clone();
            async move {
                let n = issued.fetch_add(1, Ordering::SeqCst) + 1;
                Json(json!({"connection_grant": format!("token-{n}"), "expires_in": 3600}))
            }
        });
        let introspect = post(|| async {
            Json(
                json!({"active":true,"status":"connected","installation_id":"inst_a","organization_id":"org_a"}),
            )
        });
        let reject_all = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rejecting = reject_all.clone();
        let rejected = || {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":{"type":"invalid_connection_grant"}})),
            )
        };
        let status = axum::routing::get(move |headers: HeaderMap| {
            let rejecting = rejecting.clone();
            async move {
                let bearer = headers
                    .get("authorization")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                if bearer == "Bearer token-1" || rejecting.load(Ordering::SeqCst) {
                    return rejected().into_response();
                }
                Json(json!({"engine_state":"empty","workspace_id":"cw_a","installation_id":"inst_a","available_microdollars":0})).into_response()
            }
        });
        let models = axum::routing::put(move |headers: HeaderMap| async move {
            let bearer = headers
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            if bearer == "Bearer token-2" {
                return rejected().into_response();
            }
            Json(json!({"enabled":true})).into_response()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let mut grant = GrenginGrant::new(Identity::parse(&bytes).unwrap(), None).unwrap();
        grant.refresh_url = format!("{origin}{REFRESH_PATH}");
        grant.introspect_url = format!("{origin}{INTROSPECT_PATH}");
        grant.service_origin = origin.clone();
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route(REFRESH_PATH, refresh)
                    .route(INTROSPECT_PATH, introspect)
                    .route("/v1/infer/status", status)
                    .route("/v1/connect/grengin-models", models),
            )
            .into_future(),
        );
        async fn make_old(grant: &GrenginGrant) {
            grant.state.lock().await.grant.as_mut().unwrap().issued_at =
                Instant::now() - Duration::from_secs(600);
        }

        // A cached grant the service has invalidated (for example after rotation).
        assert_eq!(grant.bearer().await.unwrap(), "token-1");
        make_old(&grant).await;
        grant.identity.workspace_id = Some("cw_a".into());
        let workspace = grant.workspace().await.unwrap();
        assert_eq!(workspace.engine_state, "empty");
        assert_eq!(workspace.available_microdollars, Some(0));
        grant.identity.workspace_id = Some("cw_other".into());
        assert!(grant.status().await.is_err());
        grant.identity.workspace_id = Some("cw_a".into());
        grant.state.lock().await.binding = Some(("inst_other".into(), "org_a".into()));
        assert!(grant.status().await.is_err());
        grant.state.lock().await.binding = Some(("inst_a".into(), "org_a".into()));
        assert_eq!(issued.load(Ordering::SeqCst), 2);
        assert_eq!(grant.bearer().await.unwrap(), "token-2");

        make_old(&grant).await;
        grant.set_models_enabled(true).await.unwrap();
        assert_eq!(issued.load(Ordering::SeqCst), 3);
        assert_eq!(grant.bearer().await.unwrap(), "token-3");

        // Refused again straight after issue: one bounded attempt, then backoff, no loop.
        reject_all.store(true, Ordering::SeqCst);
        make_old(&grant).await;
        assert!(grant.status().await.is_err());
        assert_eq!(issued.load(Ordering::SeqCst), 4);
        assert!(grant.status().await.is_err());
        assert_eq!(issued.load(Ordering::SeqCst), 4);
        server.abort();
    }

    #[tokio::test]
    async fn signed_refresh_coalesces_and_respects_expiry_backoff_and_revocation() {
        let bytes = serde_json::to_vec(&document()).unwrap();
        let verifier = Arc::new(Identity::parse(&bytes).unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let handler = post(move |headers: HeaderMap, Json(proof): Json<Proof>| {
            let verifier = verifier.clone();
            let count = count.clone();
            async move {
                assert!(headers.get("authorization").is_none());
                assert_eq!(proof.instance_uuid, verifier.uuid);
                assert_eq!(proof.nonce.len(), 64);
                let message = format!(
                    "grengin-connect-v2\n{}\n{}\n{}",
                    proof.instance_uuid, proof.timestamp, proof.nonce
                );
                for (key, signature) in [
                    (&verifier.ed, proof.signature),
                    (&verifier.ml, proof.ml_dsa_signature),
                ] {
                    let signature: Vec<_> = signature
                        .as_bytes()
                        .chunks_exact(2)
                        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                        .collect();
                    assert!(
                        Verifier::new_without_digest(key)
                            .unwrap()
                            .verify_oneshot(&signature, message.as_bytes())
                            .unwrap()
                    );
                }
                let (status, body) = match count.fetch_add(1, Ordering::SeqCst) {
                    0 => (
                        StatusCode::OK,
                        json!({"connection_grant":"opaque.token","expires_in":3600}),
                    ),
                    1 => (
                        StatusCode::SERVICE_UNAVAILABLE,
                        json!({"secret":"must not escape"}),
                    ),
                    2 => (
                        StatusCode::FORBIDDEN,
                        json!({"error":{"type":"workspace_disconnected"}}),
                    ),
                    // Registration completed later: the same process recovers without a restart.
                    3 => (
                        StatusCode::OK,
                        json!({"connection_grant":"second.token","expires_in":3600}),
                    ),
                    4 => (
                        StatusCode::OK,
                        json!({"connection_grant":"third.token","expires_in":3600}),
                    ),
                    5 => (
                        StatusCode::OK,
                        json!({"connection_grant":"moved.token","expires_in":3600}),
                    ),
                    _ => panic!("unexpected refresh"),
                };
                (status, [("retry-after", "120")], Json(body))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let introspection_verifier = Arc::new(Identity::parse(&bytes).unwrap());
        let introspect = post(move |headers: HeaderMap, Json(proof): Json<Proof>| {
            let verifier = introspection_verifier.clone();
            async move {
                let bearer = headers
                    .get("authorization")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned();
                let message = format!(
                    "{INTROSPECT_DOMAIN}\n{}\n{}\n{}",
                    proof.instance_uuid, proof.timestamp, proof.nonce
                );
                let signature: Vec<_> = proof
                    .ml_dsa_signature
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                    .collect();
                assert!(
                    Verifier::new_without_digest(&verifier.ml)
                        .unwrap()
                        .verify_oneshot(&signature, message.as_bytes())
                        .unwrap()
                );
                let installation = if bearer == "Bearer moved.token" {
                    "inst_other"
                } else {
                    "inst_a"
                };
                Json(json!({"active":true,"status":"connected",
                    "installation_id":installation,"organization_id":"org_a"}))
            }
        });
        let mut grant = GrenginGrant::new(Identity::parse(&bytes).unwrap(), None).unwrap();
        // Local transport injection exists only in tests.
        grant.refresh_url = format!("http://{}{REFRESH_PATH}", listener.local_addr().unwrap());
        grant.introspect_url =
            format!("http://{}{INTROSPECT_PATH}", listener.local_addr().unwrap());
        let status_url = format!("http://{}/v1/infer/status", listener.local_addr().unwrap());
        let status = axum::routing::get(|headers: HeaderMap| async move {
            assert_eq!(headers.get("authorization").unwrap(), "Bearer opaque.token");
            assert!(headers.get("x-api-key").is_none());
            Json(json!({"engine_state":"off"}))
        });
        let server = tokio::spawn(
            axum::serve(
                listener,
                Router::new()
                    .route(REFRESH_PATH, handler)
                    .route(INTROSPECT_PATH, introspect)
                    .route("/v1/infer/status", status),
            )
            .into_future(),
        );
        let grant = Arc::new(grant);
        let results = futures_util::future::join_all((0..20).map(|_| grant.bearer())).await;
        assert!(results.into_iter().all(|r| r.unwrap() == "opaque.token"));
        assert_eq!(
            grant.status_at(&status_url).await.unwrap().engine_state,
            "off"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        {
            let mut state = grant.state.lock().await;
            let cache = state.grant.as_mut().unwrap();
            assert!(
                (300..=360).contains(&cache.expires_at.duration_since(cache.refresh_at).as_secs())
            );
            cache.refresh_at = Instant::now() - Duration::from_secs(1);
        }
        assert_eq!(grant.bearer().await.unwrap(), "opaque.token");
        {
            let mut state = grant.state.lock().await;
            assert!(
                state.retry_at.unwrap().duration_since(Instant::now()) > Duration::from_secs(119)
            );
            state.grant.as_mut().unwrap().expires_at = Instant::now() - Duration::from_secs(1);
        }
        assert!(
            !grant
                .bearer()
                .await
                .unwrap_err()
                .to_string()
                .contains("must not escape")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        grant.state.lock().await.retry_at = None;
        assert!(matches!(
            grant.bearer().await,
            Err(ProviderError::HttpStatus { status: 403, .. })
        ));
        assert!(grant.state.lock().await.grant.is_none());
        // Fails closed during the bounded backoff, without another request.
        assert!(grant.bearer().await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        {
            let state = grant.state.lock().await;
            assert!(
                state.retry_at.unwrap().duration_since(Instant::now()) <= Duration::from_secs(121)
            );
        }
        grant.state.lock().await.retry_at = None;
        assert_eq!(grant.bearer().await.unwrap(), "second.token");
        assert_eq!(
            grant.state.lock().await.binding,
            Some(("inst_a".into(), "org_a".into()))
        );

        // A grant the gateway rejects is dropped; the next operation gets a new one.
        grant.state.lock().await.grant.as_mut().unwrap().issued_at =
            Instant::now() - Duration::from_secs(10);
        grant.reject("not.the.current.token").await;
        assert_eq!(grant.bearer().await.unwrap(), "second.token");
        grant.reject("second.token").await;
        assert_eq!(grant.bearer().await.unwrap(), "third.token");
        assert_eq!(calls.load(Ordering::SeqCst), 5);

        // Rejected again straight after issue: back off instead of refreshing on every request.
        grant.reject("third.token").await;
        assert!(grant.bearer().await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 5);

        // The service now reports another installation for this identity: refused.
        grant.state.lock().await.retry_at = None;
        assert!(matches!(
            grant.bearer().await,
            Err(ProviderError::Configuration(message)) if message.contains("different service installation")
        ));
        assert!(grant.state.lock().await.grant.is_none());
        server.abort();
    }

    #[tokio::test]
    #[ignore = "needs GRENGIN_SECURITY_TEST_DATABASE_URL (disposable, migrated database)"]
    async fn the_first_confirmed_binding_is_persisted_and_never_replaced() {
        let url = std::env::var("GRENGIN_SECURITY_TEST_DATABASE_URL").expect("test database URL");
        let database = sea_orm::Database::connect(&url)
            .await
            .expect("test database");
        let grant = GrenginGrant::new(
            Identity::parse(&serde_json::to_vec(&document()).unwrap()).unwrap(),
            Some(database.clone()),
        )
        .unwrap();
        let first = ("inst_a".to_owned(), "org_a".to_owned());
        assert_eq!(
            grant.persist_binding(&database, &first).await.unwrap(),
            first
        );
        // A later confirmation of another installation keeps the stored one, which confirm() refuses.
        let other = ("inst_b".to_owned(), "org_a".to_owned());
        assert_eq!(
            grant.persist_binding(&database, &other).await.unwrap(),
            first
        );
        // What a restarted process reads back: the first confirmed binding.
        let stored = grengin_service_bindings::Entity::find_by_id(
            uuid::Uuid::parse_str(&grant.identity.uuid).unwrap(),
        )
        .one(&database)
        .await
        .unwrap()
        .unwrap();
        assert_eq!((stored.installation_id, stored.organization_id), first);
        assert_eq!(stored.service_origin, grant.identity.origin);
    }

    #[test]
    fn a_managed_identity_runs_only_in_the_workspace_it_was_issued_for() {
        let mut managed = document();
        managed["mode"] = json!("managed_cloud");
        managed["workspace_id"] = json!("cw_alpha");
        let managed = Identity::parse(&serde_json::to_vec(&managed).unwrap()).unwrap();
        assert!(managed.check_workspace(Some("cw_alpha")).is_ok());
        assert!(managed.check_workspace(Some("cw_beta")).is_err());
        assert!(managed.check_workspace(None).is_err());
        let standalone = Identity::parse(&serde_json::to_vec(&document()).unwrap()).unwrap();
        assert!(standalone.check_workspace(None).is_ok());
        assert!(standalone.check_workspace(Some("cw_alpha")).is_err());
    }
}
