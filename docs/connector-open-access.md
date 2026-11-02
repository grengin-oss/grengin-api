# Registration-Free Connector OAuth

The 2026-11-02 product decision supersedes the installation-registration gate
and the unshipped Hatchery managed-identity draft. Anyone running Grengin may
use the configured connector broker without connecting their instance to a
Grengin.com account. Backend users still sign in and approve vendor consent.

Remove GRENGIN_CONNECTOR_INSTALLATION_ID and GRENGIN_CONNECTOR_PRIVATE_KEY as
requirements. GRENGIN_SERVICE_URL selects staging/production. Each backend
connection already has a random 32-byte encrypted grant in its owning user's
connectors row; reuse that as a private Ed25519 seed, without transmitting it.
The public key is automatically bound to that connection at its first start.
There is no installation bootstrap, shared platform secret or manual key setup.

Version-2 proofs bind connection ID, local user ID, timestamp, nonce, method,
path and exact body hash. Only the public key and signature travel to the
broker. The original backend access token never leaves the backend. Private
seeds remain encrypted with APP_KEY. Vendor credentials remain encrypted in
the broker; callback state, PKCE, owner confirmation and token-use checks stay.

Use separate additive service tables for registration-free connections, proofs
and audit events. Retain the existing registered protocol and ciphertext/AAD
unchanged for old clients. A callback state prefix selects its storage scope;
never retry a rejected request against the other scope. Public starts need
bounded IP/global admission, pending-state capacity and expiry cleanup.
These controls are resource limits, not proof of a real installation or user.

The public backend /connectors API and snake_case payloads do not change.
The existing v0.9.7 image still requires installation env vars: this refactor
requires a backend rebuild/replacement and a service migration/deployment.
The earlier statement that no backend update was needed no longer applies.
No new Hatchery identity registration or vendor secrets in tenant instances.

Tests must cover operation/body/owner binding, replay, wrong signer, idempotent
start, callback/confirmation lifecycle, encrypted storage, service outages,
admission limits, old-client compatibility and real compiled API/MCP mock flow
without either removed environment variable. No automatic production deploy.

## Upgrade And Resource Boundaries

Deploy the service with additive 0028_open_connector_broker.sql first. Keep
0018 and its registered-client tables: old clients and old ciphertext continue
to work. There is no transparent migration of an existing registered connection.
Before upgrading that backend, disconnect its legacy connections using the old
client, then reconnect using the new client. An unexpected legacy ID returns
409 instead of acknowledging cleanup or silently dropping encrypted retry state.
No new backend migration is required; preserve APP_KEY and existing storage.

New-connection admission is 20 starts/IP/minute and 120/minute globally, after
signature validation and before vendor/catalog work. Idempotent starts use the
120 requests/minute/connection budget, not the new-connection quota. Unknown
disconnects are no-ops and do not allocate replay records. At most 10,000 live
open connections may be created. Old erased terminal rows and abandoned pending
approvals age out after one day; credential-bearing failures stay for cleanup.
IP hashes are retained only in short-lived admission buckets.

These limits bound resource use, not legitimate-user identity: anyone can create
a key. The public broker still needs an operator-reviewed edge abuse policy and
monitoring. Global capacity or admission exhaustion returns 429 with Retry-After;
there is no claim that an open anonymous service cannot be denial-of-serviced.

The superseded Hatchery managed-identity draft remains isolated on its own branch
and must not be deployed for this design. Hatchery and standalone VMs only need
the updated backend image and GRENGIN_SERVICE_URL for the appropriate service.
Vendor credentials remain service-side. Existing SSO behavior is unchanged.

## Return To The Instance

