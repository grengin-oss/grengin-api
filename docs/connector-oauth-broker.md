# Connector OAuth Broker

Status: implementation contract; not deployed. Scope is a single grengin-api
instance. MCP traffic goes directly to the vendor, never through grengin-service.
This feature is separate from SSO and does not grant resource access at sign-in.

## Closure Pass (2026-10-29)

The next local pass adds a bounded connector-maintenance task to the service's
existing regional cron handler. It expires abandoned approvals, retries retained
revocations, and removes expired proof records without frontend polling. Every
transition must compare current state before vendor I/O. Credential cleanup uses
a leased compare-and-swap so a refresh, callback, disconnect or another cleanup
cannot overwrite each other's credentials. Active exchange/refresh leases are
left alone. Disabled installations are retired, never reactivated by maintenance.
Token material is erased on successful/unsupported cleanup; audit/state retention
is not silently changed to an invented privacy-policy duration.

The direct MCP client cache will use per-user entries rather than holding its
server-wide map lock across handshakes or tool requests. In-use entries must not
be evicted; a saturated cache fails with a bounded error instead of duplicating
an active user's session or blocking other users behind network I/O.

Requested scopes remain the existing `scopes` contract. A separately named
optional `granted_scopes` field is planned for provider-reported token scopes,
never inferred from requested scopes or presented as vendor introspection.
Neither this pass nor its mocks close the frontend/browser and real-vendor
deployment gates or guarantee recovery from simultaneous durable-store and
vendor outages.

## Slack Compatibility Pass (2026-10-30)

The published connector catalog is now available on meta.grengin.com. This does
not activate the broker or replace vendor consent tests. Slack uses its official
user-only OAuth endpoints, whose successful response has `ok: true` and
`token_type: "user"`, not the generic Bearer response. Accept that shape only
for the Slack provider bound to the official authorization, token and MCP
origins; reject bot tokens, missing/false `ok` and a Slack label on other
endpoints. Generic providers retain their existing token-type checks.

Operator client entries must explicitly allow each catalog `extra_params`
key/value using their own `extra_params` map. A public catalog must not request
additional vendor permissions through parameters such as `user_scope` or
change an approved consent setting without operator approval. Core OAuth and
PKCE parameters cannot be overridden, even through the operator map. The
existing redundant `response_type=code` no-op remains supported. Current operator
policy is checked again before confirmation and token checkout, not only at
creation or refresh, so removing an app or narrowing its allowlist blocks access.

This local pass also tests maintenance without user polling: abandoned consent,
revocation outages/recovery, disabled installations, active leases, bounded
batches and proof expiry. No service deployment, remote migration, frontend
change or credential publication is authorized by this pass.

## Ownership

Deleted local users are eligible for bounded, retryable connector cleanup. The
existing encrypted connector row is the durable retry record; do not cascade
away its grant before remote cleanup is acknowledged. Normal session checks
deny deleted users immediately. Temporarily deactivated/suspended users retain
their connection settings but cannot use their normal API session. The cleanup
job is for the single persistent backend process, not a durable Lambda scheduler;
frozen/serverless deployments need an external execution/retention plan.

grengin-service holds vendor client credentials and encrypted refresh/access
tokens. A registered installation receives a random, revocable connection grant
bound to its installation, local user UUID, connector key and catalog version.
grengin-api encrypts that grant with APP_KEY in a separate `connectors` table.
Provider access tokens are retrieved server-to-server for direct MCP requests;
they are not returned to the browser or persisted in the backend database.
Revoking the broker grant alone does not invalidate an already-issued vendor
token. Configured provider-side revocation is attempted separately; unsupported
or unavailable revocation is reported explicitly rather than assumed successful.

Existing self-configured MCP servers and `mcp_connections` remain supported.
Catalog entries are not automatically inserted in either database. Connectors
are created only by an authenticated user action. One account per user and
connector is supported initially. Catalog versions must be pinned explicitly.

## Instance Authentication

Until the instance-connection GUI owns these credentials, the backend operator
must configure GRENGIN_CONNECTOR_INSTALLATION_ID and
GRENGIN_CONNECTOR_PRIVATE_KEY (the existing registered Ed25519 identity, 32-byte
hex seed). No private key is sent to the service. GRENGIN_SERVICE_URL defaults
to https://service.grengin.com; HTTP is allowed only for loopback local tests.
Missing broker settings disable only this opt-in feature, never SSO or startup.

Every broker POST has four headers: X-Grengin-Installation,
X-Grengin-Timestamp (Unix seconds), X-Grengin-Nonce (32 hex bytes), and
X-Grengin-Signature (Ed25519, hex). The signed UTF-8 message is:

```text
grengin-connectors-v1
<installation_id>
<timestamp>
<nonce>
<HTTP method>
<URL pathname>
<SHA-256 hex of the exact body bytes>
```

The service checks the current D1 installation record, timestamp window,
signature, one-time nonce and per-installation/per-user quota. A user UUID is meaningful
only within its verified installation; it is not matched by email or SSO subject.
The backend also limits starts before writes (20 per minute), bounds per-user
stored connections at 64 and limits connector JSON bodies to 16 KiB. Creation
counts are protected by a per-user lock for the single-instance target.

## Flow

1. Signed-in user selects a connector and exact catalog version, optionally
   binding it to an accessible admin-configured MCP server at the same catalog URL.
   The server must use the catalog OAuth template's connection_config.oauth
   metadata; its client credentials are not used for a broker-backed user.
2. The backend persists an encrypted random grant before starting the broker
   request. The stable connection UUID makes uncertain start retries idempotent.
3. The service reads the pinned connector document from meta.grengin.com and
   checks its endpoints against operator-configured OAuth clients. It creates
   a short-lived state and PKCE challenge and returns an authorization URL.
