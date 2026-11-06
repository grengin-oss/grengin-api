# Connector Console Verification

Reviewed 2026-11-06 on `feat/connector-server-plugin` for backend **0.9.8**,
connector runtime **0.6.0**. Single-instance scope; no Redis or distributed
coordination is claimed. No frontend implementation is included.

## Changes Verified

- Removed the redundant mock-only connected-count route, DTO and Swagger entries.
  Use `overview.connected_user_count`; the admin list has no per-row count field.
- `/mcp-servers` filters servers and tools using the same access resolution as
  chat. An explicit tool grant can preserve access to an otherwise denied server.
- Picker/tool listings share one context, batched rules and one department-path
  query. SQL-count tests compare one versus thirteen department-scoped connectors:
  picker 15 SELECTs, tool list 13 SELECTs, with no growth in either listing.
- Execution pagination uses exact offsets, deterministic timestamp/UUID ordering,
  an offset ceiling of 1,000,000 and permission-before-existence checks.
- Topic groups are an optional manifest presentation field. Declared groups must
  cover every tool exactly once, without overlap, unknown tools or unsafe text.
  Legacy manifests keep their flat tool list.
- Overview returns group states (`enabled`, `disabled`, `mixed`). The new group
  switch resolves membership from the installed manifest, locks the server/tools
  and commits every enablement change with one audit event in a transaction.
  It cannot change credentials, read-only flags or access rules.

## Audit Findings Closed

1. Google Chat search allowed 2,000 characters although its current API allows
   1,000. The catalog schema now enforces 1,000; wire tests exercise 1/1,000 and
   reject 0/1,001/2,000 before any network request.
2. The Google Discovery scope checker calculated the root-scope fallback but
   compared method scopes instead. The fallback is now used and regression
   tests cover root-only scopes, method restrictions and undeclared scopes.
3. Analytics needs fixed action suffixes such as `{property_id}:runReport`.
   The runtime accepts only a fixed safe action in that position, retains colon
   prefix/malformed-action rejection and rejects argument-supplied path/query
   breakouts. Existing OData escaping and redirect credential tests still pass.

## Catalog Candidate

60 connectors, 521 read-only tools, 230 topic groups. The existing 55 providers
gain version 1.2.0; five Google apps start at 1.0.0: Ads, AdSense, Chat, Analytics
and Business Profile. Their setup declares per-user OAuth and provider-specific
requirements, never shared client secrets or service-account material.

Analytics has account/property/stream discovery, metadata, standard reports and
realtime reports, bounded limits and optional dimension filtering. It uses the
Analytics Admin/Data APIs, not the separately listed upstream stdio MCP server.

The user approved keeping the early-access 0.9.8 distribution rather than 0.9.9.
Its replacement must pin the final backend/catalog source commits. Previously
published versioned package bytes and older distributions remain unchanged.
Installed manifests require an explicit admin update; discovery installs nothing.

## Fresh Automated Evidence

- Workspace cargo tests: **891 passed, 0 failed, 76 ignored**. Ignored cases need
  explicitly selected external/live or cross-repository setups.
- Explicit cross-repository provider/business suites: **13 passed**, replaying
  all 521 tool request contracts with separate user tokens.
- Actual HTTP API against disposable PostgreSQL/local fixtures: **103 checks**.
  Includes ten topic-group checks: auth failures, unknown groups, rejected
  caller-supplied membership, multi-tool atomic changes, unrelated tools,
  mixed state, audit failure rollback, private headers and unchanged policies.
- Navikt mock OAuth through the backend: **715 checks, 0 findings**; 176 mock
  token requests and 173 local tool requests. Covers all OAuth catalog entries,
  PKCE policy, exact callbacks, replay/user binding, encrypted tokens, refresh,
  failure/retry, disconnect, configuration changes and chat calls.
- Catalog Python: **177 tests**; catalog/schema/digest validation passed.
- Official Google Discovery: **80 matched operations** across eleven connectors.
  Business Profile's two legacy v4 reads have no public Discovery document and
  remain separately reference-reviewed rather than Discovery-certified.
- Build, rustfmt, Node/bash syntax and both `git diff --check` checks passed.
  Clippy completes with existing repository warnings, including unchanged MCP
  helpers and llm-plugin SSE code. This is not a warning-free workspace.

Repeat with `bash tests/connector-plugin/review.sh` (two Rust compile jobs, no
imposed memory cap). The local test database/OAuth containers have small separate
limits and are removed, including SQL fixture logs. Existing demo servers,
databases, accounts and user frontend changes are not used or modified.

Evidence: `/tmp/grengin-connectors-final-pipeline.log`,
`/tmp/grengin-connectors-final-oauth.log`,
`/tmp/grengin-connectors-final-clippy.log`,
`/tmp/grengin-connectors-final-google-audit.json`.

## Limits And Deployment

No real vendor consent is claimed by this audit. Tenant admin approval, provider
licensing, scopes, property/resource membership and actual response compatibility
still need account testing. API documentation and mocks cannot grant permissions.
Office/PDF extraction, write/draft/send tools, write confirmations, reminders and
PII counters are not implemented. Stored connected counts are not vendor probes.

API handoff: `docs/connector-console-api.md` and
`~/grengin-docs/connector-changes-for-ui.md`. No database migration or Worker
deployment is needed for these changes. Backend branch deployment and the
separately reviewed early-access catalog replacement are required. No main merge
or stable release is implied.