The backend now signs `redirect_uri` derived from REDIRECT_URL with the fixed
`/mcp/oauth/callback` path when starting connector consent. It never accepts a
return destination from the frontend request or forwarded headers. The broker
stores that destination in the encrypted pending payload. After consent it
redirects there with a browser-only confirmation receipt and original OAuth
state; no vendor tokens leave the broker. A compatible callback page verifies
its session-stored connection, state, active user and expiry, scrubs the URL
fragment and calls the existing authenticated completion API. No code is copied
by the user. See [the frontend contract](connector-oauth-broker-frontend.md).

This remains registration-free for public instances, with HTTPS required except
for loopback development. It requires deploying the patched service, backend
and compatible frontend together. Previous pending attempts retain their original
behavior and need disconnect/restart. The service keeps the manual code response
only for old starts that did not provide an instance return URL. No database
migration, additional instance keys, Redis or new backend API endpoint is needed.

The instance-return patch passed 760 workspace tests (61 ignored), a full build,
54 compiled API/Worker/OAuth/MCP/model HTTP checks and the migration round-trip
on disposable PostgreSQL. The matching Worker passed 656 tests (1 skipped),
typechecking, local migrations and dry-run bundling. The local frontend passed
19 helper tests, 9 intercepted browser checks and a production build. Its full
typecheck still reports the pre-existing ProjectDetailPage.svelte:45 error and
12 existing warnings; no new changed-file diagnostic was found. Browser tests
include automatic return/completion and rejection of a copied callback in a new
tab. No fresh real-vendor consent, VM replacement, push or deployment was done
for this patch. Test commands remain the two scripts below and the local
`/home/anurag/.local/state/grengin/webapp-vm-test/connectors.sh` wrapper.

## Repeatable Verification

Run the compiled backend, isolated database and mock OAuth/MCP/model HTTP flow:

```bash
cd /home/anurag/work/grengin-api
GRENGIN_WEBSITE_ROOT=/home/anurag/work/grengin-service-staging \
  bash tests/connectors/run.sh
```

Run service typechecking, tests, local migrations and a dry-run bundle:

```bash
cd /home/anurag/work/grengin-service-staging
bash workers/test/connector-staging.sh
```

These scripts do not deploy or run remote migrations. The backend test loads the
requested local secret files without printing them, then replaces DATABASE_URL
with a disposable PostgreSQL fixture. Vendor/model requests are mocked; a fresh
real Slack consent and tool-call smoke test remains a staging acceptance gate.

## Local Verification Result: 2026-11-02

Both checkouts are on feat/connector-open-access. Publishing these branches does
not deploy either component; neither shared service nor the VM image was updated
by the verification work below.

| Check | Result |
| --- | --- |
| cargo test --locked --workspace, two jobs | 758 passed, 61 ignored, no failures |
| cargo build --locked --workspace, two jobs | Passed |
| Compiled API / Worker / OAuth / MCP / model HTTP fixture | 53 checks passed |
| Connector migration up/down round-trip on disposable PostgreSQL | Passed |
| Full Worker suite, including old registered clients | 647 passed, 1 skipped, no failures |
| Worker typecheck, local migration chain and staging dry-run bundle | Passed |
| Rust formatting, shell syntax and both git diff --check runs | Passed |

The build still reports the existing proc-macro-error2 future-incompatibility
notice. Ignored/live-service tests are not counted as passes. The HTTP fixture
made three mock model calls, not paid provider calls, and left no fixture server
or database running. It creates no installation or organization rows.

Additional review fixes prevent unknown disconnects from allocating replay rows,
prevent legacy credentials from being falsely acknowledged as cleaned, move
new-start admission after proof validation, and avoid charging idempotent starts
against admission. Tests cover concurrent signer claims, owner/body/path binding,
replay/expiry, small-order keys, mixed protocols, capacity, encrypted AAD and
cleanup. The shared signing-helper regression is covered in the service tests.

Still required: service migration/deployment, rebuilt backend,
and a fresh real Slack consent/tool-call smoke test. Public abuse policy and
monitoring remain operator responsibilities; the tests do not prove availability
against a distributed denial-of-service attack or universal vendor compatibility.
