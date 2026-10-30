# Connector OAuth: Staging API Handoff

Updated 2026-10-30. Core backend/service only; no admin or end-user GUI is part
of this patch. Both repositories use the feat/connector-oauth-broker branch.
Committing or pushing these branches is not permission to deploy, merge to main
or migrate a shared database.

## Readiness

Ready for controlled staging API testing after the branch integration check,
additive migrations and operator configuration below. Local verification exercises the
compiled backend and actual Worker handlers, not substituted HTTP responses.
Real vendor consent, deployed callback registration, browser integration and
Cloudflare cron execution still require staging verification. Not production
approval or a claim that every vendor has been integrated live.

## Frontend API Contract

Use the deployed grengin-api origin as API_BASE. All five paths require the
normal signed-in user's access token. Connections belong to that local user;
an administrator cannot use another user's connection ID. Requests use snake_case
and deny unknown fields. Responses and errors are no-store.

| Method | Path | Request / result |
| --- | --- | --- |
| GET | /connectors | Array of this user's stored connections; not vendor introspection |
| POST | /connectors/{connector_key}/connect | Explicitly start or resume a connection to a pinned template version |
| GET | /connectors/{connection_id} | Check state with the broker; an outage is not reported as verified connected |
| POST | /connectors/{connection_id}/complete | Submit the callback browser's one-time confirmation code |
| POST | /connectors/{connection_id}/disconnect | Disable locally, then clean up the broker connection; retry if pending |

Start request:

```json
{
  "version": "1.0.0",
  "server_id": "<optional accessible MCP server UUID>"
}
```

Omit server_id if not yet linking an MCP server. Actually executing tools needs
an enabled, accessible, admin-configured Streamable HTTP MCP server at the exact
catalog endpoint, with its OAuth configuration. Existing MCP policies still
apply. No connector/server/catalog entry is automatically inserted for everyone.

Representative public response:

```json
{
  "id": "<connection UUID>",
  "connector_key": "slack",
  "catalog_version": "1.0.0",
  "status": "pending",
  "revocation_pending": false,
  "scopes": ["<requested catalog scopes>"],
  "authorization_url": "https://slack.com/oauth/v2_user/authorize?...",
  "expires_at": "<ISO-8601 UTC timestamp>"
}
```

Optional fields include server_id, connected_at and provider_revocation.
authorization_url appears only while pending. Status values are pending,
awaiting_confirmation, connected, expired, revoked and failed. No row means
disconnected. No response exposes a connection grant, private key, client secret
or vendor token. Requested scopes are not an introspected vendor grant.

## Manual Consent Test

1. Obtain a normal access token from the staging backend's existing sign-in.
2. POST /connectors/slack/connect with the reviewed exact template version.
3. Open the returned authorization_url and complete Slack consent.
4. Slack redirects to the service callback, which displays a ten-minute code.
   This alone does not activate the connection.
5. Return to the original signed-in account and POST the code to complete:

```json
{"confirmation_code":"<64 lowercase hexadecimal characters>"}
```

6. GET the connection and verify connected, then run an allowed MCP tool using
   the linked server through the existing chat/MCP APIs.
7. Disconnect and verify the local row disappears after cleanup. If
   revocation_pending=true, use backoff and retry disconnect; do not claim that
   vendor consent has already been revoked.

Never put confirmation codes in URLs, logs or persistent browser storage. Do not
auto-complete from an unsolicited URL or forwarded code. Never forward the
backend bearer token to Slack or Grengin service. The service is an OAuth broker
only: MCP tool requests/results travel directly between grengin-api and vendor.

## Errors And Retry

Errors retain Grengin's rich detail envelope. Broker-origin errors include a
safe external_code; local validation/ownership errors retain their existing
application envelope. Do not branch on raw provider error text.

| HTTP | Meaning |
| --- | --- |
| 400 / 422 | Invalid payload, code format or unsupported connector |
| 401 | Local sign-in missing/invalid or user deleted |
| 403 | Installation/proof authorization refused; do not sign the local user out |
| 404 | Unknown connection or one owned by someone else |
| 409 | Wrong/expired/reused confirmation, changed OAuth identity, incompatible existing binding, or capacity conflict |
| 429 | Authenticated quota; honor Retry-After |
| 503 | Broker disabled, missing configuration, outage or persistence failure |

Stable broker external codes include connector_not_found, connector_forbidden,
connector_reconnect_required, connector_rate_limited, connector_capacity_exceeded,
connector_broker_disabled, invalid_connector_request and
connector_service_unavailable. After an uncertain complete response, GET status
before restarting: approval may have committed before the local save failed.

Limits: 16 KiB connector JSON bodies, 20 local starts/minute, 64 stored
connections/user and 120 signed service requests/minute/installation/user.
Use explicit refresh/backoff, not a tight polling loop. Revocation may be
revoked, pending, unsupported or not_applicable. Unsupported means the broker
erased its credentials, not that the vendor removed account consent.

