# Connector OAuth Broker: Audit Handoff

This is the historical first-pass handoff. The subsequent remediation and
expanded tests are recorded in `connector-oauth-broker-test-report.md`; use that
report and the current implementation contract for the latest status.

Date: 2026-10-28. Status: local, uncommitted implementation for review. This is
not an approval to merge or deploy, and passing tests are not a bug-free claim.

## Checkout And Provenance

Both repositories are on local `feat/connector-oauth-broker` branches. No GitHub
fork, commit, push, PR, deployment, remote migration or R2 publication was made.

| Repository | Starting HEAD | Notes |
| --- | --- | --- |
| `/home/anurag/work/grengin-api` | `3d03b0bb1e1e3c8c045e3313662f3a430ee7d2b7` | Branched from the checked-out main |
| `/home/anurag/work/grengin-website` | `e89c33ca9e399030f6f7eaf3fc34ba8528528f5f` | Branched from feat/grengin-cloud-connect; its existing introspection work remains in the ancestry |

IMPORTANT: backend `src/services/mcp_client.rs` already contained substantial
uncommitted changes before this task. They were preserved. Its entire diff is
NOT authored by this task: this task additionally disables HTTP redirects and
ran formatting. Review that pre-existing work separately when splitting commits.

`grengin-list` was read for reference only. PR #42 and its catalog were not
merged, changed or published by this implementation. No frontend UI was changed.

## Architecture Implemented

The selected architecture is **OAuth broker only; direct MCP traffic**:

1. An authenticated local user explicitly selects a connector and pinned version.
2. The API creates a connection UUID and random broker grant. It stores only an
   APP_KEY-encrypted grant, not vendor access/refresh tokens, in `connectors`.
3. A signed server-to-server request starts vendor consent at grengin-service.
4. The service fetches the immutable catalog document, verifies operator
   allowlists, and creates a ten-minute state and PKCE S256 challenge.
5. The vendor returns to the service callback. The service consumes state,
   exchanges the code, and encrypts the vendor credentials in D1.
6. The API checks broker status and checks out an access token when an authorized
   MCP operation needs it. Refresh stays at the service.
7. The API calls the vendor MCP endpoint directly. MCP requests/results and chat
   contents are not routed through the OAuth broker by this code.

This does not replace SSO, reuse SSO consent as resource consent, automatically
install catalog entries, introduce Redis, or add distributed backend support.

## Public Backend Endpoints

All four require the normal signed-in API user. A connection is scoped to its
owner; these are not system-admin-only installation-management endpoints.

| Method | Path | Behavior |
| --- | --- | --- |
| GET | `/connectors` | List the caller's stored connections; not a live verification of each vendor |
| POST | `/connectors/{connector_key}/connect` | Start/resume consent; body `{ "version": "1.0.0", "server_id": "optional UUID" }` |
| GET | `/connectors/{connection_id}` | Reconcile pending/connected state with the broker; locally revoked state stays revoked |
| POST | `/connectors/{connection_id}/disconnect` | Disable local use first, revoke remotely, then delete the local credential row |

Responses expose connection/catalog identifiers, optional MCP server ID, status,
scopes, optional expiry, pending authorization URL, and `revocation_pending`.
They never expose a broker grant, private key, vendor access token or refresh
token. Router-level `Cache-Control: no-store` covers errors as well as success.
The request DTO denies unknown fields and uses snake_case. Swagger paths are
registered in `src/docs/openapis.rs` with response/status descriptions.

Existing integration points were updated:

- Chat MCP tool execution resolves broker credentials before legacy credentials.
- Admin MCP tool sync uses the same credential resolver.
- `/mcp-servers` and `/mcp/connections` include the caller's broker connection.
- Existing `/mcp/connections/{server_id}/disconnect` also revokes a linked broker
  connection; unrelated legacy/custom OAuth continues through its old path.
- A broker mapping that is pending/revoked must not fall back to a legacy token.
- Credential checkout checks both the DB server endpoint and the actual cloned
  MCP client's endpoint against the pinned broker endpoint.
- MCP HTTP redirects are now refused. Existing deployments using a redirecting
  MCP URL need its canonical endpoint; this is a behavior change to review.

## Service Endpoints

| Method | Path | Authentication |
| --- | --- | --- |
| POST | `/v1/connectors/start` | Registered installation proof + local user + grant |
| POST | `/v1/connectors/{id}/status` | Same proof, owner and grant binding |
| POST | `/v1/connectors/{id}/token` | Same; access token is server-to-server only |
| POST | `/v1/connectors/{id}/disconnect` | Same; clears encrypted vendor credentials |
| GET | `/v1/connectors/callback/{connector_key}` | One-time OAuth state/code, not a backend browser bearer token |

The service is opt-in: `CONNECTOR_BROKER_ENABLED` must equal `true`. Otherwise
these routes return 404. The callback currently returns a plain confirmation,
not a new webapp page or an automatic window-close integration.

## Storage And Migrations

