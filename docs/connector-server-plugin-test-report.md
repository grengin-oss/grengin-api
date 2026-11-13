# Connector Plugin Implementation And Verification

Historical implementation report. Current scope, fixes and verification are in the
[all-34 report](connector-plugins-all-34-report.md); the earlier counts below are historical.

Date: 2026-11-04. Branch: `feat/connector-server-plugin`.
Status: runtime and versioned catalog integration prepared locally, awaiting review.
Follow-up: the [Navikt OAuth audit](connector-plugin-oauth-navikt-review.md) now
records eight open failing probes in inherited MCP/OAuth code. The earlier green
baseline below does not cover those findings. The branch is not merge-ready.
Application version is 0.9.8 (user-approved local preparation); runtime is 0.1.0,
plugin packages are 1.0.0, manifest schema is 1. No new migration, commit, push,
catalog publication or shared deployment was performed. The 0.9.8 distribution
is an unpublishable draft with null source pins, not a staging-ready release.

## Implemented

- Internal `connector-server-plugin` 0.1.0 crate, modeled on `llm-plugin` and
  reusing its typed JSON mapping language and destination/header validation.
- Runtime contract `schema_version: 1`, semver plugin versions, generated JSON
  Schema, a Microsoft Graph example, and an authoring/integration README.
- Eight catalog provider packages, version 1.0.0: Outlook Mail, Outlook
  Calendar, OneDrive, SharePoint, Teams, Gmail, Google Calendar and Google Drive.
  Their 54 read-only tools have explicit scope declarations and installation docs.
  The follow-up [provider contract audit](connector-plugin-provider-audit.md)
  compares current official REST/MCP docs and categorizes tool coverage.
- Production manifests, immutable version snapshots, SHA-256 indexes and coverage
  inventory now live in `grengin-list/master-data/connector-plugins`, not in the
  backend crate. The backend retains generic authoring examples and test fixtures.
- Public `GET /discovery/connector-plugins` and detail `/{id}?version=1|1.0.0`
  routes use the backend-pinned optional `connectorPlugins` distribution family.
  Identity/version/schema/digest checks precede returning a manifest. Missing
  packages allow pinned-version fallback; invalid hashes and outages do not.
  ETag/304 works without sign-in and discovery never installs database records.
- In-process JSON HTTP tools: configurable verbs, path parameters, query,
  headers, JSON body and result mapping, vendor error flags and bounded next-link
  pagination. There is no shell, JavaScript interpreter or dynamically loaded code.
- Explicit installation through existing MCP administration APIs and JSON
  storage; no new table and no auto-installation of catalog entries.
- Atomic server/tool/audit writes. Existing tool IDs, disabled flags and access
  overrides survive manifest updates. Invalid updates preserve the old runtime.
- Per-server single-instance mutation lock through cache replacement; partial
  updates rebase explicitly changed fields onto the latest saved model.
- Existing MCP access checks, per-user encrypted OAuth connections and chat
  tool calling. A manifest never receives access to the token mapping context.
- Changed OAuth bindings clear saved tokens and pending states; plugin deletion
  removes its tools, access metadata, pending OAuth state and connection rows
  transactionally. Historical execution/audit records are retained.
- Backend and sqlx-mcp upgraded together from rmcp 0.15.0 to 3.8.0.
  SDK response caching is disabled. SDK-specific reqwest 0.13 is isolated from
  the existing reqwest 0.12 OAuth/LLM implementation.
- Docker dependency-cache/source-copy stages include the new workspace member.
  Pre-existing architecture fallback changes in that file were preserved.

## Verified Results

| Gate | Result |
| --- | --- |
| Workspace tests | 795 passed, 0 failed, 64 ignored |
| New crate runtime/schema/provider tests | 23 default + 3 explicit cross-repository tests passed |
| New backend plugin unit tests (included above) | 6 passed |
| New backend discovery tests (included above) | 4 passed |
| Plugin HTTP suite | 40 passed |
| Existing broker HTTP suite | 54 passed |
| Existing connector migration up/down/up test | Passed on disposable PostgreSQL |
| Workspace development build | Passed, one Rust job |
| New crate clippy, warnings denied | Passed |
| Workspace all-target clippy | Completed with repository warnings; none reported in the new connector crate or discovery/broker-admission changes |
| Formatting and diff whitespace | Passed |
| Catalog Python suite | 126 passed, including 13 new plugin/versioning tests |
| Catalog validation / JSON Schemas | Passed; 16 alias/version manifests also compiled by the actual runtime |
| Distribution generator / released-file guard | Deterministic committed-source generation, immutable rewrite refusal, existing versions unchanged |
| Unreviewed 0.9.8 draft | Rejected by release schema as intended (null source commit pins) |

Ignored workspace tests need external credentials/services, a cross-repository
catalog or explicit database setup. Three provider catalog tests were run
separately; the remaining ignored tests were not counted as passing. The final
0.9.8 distribution-runtime check needs reviewed source pins and is not yet runnable
against a published index. The existing proc-macro-error2 future
compatibility warning remains; this work did not suppress it.

