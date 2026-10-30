# Connector Broker Security Test And Remediation Report

Updated: 2026-10-30. Both backend/service repositories use the
`feat/connector-oauth-broker` branches. This report records local verification;
branch publication does not deploy or remotely migrate either service. The separately approved
grengin-list PR #42 was merged and its metadata published to R2 before this pass;
that publication does not deploy the broker.

This supersedes the first-pass findings in connector-oauth-broker-audit.md.
Scope: this connector feature and its MCP integration, on one backend instance.
This is not a security certification or an exhaustive audit of the whole product.

## Core-Only Staging Verdict

The requested admin GUI was deferred and its unfinished configuration scaffold
removed. No admin page, route, dependency or admin-config migration remains.
The resulting core is ready for controlled staging API testing after checking
integration with newer main changes, operator configuration and additive migrations,
not for a production release. Share
connector-oauth-staging.md for the tested API contract and rollout checklist.

Confirmed defects were reproduced and patched, including a live stale-credential
fallback missed by the original happy-path tests. Local API/Worker/MCP tests pass.
Production readiness is still gated on frontend confirmation integration, real
vendor registrations/consent tests, and the open operational risks below.

The browser receipt is an explicit security/API contract change. The callback no
longer activates access by itself. It is not proof that the same human/browser
initiated and finished the flow, and it is not phishing-resistant authentication.

## Evidence Before Patching

Four focused regression tests were run against the previous draft and all failed:

| Reproduction | Before patch | Required outcome |
| --- | --- | --- |
| Callback without local owner confirmation, then token checkout | 200 token returned | 409, no credentials |
| Disconnect a local start with no remote row | 404 from broker, API stuck on 503 | Safe idempotent cleanup |
| Configure revocation_url and disconnect | Zero revocation requests | Revoke issued credentials or retain a disabled retry handle |
| Refresh endpoint rejects invalid_grant | Row remained connected | Failed state, no usable credentials |

A fifth defect was reproduced through the compiled backend over HTTP: after a
successful broker disconnect, one matching legacy MCP row still had credentials.
The test asserted zero and received one. After patching the shared transaction,
the same end-to-end assertion passes.

## Security Finding Register

### SEC-01: Callback-Only Cross-Account Activation

Severity: High. Status: automatic activation blocked; frontend review still open.

An authorization URL could be forwarded to another browser. Previously, a valid
vendor callback immediately made its credentials available to the initiating
local connection without confirmation from that signed-in local account.

Patch: callback stores awaiting_confirmation and returns a random 256-bit,
ten-minute receipt only in that browser's response body. D1 stores its hash.
GET/list/start never return the receipt. POST `/connectors/{id}/complete` requires
the local owner's bearer token; the broker additionally verifies installation,
local user, grant, code hash, expiry and one-time consumption atomically.
Token checkout is refused until completion. Callback uses no-store, a restrictive
CSP and no-referrer; the receipt is not in a redirect URL or log.

Tests: callback-only checkout refused, wrong user/grant/receipt, replay, concurrent
completion, expiry, audit failure and lost local-save reconciliation. This tests
the application contract, not browser SOP/CSP behavior or real frontend code.

### SEC-02: Broker Disconnect Re-Enabled Legacy Credentials

Severity: High. Status: patched and reproduced live before/after.

Deleting a broker mapping exposed a still-connected mcp_connections row to the
legacy fallback resolver. Thus a successful new disconnect could resume access
using old credentials.

Patch: disconnect clears the matching user's legacy access/refresh credentials,
connected flag and expiry in the same backend transaction that deletes the broker
mapping and writes its required audit event. It does not touch another user.

Test: seed a connected legacy row alongside the broker mapping, fail vendor
revocation, retry successfully, and verify no connected/token-bearing legacy row
remains. Pending/revoked mappings also suppress legacy fallback before cleanup.

### SEC-03: Provider Revocation Was Never Invoked

Severity: Medium. Status: patched for configured RFC-7009-style endpoints.