Backend migration: `m20261028_000001_create_connectors`; registered and placed in
`migration/HEAD`. It is additive and has a down migration.

`connectors` contains UUID, user ID, catalog key/version, optional MCP server ID,
status, encrypted grant, authorization/MCP URLs, scopes, expiry and timestamps.
Unique constraints cover `(userId, connectorKey)` and `(userId, serverId)`.
Deleting the user cascades; deleting the MCP server clears its link.
Initially one account per local user per connector is supported. Changing
version/server binding requires disconnect/reconnect, not an in-place patch.

Service migration: `workers/migrations/0018_connector_oauth_broker.sql` adds:

- `connector_connections`: installation/org/user bindings, grant hash, pinned
  catalog, state hash, encrypted tokens/PKCE, refresh lease and lifecycle state.
- `connector_request_proofs`: consumed nonces, expiry, and per-user quota indexes.
- `connector_audit_events`: lifecycle metadata, never raw codes/grants/tokens.

The Worker uses the existing regional storage environment. Operator rollout must
apply the migration to the D1 binding actually selected by that environment.
No existing production/dev schema was migrated during testing.

## Security Mechanisms

- Installation must currently be connected and not revoked in D1; checked on
  requests, callback exchange, and credential delivery.
- Ed25519 proofs bind installation, timestamp, nonce, method, pathname and the
  SHA-256 of exact body bytes. Timestamp tolerance is 60 seconds; nonces are
  single-use. Quota is 120 signed requests/minute per installation/local user.
- Grant lookup binds connection ID, installation ID, local user ID and SHA-256
  grant hash. Wrong owners/grants do not return credential data.
- Service AES-GCM encryption has key versioning, random IVs, and associated data
  binding connection/install/user/catalog identity. No plaintext fallback.
- Backend encryption uses the existing APP_KEY envelope and rejects legacy
  plaintext broker grants.
- Client endpoints must exactly match the operator registry; MCP origins and
  scopes must be explicitly allowlisted. Requesting users cannot submit endpoints,
  client secrets or arbitrary scopes.
- Catalog versions are explicit semantic versions, never `latest`. Only
  streamable HTTP + OAuth is supported, not legacy SSE, stdio or API-key auth.
- State is consumed before code exchange. Ambiguous exchange failures require a
  new consent flow rather than replaying a potentially redeemed code.
- Refresh uses a D1 compare-and-swap lease and cannot overwrite a concurrent
  disconnect. Omitted rotated refresh tokens retain the previous refresh token.
- Mutations and required audit entries commit in the same DB transaction/batch.
- Broker/catalog requests have response-size limits, timeouts and no redirects.
- Token requests advertise JSON (including GitHub), accept optional expiry for
  non-expiring tokens, and reject unsupported token types such as DPoP.
- Catalog `response_type=code` is accepted without allowing an override of state,
  callback, client ID, PKCE or authorization grant type.

## Configuration Required Before Real Use

Backend:

```text
GRENGIN_SERVICE_URL                 default https://service.grengin.com
GRENGIN_CONNECTOR_INSTALLATION_ID   existing registered installation ID
GRENGIN_CONNECTOR_PRIVATE_KEY       existing Ed25519 32-byte seed, 64 hex chars
APP_KEY                            existing backend encryption key
```

HTTP service URLs are allowed only on loopback for testing. Missing broker
configuration fails feature requests, not startup or unrelated SSO.

Worker:

```text
CONNECTOR_BROKER_ENABLED
CONNECTOR_OAUTH_CLIENTS
CONNECTOR_TOKEN_KEYS
CONNECTOR_TOKEN_KEY_VERSION
SERVICE_BASE_URL
```

`CONNECTOR_OAUTH_CLIENTS` is a secret JSON registry keyed by catalog auth.provider.
Each app declares client_id, optional client_secret, exact auth_url/token_url,
MCP origin allowlist and scope allowlist. Basic/post/none client authentication
comes from the catalog. Provider applications need the service callback URI
registered for each applicable connector key. SSO applications are not silently
reused. Token encryption keys are independent from SSO signing secrets.

## File Map

Backend new files:

```text
docs/connector-oauth-broker.md
docs/connector-oauth-broker-audit.md
migration/src/m20261028_000001_create_connectors.rs
src/dto/connectors.rs
src/handlers/connectors.rs
src/models/connectors.rs
src/routes/connectors.rs
src/services/connector_broker.rs
src/services/connectors_helpers.rs
tests/connectors/run.sh
tests/connectors/http.mjs
```

Backend existing files: module/route/OpenAPI registration, migration registration
and HEAD; `src/handlers/mcp.rs`, `src/handlers/chat_stream.rs`,
`src/services/mcp_helpers.rs`, `src/services/mcp_client.rs`; Cargo.toml adds the
existing tower-http set-header feature; .gitignore includes the new test folder.

Worker new files:

```text
workers/migrations/0018_connector_oauth_broker.sql
workers/src/services/connector-broker.ts
workers/test/connector-broker.test.mjs
```