Plugin HTTP tests run the actual backend with a new pgvector/PostgreSQL 16
container (384 MiB limit), fresh local APP_KEY/JWT secrets and mocked vendor/model
servers. They verify authenticated installation and denied non-admin installs,
automatic tool metadata, encrypted secrets, missing-auth denial, two-user token
isolation in real chat streams, MCP access policy enforcement, preservation of
tool IDs/access settings, failed validation/audit rollback, concurrent partial
updates, restart reload, OAuth-binding invalidation and transactional deletion.
The expanded suite also checks invalid OAuth installs leave no server behind,
explicit sync preserves disabled tools/access overrides, and revoking access,
disabling a tool or deactivating its user during model generation prevents the
later vendor request.
Six HTTP checks cover public connector discovery, exact package hashes, ETags,
invalid selectors/missing providers and absence of automatic database installs.
Two more reject custom-plugin admission to the hosted OAuth broker before a
grant can be written.

The provider contract matrix exercises every one of the 54 tools against a
local HTTP server with two different users. It checks exact HTTP method, base
path, encoded IDs, default/explicit query parameters, headers/body and bearer-token isolation.
Full vendor response envelopes retain pagination cursors. Every tool also rejects
missing credentials and unknown input properties. These are protocol mocks, not
real account consent or proof of vendor availability.
Categories are derived from annotations, not HTTP verbs. Tests classify the
read-only Calendar free/busy POST separately from the authoring example's draft
write, and ensure the reviewed tool inventory matches actual manifests/scopes.

The updated sqlx-mcp executable is exercised over real stdio with both
2024-11-05 and 2025-11-25 clients: initialization, tool listing and SELECT query.
The existing broker suite exercises actual HTTPS Streamable HTTP, direct tool
calling, callback/confirmation, replay protection, expiry, revocation, refresh
and error translation with local fixtures. The Worker source was read from
`f6a798af0bb2287de593402c579710851cd19675` without changing that checkout.

Your normal database, file storage, running frontend/backend and saved vendor
credentials were not used or modified by these new suites. Fixture processes,
containers and temporary files were removed. No paid model calls were made.

## Problems Found During Implementation

1. Mapping paths initially used connector-specific roots. Tests rejected them;
   the implementation now uses the existing `request`, `config` and
   `session.response` contract instead of changing LLM behavior.
2. The inherited server delete path left orphaned tool rows. The HTTP regression
   reproduced this; plugin-specific deletion now cleans related metadata and
   sessions in the same transaction as the audit entry. Forced audit failure
   confirms that deletion rolls back.
3. Overlapping admin writes could publish stale runtime/configuration state.
   Single-instance mutation locks now include cache replacement and partial
   updates use freshly loaded unchanged fields. A delayed concurrent-update
   test verifies the newer manifest survives a later name-only update.
4. The broker test served a CA certificate as its TLS leaf. The updated SDK
   correctly rejected it. The test now creates a server-auth leaf with CA:FALSE;
   certificate verification remains enabled.

## Follow-up Audit Findings And Fixes

1. **High: stale authorization during model generation.** Chat checked a cached
   descriptor but executed the current runtime. Admin revocation or a changed
   tool annotation could therefore be ignored. Immediately before dispatch,
   [the plugin integration](../src/services/mcp_plugin_helpers.rs) now checks the
   active user, current enabled server/configuration, enabled tool and live MCP
   permissions. Three HTTP tests revoke access, disable the tool or deactivate
   the user while the mock model is paused; none sends a vendor request.
2. **Medium: explicit sync re-enabled disabled tools.** The generic MCP sync path
   unconditionally enabled known tools. Plugin sync now uses its preserving
   metadata updater, with the configuration lock and transactional audit write.
   The live test retains both the existing tool UUID and its disabled/access flags.
3. **Medium: incomplete OAuth installs could be accepted.** An empty OAuth object
   previously passed compile-time validation. The install now requires valid
   endpoints, secure redirect (loopback HTTP allowed), nonempty valid scopes,
   client ID and a parsable existing OAuth configuration. Tests reject missing
   fields, insecure/metadata endpoints, invalid redirects/scopes, missing client ID
   and unsupported flow before inserting anything.
4. **Medium: mapped expansion could exhaust resources before the output check.**
   Mapping was synchronous and unbounded, so an async timeout alone was not a
   resource guard. An opt-in evaluator now charges steps and estimated allocation
   before copying/expanding values. Connectors use 50,000 steps and 4 MiB per
   expression; URL/custom-header limits are 16 KiB each. Nested-expansion and
   oversize request tests fail before networking. Existing LLM callers retain the
   original evaluator and all their tests pass.

The first expanded HTTP attempt used a binary whose build had started before the
last OAuth-validation edit, and failed its new unsupported-flow assertion. The
complete rerun rebuilt the final source and passed all 32 checks. No failed test
was removed or weakened. The repeat script now matches the broker suite's test
incremental setting to avoid an unnecessary second backend test build.