Patch: disable checkout first, then revoke refresh and access tokens through the
operator-configured HTTPS endpoint. Use the configured client authentication,
bounded requests/responses, and no redirects. HTTP 202, error-bearing JSON and
failed requests leave encrypted, unusable credentials for explicit retry. No
endpoint means provider_revocation=unsupported, not a false revocation claim.

Tests: two revocation requests, outage/recovery, unsupported endpoint, 202, JSON
errors, encryption retention, checkout denial and local credential removal.
An actual vendor's response semantics still require live verification.

### SEC-04: Late-Issued Credentials Could Escape Disconnect Cleanup

Severity: Medium. Status: patched within the operation lease; outage limit below.

Disconnect could finish while refresh/code exchange was running, losing the new
credentials issued immediately afterwards. A failed post-exchange save could
also discard issued tokens when vendor revocation failed.

Patch: active exchange/refresh leases make disconnect return cleanup pending.
Late credentials are retained encrypted in the revoked row, never activated.
Failed approval saves retain issued credentials in failed state if revocation
also fails. The local grant remains available only to retry cleanup. Expired
approvals attempt revocation and retain a handle only when necessary.

Tests: blocked refresh versus disconnect, blocked code exchange versus disconnect,
failed post-exchange audit plus revocation outage, expired approval plus outage,
and successful cleanup retry. Total simultaneous storage/provider failure cannot
be guaranteed recoverable without independent durable infrastructure.

### SEC-05: Invalid Refresh Grants Left Misleading Connected State

Severity: Medium. Status: patched.

Patch: classify invalid_grant separately from transient provider errors. Under
the refresh lease, move the connection to failed and erase unusable credentials.
Do not overwrite a concurrent revocation. Rate limits/outages do not erase a
potentially usable refresh token. Provider bodies are never reflected to clients.

Tests: invalid_grant state/erasure, transient concurrent refresh, refresh-token
rotation/omission and encryption-key rotation.

### SEC-06: Incoming Body Reads Had No Explicit Absolute Deadline

Severity: Medium availability hardening. Status: patched and locally exercised.

Patch: boundedText has a fixed ten-second deadline that cancels the reader; it
does not restart the clock after every chunk. Byte limits remain in place.

Test: an unauthenticated streaming request sends periodic partial data but never
finishes. It is cancelled with 408 at the fixed deadline and consumes no nonce
quota. Cloudflare-wide denial-of-service resilience was not load-tested.

### SEC-07: Credential Destination And Redirect Protection

Severity: Medium defense-in-depth. Status: patched and regression-tested.

Patch: compare pinned catalog endpoint to both current DB configuration and the
actual cloned client's effective transport endpoint, including stale SSE overrides.
MCP requests do not follow redirects, including HTTPS-to-HTTP. Broker/catalog and
revocation requests also disallow redirects. This changes behavior for users
whose MCP URL is a redirect instead of the canonical endpoint.

Tests: actual TLS MCP redirect to a local HTTP capture endpoint receives no
follow-up request; endpoint edits fail before tool traffic; a stale alternate
transport URL fails the pure endpoint-binding test.

### SEC-08: Local Writes Happened Before Upstream Abuse Controls

Severity: Medium availability risk. Status: patched and exercised over HTTP.

An authenticated user could send many syntactically valid bogus catalog keys.
Each attempt stored a local grant/audit row before upstream catalog validation
or throttling, so the service quota alone did not bound local database growth.

Patch: check a local, authenticated-user start budget before writes (20 per
60-second window), bound its memory cache, cap stored connections at 64 per user,
and serialize count/creation for the single-instance target. New connector JSON
routes also have a 16 KiB limit rather than inheriting the global upload limit.

Tests: twenty bogus starts leave twenty rows; the next returns 429 without a
write. A pre-seeded 64-row account gets 409 before insertion. Tests also cover
window reset, user isolation and a full limiter cache admitting new users.

### SEC-09: Misplaced Receipts Could Enter Generic Audit Query Logs

Severity: Medium confidentiality hardening. Status: patched and exercised live.

