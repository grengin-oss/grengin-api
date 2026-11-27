# Grengin provider integration specification

Status: implementation specification for `grengin-api`; not an implementation or
deployment receipt. Baseline inspected on 2026-11-17: API `3cdf410`, website service
`71fffc7238e7dbe49e6e3128386e580d712ffef4`. Normative backend requirements below
describe work to implement. Sections explicitly labelled **existing service
contract** describe behavior already present in the website repository.

## 1. Outcome and ownership

A connected workspace can use the Grengin engine with its workspace AI balance.
The backend authenticates with its own durable Ed25519 and ML-DSA-65 identity,
obtains short-lived connection grants, and sends native provider requests through
the Grengin service. Users do not paste Grengin proxy tokens or shared upstream
provider keys into AI engine settings.

This specification owns the API runtime, its `llm-plugin` transport integration,
local administrative APIs, model selection, persistence and tests. It also fixes
the runtime secret interface that a later Hatchery specification must supply.
It does not implement AWS resources, Hatchery backfill, website billing, tenant UI
screens, or deployment. Complete the backend implementation and its contract tests
before writing the separate Hatchery document.

The existing [service architecture](../../grengin-website/docs/cloud-provisioning-framework.md#workspace-identity-and-inference)
provides the cross-repository contract. Where older prose differs, the service
sources linked below and this baseline take precedence. In particular, there is
**no runtime-version authorization gate**: `image_version` is metadata and
`minimum_version` is `null`. The old sentence about runtime-version checks in the
architecture guide must not be copied into implementation.

### Completed work and evidence limits

| Area | Baseline |
| --- | --- |
| Website service | Hybrid registration, Connect refresh/rotation, native inference gateway, workspace balances and allowances exist. |
| Rollout configuration | Global LLM, billing-purchase, billing-credit, cloud-provisioning and lifecycle rollout gates were removed, as was `LLM_MIN_WORKSPACE_VERSION`. Do not reintroduce equivalent enable flags. |
| Real controls | Identity, authorization, workspace consent, local model policy, balance, managed workspace lifecycle and network authorization still apply. |
| Staging funding | Operator receipts confirm eight grants applied: three normal trials and five explicitly approved staging test grants, each 550,700 USD microdollars. Preserve their expiry and receipt history. |
| API | Current provider runtime uses stored static credentials; durable hybrid identity, grant renewal and gateway routing are not implemented. |
| Hatchery | Current registration omits hybrid identity; durable key publication and existing-instance backfill are not implemented. |

Applied grants do not prove current remaining balances, provider credentials,
runtime deployment, registered keys, authorized egress, or successful inference.
Do not rerun funding or change trial eligibility in this work. Existing workspace
keys and credits remain operator-managed; missing managed identity must never
cause automatic replacement identity generation.

## 2. Implementation map

| Existing area | Required change |
| --- | --- |
| [`src/config/setting.rs`](../src/config/setting.rs), [`src/state.rs`](../src/state.rs) | Load and validate the identity file; own shared grant manager and connection state. |
| [`src/models/ai_engines.rs`](../src/models/ai_engines.rs), [`src/services/ai_plugin.rs`](../src/services/ai_plugin.rs) | Reserve engine key `grengin`; persist local policy without an API key or plugin manifest. |
| [`src/handlers/admin_ai.rs`](../src/handlers/admin_ai.rs), [`src/handlers/admin_ai_plugins.rs`](../src/handlers/admin_ai_plugins.rs), [`src/routes/admin.rs`](../src/routes/admin.rs), [`src/dto/admin_ai.rs`](../src/dto/admin_ai.rs) | Specialize Grengin installation/configuration; add safe status/catalog/pairing DTOs and permission checks. |
| [`src/services/provider_runtime.rs`](../src/services/provider_runtime.rs), [`src/services/provider_resolver.rs`](../src/services/provider_resolver.rs), [`src/services/provider_models.rs`](../src/services/provider_models.rs), [`src/handlers/models.rs`](../src/handlers/models.rs) | Model-aware engine selection, catalog adaptation, provenance and cache invalidation. |
| [`llm-plugin/src/runtime.rs`](../llm-plugin/src/runtime.rs) | Support an API-owned dynamic transport/auth hook for every outgoing Grengin operation and tool continuation. Preserve native serializers/parsers. |
| [`src/handlers/chat.rs`](../src/handlers/chat.rs), [`src/handlers/chat_stream.rs`](../src/handlers/chat_stream.rs), [`src/utils/llm_error.rs`](../src/utils/llm_error.rs) | Propagate cancellation, safe structured failures, request IDs and funding provenance. |
| [`src/services/notifications.rs`](../src/services/notifications.rs), migrations and API schema | Durable deduplicated connection/balance notifications, new local persistence, generated OpenAPI contracts. |

Introduce focused `grengin_identity`, `grengin_connection`, `grengin_catalog` and
`grengin_provider` modules rather than embedding this protocol in generic API-key
decryption. Existing direct-provider installation behavior in
[ai-engine-installation.md](ai-engine-installation.md) remains intact.

## 3. Runtime identity interface — new API-owned contract

### Configuration and custody

Add one setting, `GRENGIN_IDENTITY_FILE`: an absolute path to a protected JSON
file. Its presence supplies identity, not a rollout switch. Missing or invalid
identity makes only Grengin unavailable with a safe setup error; ordinary backend
startup, administration and separately configured direct providers remain usable.
Never silently fall back to another identity or service origin.

The format is `schema_version: 1` with exactly these fields:

```json
{
  "schema_version": 1,
  "mode": "managed_cloud",
  "environment": "staging",
  "service_origin": "https://service-staging.grengin.com",
  "instance_uuid": "01234567-89ab-4def-8123-456789abcdef",
  "workspace_id": "cw_example",
  "identity": {
    "ed25519_private_key_pkcs8_base64": "<standard base64 DER PKCS#8>",
    "ml_dsa65_private_key_pkcs8_base64": "<standard base64 DER PKCS#8>",
    "public_key": "<64 lowercase hex characters>",
    "ml_dsa_public_key": "<3904 lowercase hex characters>"
  }
}
```

- `mode` is `managed_cloud` or `standalone`; `environment` is `dev`, `staging`
  or `prod`. `workspace_id` is required for managed mode and `null` for standalone.
  UUID must be stable and valid UUIDv4. It is the instance UUID, not the website
  workspace ID, installation ID or organization ID.
- Private fields contain algorithm-identified PKCS#8 DER using standard padded
  base64, not PEM, an opaque Rust struct, or a JWT. Decode and validate the actual
  algorithms; derive both public keys and compare them byte-for-byte with the
  supplied raw public keys. Reject unknown schema/fields, malformed keys, mixed
  key pairs, oversized input above 64 KiB, or invalid origin/identity metadata.
- Use independent CSPRNG-generated keys, pure Ed25519 and pure ML-DSA-65 with the
  empty signing context, compatible with the service verifier. Never derive them
  from `APP_KEY`, `JWT_SECRET`, a provider key or each other. Prove private-format
  round trips and cross-language signatures in tests before freezing an exporter.
- Accept production `https://service.grengin.com` and staging
  `https://service-staging.grengin.com`; environment `dev` also uses the staging
  service for shared-cloud tests. Reject mixed production/staging metadata,
  paths, queries, fragments and embedded credentials. Test-only injected local
  transports must not become a deployed arbitrary-origin setting.
- Managed metadata must match any existing immutable local claim/instance
  binding. Persist the first confirmed service installation/organization IDs
  bound to this UUID and origin; refuse silent reassignment on later refreshes.
  The service remains authoritative for billing organization identity.
- Keep the durable private copy in per-workspace protected storage. A launch-time
  private file may be materialized from that durable secret, but ephemeral Lambda
  disk is never the only copy. Use owner-only access (0600 on Unix), no secret
  contents in environment variables, logs, telemetry, receipts, SQL or browser
  responses. Redact debug output of key and grant types.
- Load once at startup; a restart reads a deliberately published new version.
  Do not rotate or regenerate on restart, parsing failure, refresh failure, grant
  expiry or connection revocation. Coordinated rotation is described below.

Implement an operator-only `grengin-identity` binary with `generate` and `inspect`
subcommands. The generate interface is `generate --mode <mode> --environment <env>
--service-origin <origin> --instance-uuid <uuid> --output <path>`, with required
`--workspace-id <id>` in managed mode and no workspace flag in standalone mode.
It creates the file exclusively and refuses to overwrite. `inspect --file <path>` validates
the file and prints only schema/mode/environment/origin/IDs, public keys and their
SHA-256 fingerprints. Neither command contacts the service or grants credits.
An explicit invocation is the setup action; server startup must not run it.

Shared upstream provider keys, `INSTANCE_CONNECTION_SECRET`,
`HATCHERY_CONNECTION_SECRET`, origin credentials and website user sessions are
not fields in this identity file and are not needed by the inference runtime.

### Managed registration dependency

**Existing service contract:** Hatchery sends an HMAC-signed
`PUT /v1/cloud/internal/instances/:workspace_id` with its existing exact `binding`
and unchanged `origin_credential`, plus:

```json
{"identity":{"public_key":"<Ed25519 hex>","ml_dsa_public_key":"<ML-DSA-65 hex>"}}
```

Identity is required on every PUT. An existing missing identity can be filled;
the same keys are retry-safe; different registered keys return `identity_conflict`.
Registration GET does not return identity fingerprints or prove key possession.
Do not bootstrap a managed missing identity through `/v1/connect/keys` or pairing.

The future Hatchery work must durably generate/publish the same envelope, register
its public keys from the regional network, and supply the file to the backend.
Registration PUT records the network egress IP used for managed inference. A
laptop or CloudShell registration can enroll the wrong address. The API must not
forge forwarding headers, request the Hatchery signing secret, or manage NAT.

Source: [cloud/instances.ts](../../grengin-website/workers/src/cloud/instances.ts)
and [llm-key-network.ts](../../grengin-website/workers/src/services/llm-key-network.ts).

## 4. Connection client — existing service contract

All paths below are relative to the validated `service_origin`. Send JSON over
HTTPS, reject redirects, bound responses, and never forward browser credentials.
Identity proofs use integer Unix **seconds** within 300 seconds of service time.
Generate a fresh 32-byte random nonce encoded as 64 lowercase hex characters.
Normalize UUID/nonce/public hex before constructing signed bytes.

Wire lengths: Ed25519 public key 64 hex, signature 128 hex; ML-DSA-65 public key
3,904 hex, signature 6,618 hex. Both signatures cover identical UTF-8 bytes, with
literal LF separators and **no trailing newline**. Do not prehash the message.

### Refresh and grant lifecycle

`POST /v1/connect/refresh` has no bearer token or refresh token. Body:

```json
{"instance_uuid":"<uuid>","timestamp":1794864565,"nonce":"<hex>","signature":"<Ed25519 signature>","ml_dsa_signature":"<ML-DSA signature>"}
```

Both keys sign:

```text
grengin-connect-v2
<instance_uuid>
<timestamp>
<nonce>
```

Response: `{"connection_grant":"<opaque bearer>","expires_in":3600}`.
It does not return installation ID, organization ID or key version separately.
The central service signs HS256 grants; the API must not receive its signing
secret. Treat the grant as opaque and obtain confirmed IDs through introspection.
Do not use locally decoded JWT claims as an authorization authority.

Required runtime behavior:

1. Coalesce concurrent refreshes per identity in one shared asynchronous operation.
   One process has one grant manager; do not hold a lock for the inference stream.
2. Record expiry conservatively from refresh request start plus `expires_in`.
   Refresh 300–360 seconds before expiry using jitter. Every outgoing operation,
   including later tool rounds, checks freshness; do not depend on timers firing
   while a Lambda execution environment is frozen. Never dispatch an expired grant.
3. Keep grants only in server memory. Restart obtains a new grant using the same
   durable keys. Do not persist them in `ai_engines.api_key`, browser storage,
   chat records, SQL logs or provider manifests.
4. Use a five-second timeout for control-plane requests, exponential backoff with
   jitter capped at 60 seconds, and honor `Retry-After`. Never block unrelated
   direct-provider startup on the service. A still-valid grant may remain usable
   after a transient refresh transport failure, but stop on confirmed revocation
   or invalid identity. After expiry fail clearly, without a busy retry loop.
5. Retry refresh with a new timestamp/nonce. Errors include 400
   `invalid_refresh_proof`, 401 `invalid_refresh_signature`, 403
   `workspace_disconnected`, 409 `refresh_proof_replayed`, and 429 `rate_limited`.
   The refresh/key-change limit is shared at 30/minute/source IP; avoid per-request
   refreshes and synchronized fleet refreshes.
6. For inference, allow at most one refresh-and-retry after an explicit gateway
   `invalid_connection_grant` response received before any stream data. That
   gateway rejection occurs before upstream dispatch. Do not retry arbitrary 401,
   network failure, timeout, partial stream or `provider_error` as token expiry.

### Introspection and synchronization

`POST /v1/connect/introspect` requires the current bearer and the same proof fields,
using domain `grengin-introspect-v2`. UUID must match the grant. Response is either
`{active:true,status:"connected",installation_id,organization_id}` or
`{active:false,status:"revoked"}`. A successful HTTP status alone is insufficient.
Reject a mismatched previously bound installation/organization, even if active.

`POST /v1/connect/sync` requires the bearer, with optional `domain`, `provider`
(`aws|azure|unknown`), `region`, `public_ip` (IPv4 only), `sso_proxy_enabled`, and
`image_version`. Send the actual backend version as `major.minor.patch`; omit
unowned metadata rather than overwriting it. Image versions cannot contain
prerelease/build suffixes. Response is
`{status:"connected",installation_id,ai_balance?}`. `ai_balance` is optional even
after successful sync; its omission must not be converted into a zero balance.

Introspect after a newly acquired grant and synchronize after startup/upgrade and
at most once per five minutes while active. Cache successful identity confirmation
with that grant; inference must not require refresh → introspect → sync → status
before every request. Service inference authorization still checks current state
on every call. Sync can return 403 `instance_not_connected` after a race with
rotation/revocation. None of these calls enroll a managed IP address.

### Standalone pairing

Pair only on explicit administrator action using an existing durable standalone
identity. Managed mode rejects local pairing attempts. The backend performs:

| Request | Contract |
| --- | --- |
| `POST /v1/connect/start` | Send `instance_uuid`, `public_key`, `ml_dsa_public_key`, name, environment, provider, image_version and `consent_version:"instance-connect-v1"`. Response 201 contains `device_code`, `user_code`, `verification_uri`, `verification_uri_complete`, `expires_in:600`, `interval:5`. |
| Website owner approval | Occurs on the website using the owner's session. New pairing creates an isolated organization; do not assume it is an existing account organization. |
| `POST /v1/connect/poll` | Send `{device_code}` server-side. Pending 202 `{status:"pending",interval:5}`; expired 410; denied 403; approved 200 contains installation/organization IDs, a grant, expiry and account summary. |
| Hybrid refresh | Always perform `/refresh` after approval. The polling grant is Connect-only and has no identity key version, even when both public keys were submitted. It cannot authorize hybrid inference, introspection or sync. |

Keep `device_code` server-side, encrypt it if persisted, and stop polling at expiry
or approval. Browser DTOs may include the user code, verification URLs, expiry and
status, never the device code or grant. Respect the five-second polling interval.
Current service verification and billing URLs are rooted at `https://grengin.com`
even when the service origin is staging. Preserve returned URLs; do not assume
staging browser pairing/billing links have been validated or silently rewrite them
in the backend. Record any environment-link failure as a separate website issue.

### Rotation, disconnection and recovery

`POST /v1/connect/keys` is proof-only, with `instance_uuid`, `timestamp`, `nonce`,
replacement `public_key`, replacement `ml_dsa_public_key`, current `signature` and
`ml_dsa_signature`, plus `new_signature` and `new_ml_dsa_signature`. All four sign:

```text
grengin-connect-keys-v2
<instance_uuid>
<timestamp>
<nonce>
<replacement Ed25519 public key>
<replacement ML-DSA public key>
```

Response is `{identity_key_version:<new version>}`; obtain a new grant with the
replacement keys. Old grants become invalid. Existing hybrid identities cannot
downgrade. A legacy standalone upgrade retains its Ed25519 key and may omit only
the current ML-DSA signature once; it still proves both new keys. Do not implement
legacy signed inference as a compatibility fallback.

Implement signing/rotation primitives and an explicit resumable standalone
operator operation in `grengin-identity`: `rotate prepare --file <identity>
--operation-dir <private durable directory>`, `rotate apply --operation-dir <dir>`
and `rotate verify --operation-dir <dir>`. Prepare makes no remote call; apply
submits the proof; verify reconciles before atomically publishing the accepted
envelope. These commands refuse managed mode with
`managed_rotation_requires_orchestrator`; the later Hatchery integration uses the
same tested primitives with coordinated secret publication. No automatic
scheduled rotation. Before sending a rotation, durably retain both old
and pending-new envelopes and operation state. On a lost response, probe refresh
with pending keys first; successful possession proves the new identity was
accepted. Promote only after confirmation. If pending fails with an explicit
signature rejection and old still authenticates, re-read the operation and retry
with a fresh proof. Network uncertainty never justifies overwriting either copy.
Managed rollout must coordinate all runtime copies and durable publication; do
not expose managed key rotation through a tenant browser in this increment.

The private operation directory contains `operation.json`, `old-identity.json`
and `pending-identity.json`, all owner-only. The journal records schema version,
operation ID, canonical identity-file path, UUID/origin, old/new public
fingerprints, state (`prepared|submitted|confirmed|published|complete`) and the
last safe error; it contains no grant or serialized proof. Fsync the envelopes
and journal before submission. Publish the confirmed envelope using an atomic
same-filesystem rename and fsync the directory; retain recovery copies until the
operation is complete. Because runtimes load identity once, publication requires
draining/restarting all old-key runtime copies. Report published-but-restart-needed
until a post-restart refresh proves adoption. Managed rotation is not complete
until its canonical durable secret, not just a materialized temporary file, and
all runtime copies have advanced through the future orchestrator.

There is **no grant-authenticated `/v1/connect/disconnect`**. Website
`DELETE /v1/installations/:id` uses a website user session and organization access;
managed instances use workspace deletion instead. Local engine disable/delete
does not revoke the workspace identity. Do not invent a remote disconnect API,
store a user's website session, or resurrect a revoked registration.

Sources: [connection.ts](../../grengin-website/workers/src/handlers/connection.ts),
[connection-grant.ts](../../grengin-website/workers/src/services/connection-grant.ts),
[workspace-identity.ts](../../grengin-website/workers/src/services/workspace-identity.ts).

## 5. Local engine and browser API — new backend contract

### Installation and persistence

Reserve `engine_key:"grengin"`. It represents one service-backed engine covering
multiple upstream providers. Administrators may install it through the existing `POST /admin/ai-engines` with `engine_key:"grengin"`
and model policy. Allow this reserved engine without `api_key`/`plugin_config`;
reject either field if provided. Persist exactly one `ai_engines` row with both
fields NULL. Startup and catalog reads must not create/recreate a deleted row or
auto-install direct providers. For a new managed workspace with an attached identity,
the first creator’s verified SSO login installs the native engine enabled in the
owner transaction. The allowlist starts empty and neither model default is set.
Login requires no service status or consent request. Model requests still require
an authorized workspace identity and service access. Later logins preserve
administrator changes and deletion. Existing managed confirmations from earlier
releases resume through the reconciler.

The existing engine-key index is not unique. Add a migration enforcing a partial
unique index on the existing `"engineKey"` column for value `grengin`, and perform
installation transactionally. Before reserving this key, detect any existing
custom engine or duplicate with that name; report an actionable migration conflict
and preserve its data. Do not silently overwrite or reinterpret an existing
custom plugin as the reserved service engine.

Explicit administrator installation defaults to `is_enabled:false`. Requesting `is_enabled:true` requires
the added create field `consent_version:"grengin-models-v1"` and the same durable
remote-confirmation procedure as PUT. Store the row disabled until confirmation;
return 201 when installation/configuration completes or 202 while confirmation is
pending. Cloud-side consent alone never opens local routing. A duplicate install
returns 409 rather than resetting policy. Installing disabled needs no consent
mutation and does not disable remote consent implicitly.

Use `whitelisted_models` containing canonical `provider/native_model_id` values.
An explicit empty list means deny all. If omitted on installation, choose at most
two available text models and one available image model, or three text models,
ordered lexicographically by canonical ID (the service has no model release
dates). Do not include embeddings in these defaults. Set default text/image only
from the selected applicable capability. If none are available, persist an empty
list, `default_model:""` and `default_image_gen_model:null`; do not enable all.
Return `default_model:null` in the DTO when the stored default is empty.

Validate every policy write against a fresh service catalog and supported local
capabilities. Unknown/unavailable additions and invalid defaults return 400 with
field-specific errors. Existing selected models that later become unavailable
remain visible and removable; do not silently substitute another model. Preserve
per-department access, user permissions and existing local usage restrictions.

The service is authoritative for consent and balances. Local `is_enabled` is a
confirmed local routing policy, not proof that remote consent is enabled. Expose
both. A runtime request needs local engine installed/enabled, eligible local model
policy, confirmed identity and service authorization. Do not infer authorization
from a cached balance or a catalog `available` flag.

### Endpoints and DTOs

Use existing snake_case admin DTO conventions and enforce permissions in handlers.
All responses with connection/balance information use `Cache-Control: no-store`.
Register literal Grengin subroutes alongside the existing engine routes.

| Local endpoint | Permission and behavior |
| --- | --- |
| `POST /admin/ai-engines` | `ai_platform:manage`; explicit reserved-engine installation as above; accepts `consent_version` for explicit enabling. |
| `GET /admin/ai-engines/grengin` | `ai_platform:view`; existing engine detail plus `credential_source:"workspace_identity"`; `api_key_configured:false`, no preview or plugin config. Do not describe this as a missing provider API key. |
| `PUT /admin/ai-engines/grengin` | `ai_platform:manage`; accepts `display_name`, `is_enabled`, `whitelisted_models`, `default_model`, `default_image_gen_model`, and added `consent_version`. Reject API-key/plugin fields. Confirm remote changes as below. |
| `DELETE /admin/ai-engines/grengin` | `ai_platform:manage`; remove only the local row and caches. Preserve identity, remote balance and other providers. Stop local Grengin dispatch immediately; do not claim remote disconnect. |
| `GET /admin/ai-engines/grengin/status` | `ai_platform:view`; safe local/remote status even if identity or local row is missing. |
| `GET /admin/ai-engines/grengin/catalog` | `ai_platform:view`; adapted service catalog, local capabilities, allowed/default flags and effective funding source. |
| `GET /admin/ai-engines/grengin/models` | Existing permission rules; same catalog/policy source, not a provider-key listing call. |
| `POST /admin/ai-engines/grengin/test` and `/validate` | `ai_platform:manage`; identity refresh/introspection, status and catalog only. No billable generation. Return a connection test, not a claim that upstream generation succeeded. |
| `POST /admin/ai-engines/grengin/connect/start` | `ai_platform:manage`; standalone only; empty body or `name` up to 80 characters. Uses configured identity/origin, never browser-supplied keys or URL. |
| `POST /admin/ai-engines/grengin/connect/poll` | `ai_platform:manage`; poll the server-held active pairing, empty body. Return safe pairing status and confirmed IDs, never the grant/device code/account secrets. |

Generic API-key DELETE/PUT and custom plugin validation cannot be used to configure
`grengin`; return 400 `grengin_uses_workspace_identity`. Do not add a local
disconnect route whose implied service operation does not exist. Listing models
for ordinary authenticated users must not expose admin-only connection details.

Define a `GrenginStatus` DTO with these fields (nullable values are explicit):

```text
installed: boolean
local_enabled: boolean
connection_state: setup_needed | pairing | connected | disconnected | unavailable
engine_state: disconnected | update_needed | off | pending | empty | low | on | null
remote_enabled: boolean | null
installation_id, workspace_id: string | null
balance_microdollars, available_microdollars, low_balance_microdollars: integer | null
minimum_version: null
balance_url, top_up_url: string | null
observed_at: RFC3339 UTC timestamp | null
stale: boolean
error_code: string | null
pending_operation: {id, revision, state, error_code} | null
```

`engine_state` carries the last confirmed service value, except local missing
identity may explicitly map to the legacy `update_needed` state with
`connection_state:"setup_needed"`. Label it **Connection setup needed** (French:
**Connexion à configurer**), never an upgrade requirement. Mark cached values stale
on a failed refresh; unknown balances remain null, never zero. A transient timeout
means unavailable, not revoked or empty. Inactive introspection/confirmed service
disconnection means disconnected. Include a safe error code for NAT/isolation
failures rather than claiming the user has no funds.

The local catalog DTO adds `provider_id`, `native_model_id`, `capability`, integer
service prices, `processing_region`, `status`, `observed_at`, `is_allowed`,
`is_default`, `effective_engine` and `funding_source` (`grengin` or `own_key`).
For an own-key overlap, service prices remain labelled as Grengin tariffs; do not
represent them as the user's own-key price. Never expose private keys, grants,
device codes, full provider credentials or signed proof bodies in these DTOs.

### Configuration confirmation and partial failure

When changing `is_enabled`, persist a durable operation before the service call:
operation UUID, instance UUID/origin binding, increasing revision, desired policy,
state (`pending|applying|applied|failed`), safe error code and timestamps. Serialize
mutations across backend processes using durable database coordination, not only
an in-process mutex. Model-policy-only writes need no remote consent mutation.
Keep an installation generation/tombstone in durable state outside the engine
row. DELETE closes routing, increments that generation and cancels outstanding
local config intents. Every operation completion must compare its generation and
revision before committing; it cannot recreate the row. A later explicit install
gets a new generation and never resumes an older install's pending enable. An
already-dispatched remote SET may still finish; that must not resurrect local
routing or be described as a successful remote disconnect.

An administrator’s enable request requires `consent_version:"grengin-models-v1"` and service
`PUT /v1/connect/grengin-models` with `{enabled:true,consent_version}`. Disabling
sends `{enabled:false}`. Close local routing immediately for disable; open it for
enable only after remote confirmation and local policy commit. Read status after
the PUT; enabled remote states can be on/low/empty/pending, so an empty balance is
not evidence that enable failed. Missing/failed status leaves confirmation pending.

Return 200 with confirmed detail/status after completion, or 202 with operation
ID and pending status when reconciliation remains necessary. Status reads expose
the pending operation. On timeout/restart, read remote status before retrying the
idempotent SET; do not blindly toggle the opposite value as compensation. The
service has no revision/CAS for enablement: do not promise a distributed atomic
transaction, overwrite a newer local intent, or let a stale completion reopen
routing. Record conflicts/outages and reconcile to the latest explicit local
intent. Status refresh and managed Cloud registration must not silently re-enable
a locally disabled engine.

## 6. Service status and catalog — existing service contract

`GET /v1/infer/status` requires the current hybrid bearer. Response:

```json
{
  "installation_id": "inst_example",
  "workspace_id": "cw_example",
  "engine_state": "on",
  "balance_microdollars": 550700,
  "available_microdollars": 550700,
  "low_balance_microdollars": 0,
  "minimum_version": null,
  "balance_url": "https://grengin.com/ai-balance?workspace=cw_example",
  "top_up_url": "https://grengin.com/ai-balance?workspace=cw_example&dialog=top-up"
}
```

Amounts above are illustrative. Standalone `workspace_id` is the installation ID.
Use returned billing links and preserve their target through sign-in. Local AI
administration permission does not imply website billing ownership; only website
owners top up or change billing alerts. Backend does not implement Stripe checkout.

State precedence is disconnected → missing hybrid identity (`update_needed`) →
off → pending allowance → empty (available <= 0) → low (available <= threshold) →
on. However status authenticates first: missing identity, revoked/disconnected,
shared-organization, paused/not-ready managed, or unauthorized NAT conditions
normally produce HTTP 401 instead of a successful disconnected/update-needed
snapshot. Only engine-off bypasses the enable check for status/configuration.

`GET /v1/infer/catalog` is public and returns `{enabled:true,providers:[...]}`.
Each provider has `id`, `name`, `status` (`available|unavailable|pricing_unavailable`),
`processing_region` (currently null) and models. Each model contains:

```text
id: provider/native_model_id
native_model_id, name: string
capability: text | image | embed
input_per_million_microdollars, output_per_million_microdollars: integer
free: boolean
status: available | unavailable
fetched_at: timestamp from tariff effectiveFrom, not this request's observation time
```

Split IDs at the first slash only and preserve the native ID exactly, including
case and further slashes. Keep money in integer microdollars; prices are per
million tokens. Use a separate local `observed_at`. Unknown processing region
stays unknown. Availability means configured credential plus published tariff,
not measured provider health. Partial capability failures may yield a partial
catalog. Missing pricing is not a free model; zero tariff rates still require
authentication and currently at least one microdollar for reservation.

Coalesce catalog/status reads; use a 60-second catalog TTL and a 10-second status
TTL, with timestamps/staleness exposed. Bypass the catalog TTL for policy writes
and status TTL for configuration confirmation. Never query Stripe or the wallet
directly. Do not put the status endpoint on the hot path of every inference call.

Source: [ai-engine.ts](../../grengin-website/workers/src/handlers/ai-engine.ts).

## 7. Native routing, model policy and provenance

The service is a native gateway, not a single OpenAI-compatible model API. Reuse
provider-native serializers and stream parsers, but replace the transport
destination/authentication for the Grengin route. Attach the current grant at
dispatch time to **every** operation, including tool continuations from a cloned
chat session. Do not capture the token permanently in `ProviderRuntimeConfig`.

The browser/API request convention is `provider:"grengin"` with
`model_name:"<provider>/<native_model_id>"` in the existing chat input. Model
listing exposes the compound selection key under engine `grengin`. After
resolution retain that requested selection separately from effective engine,
canonical upstream provider and native model ID. Direct-provider requests keep
their existing shape. Validate the requested Grengin policy before applying
own-key precedence; canonicalization must not make an unallowed requested model
valid. Apply the same convention to image and embedding selection DTOs.

The transport resolves only the following routes under
`/v1/infer/{canonical_provider}/{operation}`:

| Provider | POST operations | GET operations |
| --- | --- | --- |
| openai | `responses`, `chat/completions`, `embeddings`, `images/generations`, `images/edits` | `models` |
| anthropic | `messages` | none |
| mistral | `conversations`, `chat/completions`, `embeddings` | `models` |
| gemini | `models/{model}:generateContent`, `models/{model}:streamGenerateContent`, `openai/embeddings` | `openai/models` |
| qwen, xai | `responses`, `chat/completions` | `models` |
| deepseek, kimi, groq, cerebras, glm, minimax, sarvam, sakana, nvidia | `chat/completions` | `models` |
| tinker | `chat/completions` | none |

`google` is a service alias for `gemini`; publish canonical IDs locally. Bedrock,
Azure OpenAI and OpenRouter are not Grengin gateway providers. Native Gemini
embedding endpoints are unsupported. Native Gemini path model IDs accept only
letters, digits, underscore, dot and hyphen. A manifest capability is offered
only if its native operation is supported by this matrix and the service catalog.
Do not send all providers through `chat/completions` or guess unsupported paths.

Rules for requests:

- Use the native model ID in payload/path, not the compound catalog ID. Keep
  compound ID, effective engine and funding source in local usage provenance.
- Service authentication is **only** `Authorization: Bearer <connection_grant>`.
  Strip plugin-generated `x-api-key`, `x-goog-api-key`, `api-subscription-key`,
  `x-grengin-proxy-key` and any static Authorization before injecting the grant;
  presence of those native credential headers is rejected even with a valid grant.
- Forward only required content/accept headers and optional `anthropic-beta`.
  The service supplies provider credentials and the Anthropic version header.
  Preserve no raw user email or provider key in auxiliary headers. Optional
  `X-Grengin-User-Hash` must be a workspace-scoped pseudonymous lowercase SHA-256
  value, not a raw user identifier or email.
- Permit only Gemini streaming `?alt=sse`; reject other queries and arbitrary
  origin/base-URL overrides. No redirects, escaped path separators, traversal or
  user-supplied plugin routing for the reserved engine.
- JSON is the normal media type; multipart is supported only for OpenAI image
  edits. Images are non-streaming. Native text streams retain their own SSE
  protocol. Native model listing is authenticated/enabled/rate-limited but does
  not reserve money; use the public catalog for selection.
- Send inline history, tool calls/results and files. Do not use saved provider
  response/conversation/thread/assistant/file/vector-store/cache/container/skill
  references or background work. Preserve opaque customer tool arguments as
  content. Responses and Mistral Conversations disable provider storage.
- Preserve valid native output limits and usage events. The service fills bounded
  output defaults and requests final usage on compatible chat streams. Requests
  are capped at 32 MiB; buffered responses at 16 MiB; upstream work at 240 seconds.

### Own-key precedence

Use a shared model-aware selection function for model listing and all dispatch
paths: chat, image, embeddings and internal summary/RAG calls. If the same native
provider/model is eligible under an enabled direct own-key engine and Grengin,
select the own-key engine. An invalid/exhausted own key remains that engine's
error; never silently fall through to Grengin and debit another funding source.
Each engine's whitelist governs that engine: an enabled direct engine whose
whitelist excludes this model is not eligible and does not implicitly prohibit
the same model in an explicitly allowed Grengin policy. Department/user model
restrictions and budgets apply after canonical model resolution across both
funding routes and cannot be bypassed by duplicate catalog entries. A
disabled/uninstalled direct engine likewise does not shadow an otherwise
explicitly allowed Grengin model.

Persist effective engine/provider/model/funding provenance on requests and local
usage. Deduplicate display entries without losing that provenance. Do not change
the funding source halfway through an in-flight generation/tool session. If its
policy is revoked, stop further operations instead of switching engines.

Invalidate registry and model/selection caches on key changes, direct engine
enable/disable/delete, Grengin model/config changes, identity changes and grant
replacement. Cache keys must include connection identity and policy generation,
not just `engine_key`; a cached provider must not bypass new restrictions. Before
every new operation and tool continuation, read authoritative durable installation
generation, installed/enabled state and policy revision, and enforce current
user/department access. Use the same check for direct-engine precedence changes.
An in-process invalidation event alone is insufficient across Lambda processes.
Do not promise revocation of bytes already dispatched; once a disable/delete is
observed, cancel active work where possible and permit no further dispatch.

Sources: [llm-providers.ts](../../grengin-website/workers/src/services/llm-providers.ts),
[llm-proxy-auth.ts](../../grengin-website/workers/src/services/llm-proxy-auth.ts),
[llm-proxy-state.ts](../../grengin-website/workers/src/services/llm-proxy-state.ts).

## 8. Streaming, failures and accounting boundaries

Stream incrementally with backpressure. Client cancellation closes the outgoing
service request; preserve the existing native tool/reasoning parsing and browser
SSE contract. Keep safe request IDs through errors and logs. Successful service
responses expose `X-Grengin-Request-Id`, optional provider request IDs and
`Retry-After`. `X-Grengin-Usage-Status: pending` means accounting awaits review,
not that successful generation should be discarded or repeated.

Parse `{error:{type}}` even when no message exists. Preserve the type and HTTP
status; user-facing text may be localized. Keep local connection state errors
separate from gateway/provider errors. The following wire families are material:

| Service status | Types / handling |
| --- | --- |
| 401 | `invalid_connection_grant` (one proven pre-forward retry); `workspace_disconnected`, `workspace_balance_not_isolated`, `engine_off`, `ip_not_authorized` (surface distinct condition). |
| 402 | `ai_balance_empty`; show balance/top-up action, never auto-purchase. |
| 400 | `unknown_provider`, `unsupported_operation`, `unsupported_model`, `invalid_body`, `unsupported_image_stream`, `provider_state_not_supported`, `invalid_request`. Fix request/configuration; do not retry automatically. |
| 405 / 413 / 415 | `method_not_allowed` / `body_too_large` / `unsupported_media_type`. |
| 429 | Inference `rate_limit_exceeded` with `retry_after_ms` and headers; Connect uses `rate_limited`. Respect backoff and preserve cancellation. |
| 503 | `provider_unconfigured`, `pricing_unavailable`, `llm_allowance_pending`, `credits_unavailable`, `accounting_unavailable`. Do not describe all of these as empty balance. |
| 502 | `provider_redirect`, `provider_unavailable`, `upstream_timeout`, `unexpected_provider_response`, `provider_response_too_large`. Ambiguous generation must not be replayed. |
| Upstream status | `{error:{type:"provider_error",upstream_status}}` with safe IDs. An upstream 401 is not a workspace grant-refresh signal. |

Do not expose raw upstream error bodies or credential/account details. The
generic plugin retry policy currently considers transport failures, interrupted
streams and some conflicts retryable; override that behavior for Grengin. The
service generates a fresh request ID per call and provides **no inference
idempotency/replay contract**. Automatic generation replay after a dropped response,
partial stream, timeout or uncertain settlement risks duplicate work/charges.

The service exclusively reserves, settles and reconciles the workspace wallet.
The backend records its normal local user/department usage, but must not create a
second wallet ledger, grant credits, infer final charges from reservations, or
write D1/CreditManager. Reservations are estimates; actual reported usage can
exceed them. Missing/partial usage is a service review condition, not permission
to invent a charge or retry. Preserve existing manual keys/credit setup.

Emit durable, deduplicated notifications to Super Admins/IT Admins for confirmed
transitions into empty, disconnected and setup-needed. Use a persisted transition
generation and per-recipient uniqueness, so process restarts/polling do not spam.
Suppress transient timeout alerts and distinguish the runtime notice from website
billing notices/Stripe receipts. Include safe action links, never grants/keys or
prompt/output content. Do not synthesize an unsupported abuse-hold state.

Source: [proxy.ts](../../grengin-website/workers/src/handlers/proxy.ts).

## 9. Required validation and completion criteria

Add focused unit and integration coverage using disposable local storage and a
mock service/provider transport; no shared cloud mutation in ordinary tests.
Cross-language fixtures must invoke the actual website verifier/handlers, not a
second Rust-only approximation. Use ephemeral test keys, never workspace secrets.

| Area | Required proof |
| --- | --- |
| Identity format | PKCS#8 generate/load round trip; independently verify each public key; wrong algorithm/schema/mode/origin/UUID/mismatched pair rejected; no secret logging. Startup missing keys never generates new identity. Restart preserves public fingerprints. |
| Hybrid signatures | Rust proofs accepted by website Ed25519 + native workerd ML-DSA verifier; corrupted either signature, altered bytes/domain/UUID, trailing newline, timestamp milliseconds/skew, reused nonce and downgrade rejected. |
| Grants | Concurrent callers coalesce; frozen-time/restart/expiry behavior; request-time refresh; 429/backoff; introspection active:false; changed installation/org refused; no grant leakage. |
| Pairing | Pending/denied/expired/approved flows; device code private; managed mode rejected; polling grant never used for inference; separate hybrid refresh required. |
| Rotation/revocation | New key version rejects old grant; lost rotation response and restart recover using saved pending keys without overwriting old; revoked state cannot be recreated by restart or retry. |
| Engine policy | First managed creator preinstallation and explicit administrator installation; reserved key cannot take custom plugin/API key; deletion stays deleted after restart; empty list denies all; bounded defaults; invalid defaults/model capabilities rejected. |
| Configuration | Manage permission on every administrator mutation; view-only cannot enable; explicit installs default to disabled; managed first-creator installs start enabled with empty model policy; pending operations survive restart; timeout/readback; concurrent revisions; disable closes local routing across processes; delete/reinstall changes generation so stale completion cannot re-enable. |
| Routing | Native OpenAI Responses, Anthropic Messages, Mistral Conversations, Gemini content and compatible chat; image generate/edit and embeddings; reject unsupported routes/queries; service sees bearer only and exact native model ID. |
| Continuations | Grant expires between tool rounds: next dispatch uses fresh grant without replaying previous output or tool execution. No grant captured by a long-lived cloned plugin session. |
| Models/provenance | First-slash parsing and case preserved; same listing/dispatch precedence; department policy retained; own-key failures never charge Grengin; cache invalidation works across config/key/identity changes. |
| Streaming | Native usage/tool/reasoning events, backpressure, client cancellation, timeout, partial stream, usage-pending response; no automatic replay or speculative charge. |
| Status/errors | Missing identity is setup-needed; no version gate; engine-off status allowed; NAT/paused/isolation auth failures distinct; unknown balance null; stale flag; preserve error type with no message and safe request IDs. |
| Authorization | Service revocation, changed identity version, shared org, paused/deleted managed workspace and wrong NAT reject inference even with cached grant. Backend does not bypass or auto-repair them. |
| Performance | Warm inference adds no mandatory status/catalog/sync round trips; concurrent metadata reads coalesce; control-plane timeout does not block unrelated direct calls; no fleet refresh loop. |
| Safety of checks | `/test` and `/validate` do not perform billable inference; fixtures do not call production or mutate existing workspaces; API documentation exposes no secret fields. |

Existing service regression references:

- [workspace-identity.test.mjs](../../grengin-website/workers/test/workspace-identity.test.mjs)
  and [workspace-identity-runtime.test.mjs](../../grengin-website/workers/test/workspace-identity-runtime.test.mjs).
- [llm-proxy-auth.test.mjs](../../grengin-website/workers/test/llm-proxy-auth.test.mjs),
  [llm-providers.test.mjs](../../grengin-website/workers/test/llm-providers.test.mjs),
  [llm-proxy.test.mjs](../../grengin-website/workers/test/llm-proxy.test.mjs),
  [llm-proxy-state.test.mjs](../../grengin-website/workers/test/llm-proxy-state.test.mjs).
- [ai-engine-catalog.test.mjs](../../grengin-website/workers/test/ai-engine-catalog.test.mjs),
  [ai-balance.test.mjs](../../grengin-website/workers/test/ai-balance.test.mjs),
  [llm-key-network.test.mjs](../../grengin-website/workers/test/llm-key-network.test.mjs).

Run the API's normal format/build/type/schema checks and affected tests, including
existing direct-engine, model-policy and chat/tool regressions. Do not start a
developer server or run live cloud tests implicitly. Publish exact test commands
and results with implementation, not a blanket claim based on this document.

### Staging acceptance after the later Hatchery work

Backend code completion is distinct from runtime activation. The operator first
selects one ready, unpaused staging workspace, deploys the compatible backend,
publishes its durable identity through the agreed file interface, and registers
the public keys from the correct regional network without changing its UUID,
organization, origin credential or existing credit receipts.

Verify a hybrid refresh/introspection, status and catalog. Confirm the new managed
creator sees the enabled engine with no models allowed, then configure the tenant’s model allowlist. With an operator-authorized small generation,
prove native response/stream success and service usage/wallet settlement; record
only safe IDs, timestamps and amounts. Exercise a later request after grant
refresh and a restart to prove durable identity and continued routing. Own-key
overlap must select the direct engine without a Grengin debit.

Repeat only after the canary passes. All eight staging grant receipts are already
applied; read current balance/expiry instead of funding again. Paused workspaces
require an intentional operator lifecycle action before inference. Production
inventory is not staging activation evidence. Provider secret configuration and
live upstream health also require separate verification.

After backend contract/tests are complete, the Hatchery document must cover new
claims and the operator-run, resumable existing-instance plan/apply/verify path,
durable secret version publication, regional registration/NAT observation,
conflict handling and sanitized receipts. Do not make those changes or run AWS,
Cloudflare deployment or remote migrations as part of this specification task.