## Operator Configuration

grengin-api requires its existing DATABASE_URL, APP_KEY, JWT_SECRET and runtime
configuration, plus:

- GRENGIN_SERVICE_URL: staging service origin, without a /v1 suffix. Default is
  https://service.grengin.com; set staging explicitly, not accidentally production.
- GRENGIN_CONNECTOR_INSTALLATION_ID: an installation connected in that service.
- GRENGIN_CONNECTOR_PRIVATE_KEY: the corresponding registered Ed25519 seed,
  32 bytes encoded as 64 hexadecimal characters. Keep it server-side.
- GRENGIN_CONNECTOR_CLEANUP_INTERVAL_SECS: optional 5-3600 seconds, default 60;
  prefer the default for normal resource use. Tests use five seconds.

grengin-service requires its existing session/installation bindings, plus:

- CONNECTOR_BROKER_ENABLED=true; otherwise broker routes return 404.
- SERVICE_BASE_URL: the exact staging service origin used to form callbacks.
- CONNECTOR_TOKEN_KEYS: secret JSON map of key versions to 64-hex AES keys.
- CONNECTOR_TOKEN_KEY_VERSION: active version, default 1. Retain old keys while
  ciphertext still depends on them. Do not reuse SSO signing keys.
- CONNECTOR_OAUTH_CLIENTS: secret JSON map keyed by catalog auth.provider.
  Each client has client_id, optional client_secret, exact auth_url/token_url,
  explicit mcp_origins/scopes and approved extra_params. Optional revocation_url
  enables provider-side revocation attempts. Scopes must include all requested
  scopes from the reviewed pinned template; unauthorized extras are refused.

For Slack use its official user-only authorization and token endpoints. Review
https://meta.grengin.com/connectors/slack/versions/1.0.0.json for the exact scopes
and include extra_params={"response_type":"code"} in the operator allowlist.
The Worker consumes CONNECTOR_OAUTH_CLIENTS, not standalone SLACK_CLIENT_ID or
SLACK_CLIENT_SECRET bindings. Those source variables are used by local preflight.

Register the additional Slack redirect:

```text
https://service-staging.grengin.com/v1/connectors/callback/slack
```

Keep localhost:5173/mcp/oauth/callback if it is needed for legacy/self-configured
OAuth; it is not the service broker callback. The local preflight checks URI
construction, not Slack's app registration or the validity of the client secret.
Do not change the OAuth client ID of live connections as a secret-rotation step;
finish cleanup with the original app first. A client-ID change is fail-closed.

## Migrations And Rollback

- Backend: m20261028_000001_create_connectors, including the connectors table,
  uniqueness constraints and the user FK that preserves outstanding grants.
- Worker: workers/migrations/0018_connector_oauth_broker.sql. No admin-config
  migration or GUI table is required. Apply it to the selected regional D1
  binding, including EU_DB if the deployment has cut over to EU storage.

The backend migrates on startup by default. If automatic migrations are
disabled, apply the migration through the normal dedicated migrator first.
Review database backups and migration state before operator changes. Local tests
verify backend up/down/up and the complete local Worker migration chain.

To stop new broker use, disable CONNECTOR_BROKER_ENABLED. This is not credential
erasure or vendor revocation. Disconnect/clean retained credentials before
dropping tables or encryption keys. Reverting code while retaining additive
tables is safer than blindly running a destructive down migration. Do not
delete the backend grants while remote revocation is still pending.

Deleted local users are denied API access immediately. On a persistent single
backend instance, bounded cleanup retries their connections after broker/vendor
recovery, auditing system actions without impersonation. Deactivated users keep
their settings. Hard deletion cannot cascade away cleanup grants. The in-process
timer is not a durable scheduler for frozen Lambda instances; serverless rollout
requires an external execution/retention plan and is not covered by this claim.

## Staging Operator Runbook

These commands are for the operator; no shared deployment or remote migration
was executed as part of the local verification or branch publication.

### 1. Preserve Newer Main Changes

Check both feature branches against current main before replacing an existing
staging deployment. At publication preparation, backend main was one release-only
commit ahead (the 0.9.6 bump), while website main had 50 newer commits, including
workspace, billing and email changes. Those changes were deliberately not
merged into the locally verified feature branches. Do not deploy the older
website checkout over a staging service that needs those newer features.

Integrate current main into the respective feature branches through the team's
normal review process and repeat the local audit after resolving any conflicts.
This means merging main into the feature branches, not merging these features
to main. The test counts below apply to the tested connector branch revisions,
not to an untested future merge result.

### 2. Configure And Deploy The Service First

Run in grengin-website/workers. Keep the existing staging authentication,
installation and signing secrets; these are additional bindings:

