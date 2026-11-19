// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

//! Credentials for an already registered workspace. Keys are loaded once; grants stay in memory.
use super::{
    catalog_download::read_body_capped,
    connector_broker::{hex, random_secret},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use llm_plugin::ProviderError;
use openssl::{
    pkey::{Id, PKey, Private},
    sign::Signer,
};
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
            ed,
            ml,
        })
    }

    fn proof(&self) -> SetupResult<Proof> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "invalid system clock")?
            .as_secs();
        let nonce = random_secret().map_err(|_| "identity random generation failed")?;
        let message = format!("grengin-connect-v2\n{}\n{timestamp}\n{nonce}", self.uuid);
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
    refresh_at: Instant,
    expires_at: Instant,
}

#[derive(Clone, Copy)]
enum Failure {
    Setup(&'static str),
    Transport,
    Response,
    Rejected(u16),
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
        }
    }
    fn transient(self) -> bool {
        matches!(
            self,
            Self::Transport | Self::Rejected(408 | 409 | 425 | 429 | 500..=599)
        )
    }
    fn terminal(self) -> bool {
        matches!(self, Self::Setup(_) | Self::Rejected(400 | 401 | 403))
    }
}

#[derive(Default)]
struct State {
    grant: Option<Cached>,
    failure: Option<Failure>,
    retry_at: Option<Instant>,
    failures: u32,
}

pub struct GrenginGrant {
    identity: Identity,
    client: reqwest::Client,
    refresh_url: String,
    state: Mutex<State>,
}

fn setup_error(message: &'static str) -> ProviderError {
    ProviderError::Configuration(format!("Grengin setup required: {message}"))
}

impl GrenginGrant {
    /// One process-wide identity; deliberate key replacement requires a restart.
    pub fn configured() -> Result<&'static Self, ProviderError> {
        static GRANT: OnceLock<SetupResult<GrenginGrant>> = OnceLock::new();
        GRANT
            .get_or_init(|| {
                let path = std::env::var_os("GRENGIN_IDENTITY_FILE")
                    .ok_or("GRENGIN_IDENTITY_FILE is not configured")?;
                Self::new(Identity::load(Path::new(&path))?)
            })
            .as_ref()
            .map_err(|message| setup_error(message))
    }

    fn new(identity: Identity) -> SetupResult<Self> {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| "credential client unavailable")?;
        let refresh_url = format!("{}{REFRESH_PATH}", identity.origin);
        Ok(Self {
            identity,
            client,
            refresh_url,
            state: Mutex::new(State::default()),
        })
    }

    pub fn origin(&self) -> &'static str {
        self.identity.origin
    }

    /// Authenticated, non-billable connection check. The result describes remote model
    /// access; an accepted workspace identity can still have access switched off.
    pub async fn status(&self) -> Result<&'static str, ProviderError> {
        self.status_at(&format!("{}/v1/infer/status", self.origin()))
            .await
    }

    async fn status_at(&self, url: &str) -> Result<&'static str, ProviderError> {
        let response = self
            .client
            .get(url)
            .bearer_auth(self.bearer().await?)
            .send()
            .await
            .map_err(|_| ProviderError::Transport("Grengin workspace status unavailable".into()))?;
        if !response.status().is_success() {
            return Err(ProviderError::HttpStatus {
                status: response.status().as_u16(),
                message: "Grengin workspace status rejected".into(),
            });
        }
        let bytes = read_body_capped(response, MAX_BYTES).await.map_err(|_| {
            ProviderError::ResponseMapping("invalid Grengin workspace status".into())
        })?;
        workspace_status(&bytes)
    }

    /// Called at every native operation dispatch, including later tool rounds.
    pub async fn bearer(&self) -> Result<String, ProviderError> {
        let mut state = self.state.lock().await;
        let now = Instant::now();
        if let Some(failure) = state.failure.filter(|failure| failure.terminal()) {
            return Err(failure.error());
        }
        if let Some(grant) = &state.grant {
            if now < grant.refresh_at {
                return Ok(grant.bearer.clone());
            }
        }
        if state.retry_at.is_some_and(|retry| now < retry) {
            if let Some(grant) = &state.grant {
                if now < grant.expires_at {
                    return Ok(grant.bearer.clone());
                }
            }
            return Err(state.failure.unwrap_or(Failure::Transport).error());
        }
        // Keep the mutex only for this bounded control request, never for inference.
        match self.refresh().await {
            Ok(grant) => {
                let bearer = grant.bearer.clone();
                *state = State {
                    grant: Some(grant),
                    ..State::default()
                };
                Ok(bearer)
            }
            Err((failure, retry_after)) => {
                state.failures = state.failures.saturating_add(1);
                let delay = (Duration::from_secs(1u64 << state.failures.min(6)) + jitter(1000))
                    .min(Duration::from_secs(60));
                state.retry_at = Some(Instant::now() + delay.max(retry_after));
                state.failure = Some(failure);
                if failure.transient() {
                    if let Some(grant) = &state.grant {
                        if Instant::now() < grant.expires_at {
                            return Ok(grant.bearer.clone());
                        }
                    }
                }
                state.grant = None;
                Err(failure.error())
            }
        }
    }

    async fn refresh(&self) -> Result<Cached, (Failure, Duration)> {
        let started = Instant::now();
        let proof = self
            .identity
            .proof()
            .map_err(|e| (Failure::Setup(e), Duration::ZERO))?;
        let response = self
            .client
            .post(&self.refresh_url)
            .json(&proof)
            .send()
            .await
            .map_err(|_| (Failure::Transport, Duration::ZERO))?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(parse_retry_after)
            .unwrap_or_default();
        if !status.is_success() {
            return Err((Failure::Rejected(status.as_u16()), retry_after));
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
            "proof": identity.proof().unwrap(),
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
                    _ => panic!("unexpected refresh"),
                };
                (status, [("retry-after", "120")], Json(body))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut grant = GrenginGrant::new(Identity::parse(&bytes).unwrap()).unwrap();
        // Local transport injection exists only in tests.
        grant.refresh_url = format!("http://{}{REFRESH_PATH}", listener.local_addr().unwrap());
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
                    .route("/v1/infer/status", status),
            )
            .into_future(),
        );
        let grant = Arc::new(grant);
        let results = futures_util::future::join_all((0..20).map(|_| grant.bearer())).await;
        assert!(results.into_iter().all(|r| r.unwrap() == "opaque.token"));
        assert_eq!(grant.status_at(&status_url).await.unwrap(), "off");
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
        assert!(grant.bearer().await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        server.abort();
    }
}