The intended receipt is in a POST body, which this middleware does not capture.
However, accidentally placing confirmation_code in a query could log it, and
encoded parameter names bypassed name matching.

Patch: decode query parameter names before classification and redact receipt,
connection-grant and private-key names, including normalized/encoded spellings.
The frontend must still never put secrets in URLs.

Tests: unit tests cover encoded names and a real successful completion with a
deliberately misplaced receipt query is checked in the persisted audit row;
only REDACTED markers remain. The actual body receipt stays absent from logs.

### SEC-10: Unapproved Authorization Parameters And Stale Operator Policy

Severity: Medium defense-in-depth. Status: reproduced and patched locally.

The operator allowlist checked endpoints and requested scopes, but catalog
extra_params could still add vendor-specific permissions such as user_scope.
Removing an app or narrowing its allowlist also did not block delivery of an
already-issued, unexpired token until a refresh was required.

Patch: every catalog extra_params key/value requires an exact match in the
operator client's extra_params map. Reserved OAuth/PKCE fields cannot be
overridden. Recheck the current client policy before owner confirmation and
credential checkout, not only at start/refresh. Operator configuration must now
explicitly include approved extras, including benign response_type=code where
the catalog declares it; this is an intentional configuration-contract change.

Tests first reproduced 200 responses for unapproved/changed parameters and
confirmation after app removal. The resulting tests cover rejection before row
creation, approved values, changed values, reserved-field overrides, missing apps
at confirmation and a narrowed scope allowlist before unexpired-token checkout.

## Other Confirmed Fixes

| ID | Finding | Patch and validation |
| --- | --- | --- |
| REL-01 | A failed start with no broker row could not disconnect | Signed disconnect acknowledges genuinely absent rows; present rows still need exact installation/user/grant. Live API cleanup and wrong-grant preservation tests pass |
| REL-02 | All upstream errors collapsed into 503 | New connector endpoints expose safe 400/403/404/409/429 and bounded Retry-After. Unknown bodies stay sanitized 503. Existing MCP error envelopes remain unchanged |
| REL-03 | connected_at changed with status refresh | Store first local connection verification time separately; repeated status reads keep it stable |
| REL-04 | Duplicate user/server bindings returned 503 | Map the actual unique constraint violation to 409; live test verifies only one connection exists |
| REL-05 | Revoked installation could not clean up its credentials | Allow only signed, correctly bound disconnect after installation revocation. Start/checkout still fail. Unit tests verify both boundaries |
| REL-06 | Generic OAuth compatibility details | JSON Accept header, exact PKCE/redirect binding, proper basic credential encoding, safe response_type=code, rejection of unknown auth modes, optional token expiry and unsupported token-type rejection |
| REL-07 | Slack's successful user-only token response was rejected, while a generic Bearer shape was accepted | Restrict the adapter to the Slack provider and exact official authorization/token/MCP endpoints; require ok=true and token_type=user. Mock approval, refresh, malformed/error/bot responses, endpoint impersonation and generic-provider isolation pass |
| REL-08 | Deleted owners could leave broker credentials without a retrying local cleanup | Retain encrypted grants, deny the deleted user's session immediately, and run bounded cleanup on the persistent single-instance backend. Broker/provider failures are retried, successful cleanup removes grants, and required audit events identify system actions. The user FK now restricts hard deletion until cleanup; temporarily deactivated accounts retain settings |

### SEC-11: OAuth App Identity Was Not Bound Across Consent

The operator client ID could change between start/callback or between issuance
and confirmation/checkout while endpoints and scopes remained the same. Tests
reproduced successful callback/confirmation despite this app identity change.

The encrypted pending payload and issued credentials now include the original
client ID. Callback, confirmation and checkout reject mismatches with 409 before
exchanging/delivering credentials. Cleanup retains credentials if the current
app differs, allowing an operator to restore the original app and retry. Legacy
prototype credentials missing this binding are denied for use but still allowed
through destruction-only cleanup. No new browser fields or D1 columns are added.

### REL-09: Non-JSON Edge/Vendor Rate Limits Were Collapsed To 503