```bash
pnpm install --frozen-lockfile
pnpm exec wrangler secret put CONNECTOR_TOKEN_KEYS --env staging
pnpm exec wrangler secret put CONNECTOR_OAUTH_CLIENTS --env staging
```

Enter secret JSON at the prompts, not as command-line arguments. Token keys have
the shape {"1":"<fresh 64-lowercase-hex AES key>"}; retain previous key versions
when rotating. The OAuth client map is keyed by provider, for example slack, and
must contain the actual client ID/secret and the exact endpoints, MCP origins,
scopes and approved extra_params described above. A standalone Slack secret does
not populate this map. Register the staging Slack callback before enabling it.

Set these non-secret values in [env.staging.vars] of workers/wrangler.toml:

```toml
CONNECTOR_BROKER_ENABLED = "true"
CONNECTOR_TOKEN_KEY_VERSION = "1"
SERVICE_BASE_URL = "https://service-staging.grengin.com"
```

Review backups and pending migrations before applying them. In the checked
staging configuration both DB and EU_DB select grengin-service-staging-eu, so
the existing staging migration command targets the EU primary, not production:

```bash
pnpm exec wrangler d1 migrations list DB --remote --env staging
pnpm db:migrate:staging
pnpm deploy:staging
```

Re-check that binding if the configuration changes. Verify the deployed scheduled
trigger runs connector maintenance; the repository's shared schedule is every
five minutes. Backend cleanup additionally needs a persistent running instance.
No website frontend deployment is needed for these Worker-only changes.

### 3. Build The Backend Feature Branch, Not A Stable Tag

The existing v0.9.6 image does not contain this feature. The release workflow
verifies that the catalog pins the exact backend commit, including on manual
feature-branch dispatches. A connector staging commit is not that released
commit, so do not use that workflow or rewrite a released catalog for this test.
The stable publication checks remain unchanged.

For a staging platform that builds directly from Git, select this feature branch
and its Dockerfile in the staging service, not the production service. Otherwise,
build on a native amd64 builder and push a separate commit-specific staging tag:

```bash
cd /home/anurag/work/grengin-api
revision=$(git rev-parse HEAD)
version=$(cargo metadata --no-deps --format-version 1 | \
  jq -r '.packages[] | select(.name == "grengin-api") | .version')
image="ghcr.io/grengin-oss/grengin-api:staging-connectors-${revision:0:12}"
docker build --platform linux/amd64 \
  --build-arg CARGO_BUILD_JOBS=2 \
  --build-arg IMAGE_VERSION="$version" \
  --build-arg IMAGE_REVISION="$revision" \
  --build-arg MIGRATION_HEAD="$(tr -d '\r\n' < migration/HEAD)" \
  --tag "$image" .
docker push "$image"
```

GHCR write authentication is required for the push. For a native arm64 staging
builder and host, use linux/arm64 instead; this is a single-architecture staging
image, not a multi-architecture release. Pin the resulting digest in deployment.
These are operator commands only; no Docker image build or push was executed
for this handoff. Never substitute latest or a stable version tag.

Set the backend service URL explicitly to https://service-staging.grengin.com
and supply the staging installation ID and its matching registered private seed.
That installation must be connected in the staging Worker database. Preserve
APP_KEY so existing encrypted records remain readable, keep PostgreSQL and
storage isolated from production, and apply the backend migration before serving
requests if GRENGIN_AUTO_MIGRATE=false.

Check GET / reports the expected and applied migration head as
m20261028_000001_create_connectors. Verify the five documented endpoints appear
in Swagger, then perform the manual consent, owner confirmation, direct tool use
and disconnect/retry checks above. A successful health response alone does not
prove that the Slack app or its OAuth callback has been configured correctly.

## Repeatable Local Checks

```bash
cd /home/anurag/work/grengin-api
bash tests/connectors/audit-core.sh

# Individual suites:
bash tests/connectors/run.sh
bash tests/connectors/slack-preflight.sh

cd /home/anurag/work/grengin-website/workers
GRENGIN_CONNECTOR_CATALOG_ROOT=/home/anurag/work/grengin-list/master-data/connectors \
  node --import tsx --test --test-concurrency=2 test/*.test.mjs
pnpm exec tsc --noEmit
bash test/migrations.sh
```

The API script sources the requested local secret files without printing values
and replaces DATABASE_URL with a disposable loopback database. It cleans its
servers, containers and scratch storage. Rust uses at most two jobs. Mock vendor
and model calls do not spend model credits. Preflight fetches public metadata
but does not contact Slack's token endpoint. GUI work was explicitly removed.

The backend branch is one release-only commit behind main (the 0.9.6 release
bump). Its local manifest still says 0.9.5; this is not a new released artifact.
Integrate main and repeat the audit before the staging build. No release,
shared migration or deployment was performed during these tests.