4. Vendor redirects to the service's fixed callback. It atomically consumes
   state, exchanges the code, encrypts tokens and records awaiting_confirmation.
   A random, ten-minute confirmation code is delivered only in that browser's
   callback response. It is never returned by polling, stored plaintext or put
   in a redirect URL. No provider token or broker grant reaches the browser.
5. The owner submits that code through the authenticated backend complete
   endpoint. Until then token checkout is forbidden. This blocks automatic
   activation by forwarding an authorization URL to another person. The initial
   integration uses explicit code transfer in the original signed-in account;
   it is not a claim of phishing-resistant or same-browser authentication.
   A future popup integration must validate opener/origin, owner and window state;
   blindly posting the code to an arbitrary instance is not acceptable.
6. The backend checks broker status and retrieves an access token only when
   making an authorized direct MCP call. Refresh remains at the broker.
7. Disconnect revokes the broker grant and removes local credentials. Reconnect
   uses a new connection UUID/grant and a new explicit consent flow.
   Local use is disabled before contacting the broker. If the broker is down,
   the endpoint returns 503 and status remains revoked with revocation_pending;
   retry disconnect when service recovers to finish remote cleanup. Status
   refresh must never reactivate that locally revoked connection.

## Contracts

Backend, all requiring the signed-in owner:

```text
GET  /connectors
POST /connectors/{connector_key}/connect  {version, server_id?}
GET  /connectors/{connection_id}
POST /connectors/{connection_id}/disconnect
POST /connectors/{connection_id}/complete  {confirmation_code}
```

Broker POSTs have a JSON body with connection_grant and user_id (except start,
which also has connection_id, connector_key and version). All are signed:

```text
POST /v1/connectors/start
POST /v1/connectors/{connection_id}/status
POST /v1/connectors/{connection_id}/token
POST /v1/connectors/{connection_id}/disconnect
POST /v1/connectors/{connection_id}/complete
GET  /v1/connectors/callback/{connector_key}  # vendor redirect only
```

Broker status returns status, authorization_url only while pending, expires_at,
scopes, mcp_url and transport. Token returns access_token and expires_at only
to the authenticated installation. All private responses use no-store.
expires_at is null for vendor tokens that explicitly have no expiry. Unsupported
token types such as DPoP are rejected rather than used as bearer credentials.
The broker never accepts user-supplied authorization/token endpoints or scopes.
Only streamable HTTP catalog connections are enabled initially; legacy SSE
and stdio do not become supported through metadata alone.

When revocation_url is configured, disconnect first disables checkout and then
tries RFC 7009 revocation of refresh and access tokens without following redirects.
Failures retain only encrypted, revoked credentials for explicit retry; the
response reports revocation_pending. If no endpoint exists, the response says
unsupported and clears broker credentials without claiming vendor revocation.
An absent remote row is safe to disconnect only after signed installation auth;
a present row still requires its exact user/grant binding.

## Service Configuration

CONNECTOR_OAUTH_CLIENTS is a secret JSON map keyed by catalog auth.provider.
Each entry has client_id, optional client_secret, auth_url, token_url, and
optional revocation_url, plus explicit mcp_origins and scopes allowlists.
Endpoints must exactly match the pinned catalog. The service rejects catalog
documents that request other MCP origins or scopes, even if the OAuth endpoints match.
Include an explicit extra_params allowlist for any declared authorization
extras; for the current Slack template this is {"response_type":"code"}.
The deployed Worker reads this JSON map, not standalone SLACK_CLIENT_ID or
SLACK_CLIENT_SECRET bindings. Those two local variables are used by the
one-shot preflight script. Register the additional service callback without
removing an existing localhost callback used for self-configured OAuth.
CONNECTOR_TOKEN_KEYS is a secret JSON map of key version to 64 hex characters;
CONNECTOR_TOKEN_KEY_VERSION selects the active key. Old keys must remain for
decrypting existing connections. Use independent keys, not SSO signing secrets.

## Tests And Rollout

Tests must cover wrong user/install/grant, disabled or revoked installation,
proof replay/tampering, pending/approved/expired flows, reused callback state,
refresh rotation, concurrent refresh/disconnect, broker outages, URL changes,
credential redaction, and reversible additive migrations. Use mock providers
and disposable databases. Rust commands use at most two jobs.
Run tests/connectors/slack-preflight.sh to check saved Slack settings against
the published immutable catalog without contacting Slack's token endpoint.

No remote migrations, client registrations, R2 uploads, deployments or pushes
are part of local implementation. Existing cloud-connect service changes are
a prerequisite and must remain in the service branch ancestry.

## Core-Only Staging Handoff

The requested operator GUI was explicitly deferred; its unfinished scaffold was
removed. See connector-oauth-staging.md for the five user-facing API paths,
payloads, operator settings, additional Slack callback, error handling and
migration/rollback checklist. No admin-config tables or GUI dependencies remain.

Pending consent and encrypted vendor credentials are bound to their OAuth client
ID. Changing that ID requires cleanup/reconsent, not silent reuse. Old prototype
credentials without a binding are refused for use but can still be destroyed.
Service key rotation remains separate from OAuth app identity changes.

Deleted-owner cleanup defaults to one pass per 60 seconds, at most five rows and
a 20-second start budget. GRENGIN_CONNECTOR_CLEANUP_INTERVAL_SECS accepts 5-3600
seconds; invalid values fall back to 60. Grant records persist across failures,
and the user FK restricts hard deletion until cleanup completes. System cleanup
does not impersonate the deleted owner in audit entries. This scheduling claim
applies to a persistent single-instance backend, not frozen Lambda processes.