Actual backend HTTP and Worker refresh tests reproduced 503 for an HTML 429.
Both boundaries now inspect the HTTP status before JSON parsing. Retry-After is
bounded to 1-300 seconds, unknown bodies are never reflected, and refresh grants
survive rate limiting. Tests cover the edge HTML case, vendor HTML refresh,
recovery and malformed/out-of-range delay headers.

Slack's response contract is documented in the official
[oauth.v2.user.access reference](https://docs.slack.dev/reference/methods/oauth.v2.user.access/).
The adapter does not add support for Slack bot OAuth, verify an app registration,
or establish that the registered redirect URI/scopes have been accepted by Slack.

## Existing Security Controls Rechecked

- Ed25519 proofs bind exact body, method, path, installation, timestamp and nonce.
- Cross-installation use of a copied grant is refused, even with a valid signature
  from the other installation. Wrong users/grants and revoked installations fail.
- Timestamp boundary and nonce replay tests pass. Quota is per installation/user;
  malformed/unsigned requests do not consume authenticated quota.
- APP_KEY grant corruption fails closed. The model cannot serialize credentials.
- AES-GCM encryption binds connection/install/user/catalog as associated data.
- API/Worker error and success responses are no-store; vendor error text is redacted.
- Required audit failures roll back local creation and completion. A remote
  approval followed by local-save failure recovers via GET status, not code replay.
- Operator allowlists prevent catalog changes from expanding scopes or changing
  OAuth endpoints/MCP origins silently. No user-provided endpoints/secrets are used.
- SSO configuration, frontend files, dev database and real vendor registrations
  are not modified. No Redis/distributed backend machinery is introduced.

## Verification Results

Final verification on the resulting working tree:

| Check | Result |
| --- | --- |
| Rust workspace | 758 passed, 0 failed; 61 live/environment-dependent tests ignored |
| Compiled backend HTTP/security suite | 47 named checks passed, including actual MCP tools/call, chat continuation, HTML edge 429, matching runtime/DB/source migration heads and deleted-owner outage/recovery cleanup |
| SeaORM connector migration up/down/up | Passed against disposable PostgreSQL |
| Worker full suite | 402 passed, 0 failed |
| Focused broker suite | 57 passed; included in the Worker total |
| Real catalog shapes against mock OAuth vendors | 33 templates passed as subcases, not 33 live vendor integrations |
| Saved Slack settings / published-catalog preflight | Passed: immutable package SHA-256, operator settings, PKCE, exact service callback, encrypted pending state and idempotency; no vendor consent/token call |
| Worker typecheck | Passed |
| Local Wrangler migration chain | Passed |
| Rust workspace build, two jobs | Passed with local debug-info override |
| Clippy, workspace/all targets | Completed; existing/previous-work warnings remain, none in the new connector modules |
| Formatting, Bash/JS syntax, both diffs | Passed |

Clippy is not globally warning-free: the current run reports 121 normal-target
and 122 test-target warnings, plus warnings in other workspace crates; none
points to the new connector modules. Existing warnings also remain in the
pre-existing MCP integration code, outside the new per-user cache changes. The
proc-macro-error2 future-incompatibility warning also remains.

Tests use a compiled real backend, disposable PostgreSQL 16/pgvector, an in-memory
D1 adapter, a local TLS MCP vendor and local model endpoint. They do not replace
the backend handlers with mock return values. No paid model calls are made.

The test script sources the requested grengin.sh and .grengin_auth_token without
printing their contents. APP_KEY/JWT_SECRET are used for short-lived synthetic
test users, not actions on the dev or staging database. Backend ports are random
loopback ports. Temporary files, TLS keys, processes and containers are cleaned up.

Test entry points:

```bash
# grengin-api: the script includes fmt, workspace tests, build, HTTP, migration rollback
bash tests/connectors/audit-core.sh

# Individual backend suite:
bash tests/connectors/run.sh

# Reuse a verified current build for HTTP and migration tests only
bash tests/connectors/run.sh --live-only

# Saved Slack settings + real published metadata; no Slack consent/token request
bash tests/connectors/slack-preflight.sh

# grengin-website/workers: exercise the real local catalog against mock vendors
GRENGIN_CONNECTOR_CATALOG_ROOT=/home/anurag/work/grengin-list/master-data/connectors \
  node --import tsx --test --test-concurrency=2 test/*.test.mjs
pnpm exec tsc --noEmit
bash test/migrations.sh
```

Rust uses at most two Cargo jobs. Local test builds use package-level debug=0
and codegen-units=16 with incremental test compilation disabled for memory
headroom in the three-GiB test scope. Earlier compiler
OOMs and a stale-binary HTTP attempt are recorded as test setup failures, not
passing product tests. Final results must come from the rebuilt current binary.
The first 2026-10-30 HTTP attempt linked a stale cached migration library and
failed because connectors did not exist. Rebuilding only the migration package
fixed that setup mismatch. The HTTP suite now refuses to proceed unless both
runtime migration heads equal the checked-out migration/HEAD; the clean rerun
passed all 39 checks. No dev/staging database was used for this diagnosis.
The subsequent core-only pass adds seven named HTTP lifecycle checks plus an
HTML edge 429 regression (47 total): hard
deletion preserves grants, deactivation retains settings, deleted sessions are
refused, broker/provider failures retain retry records, recovery erases remote
credentials, system audit attribution is correct, and active users are untouched.

## Open Security/Deployment Risks

| ID | Severity / status | Remaining risk | Proposed follow-up and exit test |
| --- | --- | --- | --- |
| OPEN-01 | High deployment gate | Actual frontend/session handling is not implemented or browser-tested. Codes can still be socially forwarded | Implement the owner-confirmation UI from connector-oauth-broker-frontend.md; reject unsolicited IDs/URL codes; verify origin/window state for any popup automation; test two isolated browser accounts |
| OPEN-02 | Closed locally; deployment gate remains | Bounded regional maintenance is wired to the existing scheduled handler; deployed cron behavior is not yet verified | Tests cover abandonment without polling, provider outage/backoff/recovery, disabled installations, active leases, ten-row batches and proof expiry; audit/state tombstones are retained |
| OPEN-03 | Medium resilience | If issuance succeeds while all durable writes and vendor revocation are unavailable, cleanup cannot be guaranteed | Review an independently durable encrypted revocation outbox and regional requirements; fault-test total storage outage plus vendor outage, then recovery |
| OPEN-04 | Medium compatibility gate | Mock token endpoints do not establish that all 33 vendors accept the registered app/scopes/callback or implement the declared revocation behavior | Register reviewed apps, set exact callback URIs, and complete real consent/tool/revocation tests per vendor; add provider-specific adapters only where justified |
| OPEN-05 | Closed locally | Authenticated HTTP clients now use per-user locks; the server-wide map lock is not held over I/O | Rust regressions cover a blocked user's tool versus another user's list/token rotation, a 256-entry per-server cap and refusal to evict in-use entries; this is not a global memory cap or distributed-session implementation |
| OPEN-06 | Low metadata gap | Scopes describe requested catalog permissions, not provider-introspected actual grants | Store/report granted scopes separately if the vendor supplies them; test partial/default grants and refresh scope changes |
| OPEN-07 | Closed locally for persistent single-instance backend; serverless execution gate remains | Deleted-user cleanup is now retryable using the existing encrypted connector rows, and hard deletion cannot cascade them away | Live HTTP tests exercise actual admin deletion, immediate 401, broker outage, vendor outage, eventual erasure and system audit events. The in-process timer is not durable while Lambda is frozen; define external scheduling before serverless rollout |

No claim is made that these changes satisfy any particular privacy law or that
provider consent is always revoked. Unsupported revocation and simultaneous
outages require honest UI/status and potentially manual vendor-account action.

## Review Boundaries And Changed Contract

- Backend adds POST `/connectors/{id}/complete` and awaiting_confirmation;
  frontend refresh-only logic is now insufficient and must not be shipped unchanged.
- Disconnect may return 200 with revocation_pending=true; this means disabled,
  not fully cleaned. Retry explicitly. provider_revocation reports revoked,
  pending, unsupported or not_applicable without returning credentials.
- Existing mcp_client.rs dirty work predates this task. This work additionally
  refuses redirects, exposes the effective endpoint and replaces the shared HTTP
  cache I/O lock with bounded per-user entries. Formatting also touches the
  existing diff. Do not attribute/revert it wholesale.
- Local start quotas/capacity are single-instance guards, not distributed limits;
  a future multi-instance deployment must replace their process-local coordination.
- Both new migrations are untracked local draft files; neither was deployed.
- The backend user FK is now RESTRICT rather than CASCADE to preserve outstanding
  cleanup grants. Cleanup defaults to 60 seconds, at most five candidates per pass
  and a 20-second start budget; retries rotate timestamps to avoid starving other
  deleted owners. This is not Redis or a distributed worker. Configuring a frozen
  serverless deployment without an external execution plan is not approved here.
- Non-JSON 429 replies are treated as rate limits at both broker boundaries;
  incoming HTML/error text is never used as a frontend error message.
- grengin-list metadata was separately reviewed, merged and published under
  explicit user approval before this local pass. No further catalog changes or
  R2 uploads were made here. The service branch still includes its pre-existing
  cloud-connect/introspection ancestor, which must be included in merge review.

## Review File Map

Backend repository:

| Files | Review focus |
| --- | --- |
| src/services/connector_broker.rs | Proof signing, bounded HTTP, safe error translation |
| src/services/connector_limits.rs | Authenticated pre-write budgets and bounded limiter memory |
| src/services/connectors_helpers.rs | Ownership, grants, activation, revocation, legacy fallback, capacity and transactions |
| src/dto/connectors.rs / src/models/connectors.rs | Public/private boundary, typed states and snake_case contract |
| src/handlers/connectors.rs / src/routes/connectors.rs / src/docs/openapis.rs | Five endpoints, body limit, no-store, Swagger registration |
| src/handlers/mcp.rs / src/handlers/chat_stream.rs / src/services/mcp_helpers.rs | Existing MCP runtime/list/disconnect integration |
| src/services/mcp_client.rs | Broker redirect/endpoint binding and bounded per-user HTTP cache amid pre-existing dirty work |
| src/middleware/audit_log.rs | Receipt/grant/key redaction and decoded query names |
| migration/src/m20261028_000001_create_connectors.rs / migration/HEAD / migration/src/lib.rs | Additive reversible schema, stable connection time and registration |
| tests/connectors/run.sh / tests/connectors/http.mjs | Sourced credentials, isolation/cleanup, actual HTTP security regression cases |

Service repository (grengin-website):

| Files | Review focus |
| --- | --- |
| workers/src/services/connector-broker.ts | Broker trust boundary, consent receipt, token encryption, leases and provider cleanup |
| workers/migrations/0018_connector_oauth_broker.sql | D1 states, confirmation hashes, proofs, audit events and indexes |
| workers/test/connector-broker.test.mjs | Negative/race/fault cases and optional real-catalog/mock-vendor coverage |
| workers/src/index.ts / workers/src/types/index.ts | Opt-in routing and optional secret bindings |

Module registration, .gitignore and the existing tower-http set-header feature are
also changed. Local .codex notes are updated but remain ignored. Both branches
include the test scripts and migrations in their reviewed publication scope;
credentials and generated local artifacts are excluded. The tested tree also
contains MCP OAuth/transport changes already present before the broker work;
these were preserved and tested together, not attributed solely to the broker.

## Protocol References

The browser/account-binding review is informed by the redirect-flow and CSRF
guidance in [OAuth Security BCP, RFC 9700](https://www.rfc-editor.org/rfc/rfc9700.html).
The optional revocation request format follows
[OAuth Token Revocation, RFC 7009](https://www.rfc-editor.org/rfc/rfc7009.html).
Neither reference proves that an individual vendor or this frontend integration
has been validated; those remain explicit test gates.