Worker existing files: `workers/src/index.ts` route registration and
`workers/src/types/index.ts` optional environment bindings. No frontend changes.

## Tests Actually Run

| Check | Result |
| --- | --- |
| Rust workspace tests | 749 passed, 0 failed; 61 ignored live/environment-dependent tests |
| Rust workspace build, 2 jobs | Passed; pre-existing proc-macro-error2 future-incompatibility warning remains |
| Worker full suite | 362 passed, 0 failed |
| Focused Worker broker suite | 17 passed, 0 failed |
| Worker TypeScript typecheck | Passed |
| Local Wrangler migration script | Passed; local-only fresh migration chain |
| Real API + Worker + mock vendor/LLM HTTP suite | 20 checks passed |
| SeaORM connector migration up/down/up | Passed against disposable PostgreSQL |
| cargo fmt --check / git diff --check | Passed |
| Bash test-script syntax | Passed |

The HTTP suite sources `/home/anurag/work/secrets/grengin.sh` and the backend's
`.grengin_auth_token`, without printing credentials. It uses APP_KEY/JWT_SECRET
to mint short-lived fixture users in a fresh isolated database; it does not
operate on the dev database or send real provider keys to mocks/vendors.
The vendor is a local TLS MCP server, not a replacement mocked Rust handler.
The test runs real MCP initialization, tools/list, authenticated tools/call and
chat tool-result continuation through the compiled backend. Three model requests
are served by a local fixture; no paid model calls are made.

HTTP checks cover idempotent start, wrong-user denial, encryption/redaction,
no-store errors, callback replay, approval, broker outage, direct MCP execution,
existing connection listings, installation revocation, disconnect during outage,
no local resurrection on recovery, legacy disconnect integration and reconnect.
Worker tests add proof tampering/replay, expiry, provider denial, concurrent start,
audit-write rollback, allowlist drift, refresh rotation, key rotation, concurrent
disconnect/refresh, per-user rate limits, non-expiring tokens and token type checks.

All owned test servers, temporary TLS keys and disposable PostgreSQL containers
are cleaned up by the test harness. Real browser/vendor-consent integrations,
production D1, the frontend and Cloudflare deployment were NOT tested.
Clippy has not been rerun for the final draft; no lint-clean claim is made.

Reproduce from the backend directory, with dependencies installed in the website:

```bash
export CARGO_TARGET_DIR="$PWD/target-security"
cargo test --locked --workspace --jobs 2 \
  --config 'profile.test.package.grengin-api.debug=0' -- --test-threads=2
cargo build --locked --workspace --jobs 2
bash tests/connectors/run.sh --live-only
```

Worker commands, from grengin-website/workers:

```bash
pnpm exec tsc --noEmit
node --import tsx --test --test-concurrency=2 test/*.test.mjs
bash test/migrations.sh
```

## Open Findings And Explicit Review Questions

1. **Failed-start cleanup edge case (code-inspected, not fixed):** the API
   persists its encrypted grant before broker start for idempotency. If start
   fails before the broker creates a row, a later disconnect receives broker 404.
   ConnectorBroker maps that to 503, leaving local revoked/revocation_pending.
   Define safe idempotent cleanup for a genuinely missing remote row without
   weakening wrong-owner/grant checks; add an end-to-end regression.
2. **Consent/session binding needs security review:** callback approval currently
   relies on possession of state and a valid vendor code, not the initiating
   browser's signed-in local session. Explicitly assess forwarded authorization
   URLs/account-binding attacks for a shared multi-installation OAuth client.
   Do not treat the existing state/PKCE tests as proof this threat is closed.
3. **Vendor-side revocation is not implemented:** disconnect destroys broker
   credentials and prevents new checkout; it does not revoke a vendor-issued
   token/consent. `revocation_url` exists in the registry type but is unused.
   In-flight calls/already-issued tokens cannot be recalled by local revocation.
4. **Error translation remains coarse:** the backend maps broker errors including
   403/404/409/429 to generic 503 and does not propagate Retry-After. The broker
   itself returns structured fixed error types and Retry-After for 429.
5. **Status is not vendor introspection:** connected means broker consent state,
   not proof the vendor has not independently revoked it. Review invalid_grant
   refresh handling; it currently fails checkout without transitioning to failed.
6. **Known metadata imprecision:** the existing MCP connection DTO is populated
   with updated_at as connected_at for broker rows, not a stable approval time.
7. **Rollout prerequisites:** real vendor app registrations/secrets, published
   catalog versions, registered installation identity, regional D1 migrations,
   frontend consent/status/disconnect UI and final lint/audit remain operator work.
8. **Operational limits:** no background retry queue for pending revocation, no
   generalized vendor-specific token-response adapter, no SSE/stdio broker path,
   and no automatic cleanup policy for old terminal broker/audit records yet.

The test harness is intentionally mock-based: it proves protocol/control-flow
behavior, not that all catalog vendors accept the configured app/scopes/token
format. Review the above before calling this staging-ready or splitting commits.