## Catalog Relocation Follow-up

- **Broker isolation gap, fixed:** the separate `/connectors/{key}/connect`
  endpoint accepted a plugin server past its admission check even though plugin
  creation rejected broker flags. A new HTTP test reproduced 503 from the
  unconfigured broker instead of an early 404. The common `allowed_server` guard
  now refuses plugin configurations on broker start/status/token access. The
  final tests verify 404 and zero saved connector grants. This reproduced an
  admission gap, not a demonstrated vendor-token theft.
- **Validator output collision, fixed:** both crates initially named their
  example `validate_manifests`. The connector example is now
  `validate_connector_manifests`, including test and CI calls.
- **Memory:** two compiler attempts were stopped by a 3 GiB cgroup limit, not by
  failing assertions. A sequential one-job run under a 4 GiB limit with swap
  disabled completed. No other user processes or existing build directories
  were removed. Tests use existing fixtures, not paid API calls.
- **Source pinning:** the catalog generator reads exact committed indexes.
  The preparation helper writes only `grengin-list/docs/drafts/grengin-api/0.9.8`.
  No fabricated source commits or publishable 0.9.8 index were introduced.

## Security Controls Tested

- Reject unsupported schema versions, duplicate tools, unknown fields, unsafe
  headers, invalid JSON Schemas and external schema references.
- Reject unsafe URL schemes, private destinations by default, cloud metadata
  even with private-network approval, and path-parameter traversal.
- DNS validation is performed by the resolver supplying actual connection
  addresses; environment proxies are disabled. Redirects never forward tokens.
- Pagination cannot change origin or escape the configured base path; cycles,
  invalid responses and exceeding limits fail explicitly.
- User credentials are passed per invocation, not cached in a shared manifest
  or client header. Public tools do not forward an accidentally supplied token.
- Response sizes and concurrent requests are bounded; cancellation releases
  runtime capacity. Mutating calls are never retried automatically.
- Vendor error bodies are not exposed by the new runtime.
- Custom manifests cannot claim a centrally approved broker connector identity.

## Explicit Limits And Follow-ups

- This is a single-instance implementation, not distributed coordination.
- The Graph example compiles and validates; real Microsoft consent, tenant
  policies and actual Microsoft tool calls have not been tested.
- The eight new providers are read-only, mock-tested plugins with partial catalog parity. Google
  paginates with explicit page-token arguments; Graph lists preserve next links
  but fetch one page. Gmail does not decode full email bodies; Teams reads
  messages only with the relevant additional scopes. Drive/OneDrive do not
  download file bytes. The catalog mapping has 38 comparable read scenarios,
  9 partial and 27 deferred. All 95 catalog write/delete entries remain deferred.
- Slack's direct REST plugin is deferred because its nested per-user OAuth token
  response is not supported by this path. Existing Slack broker support is intact.
  Google Ads/Analytics/Business need additional reviewed contracts; see the
  provider guide in `grengin-list/docs/connector-plugin-providers.md`.
- Existing Google/Slack broker flows are regression-tested with mocks, not
  newly re-consented against the real providers in this change.
- Initial plugins use administrator-configured OAuth apps or no vendor auth.
  First-party Grengin broker bindings for custom plugins still require a separate
  reviewed implementation. Catalog discovery is now implemented, but its 0.9.8
  release pins/publication remain pending. No cloud connector was converted.
- `required_scopes` describes vendor requirements; it does not grant access or
  replace provider-side scope enforcement or Grengin MCP permissions.
- No multipart/binary responses, cursor/body pagination, API-key authentication
  or JSON-configured executable stdio plugins yet. Existing executable MCP
  servers continue to work through their current transport.
- Already-dispatched HTTP calls can finish after a permission/configuration
  change. The pre-dispatch check is not cancellation of a request already sent.
- Docker/musl and ARM64 builds were not run. Only Dockerfile workspace wiring and
  the native development build were checked.
- No frontend plugin-upload screen was added.
- This is not a completed audit of every legacy MCP/OAuth path. Earlier review
  questions about disabled-server tool listings and fresh direct-OAuth callback
  authorization remain outside the catalog-relocation verification. Do not
  present this report as unconditional merge approval for the entire branch.

## Repeat

```bash
GRENGIN_CARGO_JOBS=1 GRENGIN_TEST_CODEGEN_UNITS=64 \
  bash tests/connector-plugin/run.sh --http --broker \
  --catalog ../grengin-list/master-data/connector-plugins
# In the grengin-list checkout:
bash scripts/test_connector_plugins.sh
```

Requires Docker, psql, OpenSSL, Node, Cargo and the existing website checkout's
installed dependencies for the optional broker check. Commands use two Cargo
jobs at most (one in the verified low-memory run) and local fixtures, without
sourcing real provider keys. The catalog publication workflow validates packages
against a reviewed backend runtime ref and still requires explicit manual R2
publication approval; it was not dispatched.
