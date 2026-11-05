# All 34 Connector Plugins: Audit And Local Verification

Date: 2026-11-05. Backend branch: feat/connector-server-plugin.
Catalog branch: feat/connector-listings. Everything remains local and uncommitted.
No merge, push, R2 upload, Worker deployment, production DB change or Docker build.

## Result

All 34 IDs in the existing cloud catalog now have internal read-only packages:
**206 tools**, with one explicit request fixture per tool. There are no placeholder
MCP proxies or advertised unimplemented write tools. Coverage means all 34 provider
IDs, NOT every tool exposed by each vendor's official MCP service.

The implementation is ready for user audit and real-credential staging validation.
Mocks are not evidence that vendor consent, scopes, licenses, app approval or all
account-specific API responses work. Do not describe this as production certified.

## Verification

| Gate | Actual result |
| --- | --- |
| cargo test --workspace | 805 passed, 0 failed, 64 ignored live/optional tests |
| Explicit external catalog tests | 3 passed; 206 tools x 2 users = 412 local HTTP contracts |
| Native plugin/backend HTTP suite | 40 passed |
| Navikt OAuth/API-key/chat suite | 427 passed, 0 findings; 104 actual mock token requests, 101 local tool calls |
| Existing broker regression | 54 passed; isolated connector migration round trip passed |
| Catalog Python suite | 127 passed |
| Catalog generator and shell validation | Passed; all 34 plugin/setup packages are pinned by exact digests |
| Plugin/distribution JSON Schema checks | Passed; null-source-pin draft is correctly rejected as a release |
| cargo build --workspace | Passed, no Docker build |
| Runtime strict clippy | Passed with -D warnings |
| Workspace all-target clippy | Completed; repository warnings remain, not a warning-free claim |
| cargo fmt / git diff --check / bash -n / node --check | Passed |

Cargo also reports the existing third-party proc-macro-error2 future-compatibility
notice. The 64 ignored tests were NOT silently counted as passing. The 3 explicit
external catalog tests are reported separately from the 805 workspace tests.

## Packages

Backend compatibility preparation: **0.9.8**. Runtime crate: **0.2.0**.
Every provider's first unpublished package: **1.0.0**, schema_version **1**.

| ID | Tools | Auth |
| --- | ---: | --- |
| asana | 6 | oauth2 |
| box | 6 | oauth2 |
| canva | 6 | oauth2 |
| confluence | 5 | oauth2 |
| dropbox | 6 | oauth2 |
| github | 9 | oauth2 |
| gmail | 7 | oauth2 |
| google-calendar | 5 | oauth2 |
| google-drive | 5 | oauth2 |
| hubspot | 7 | oauth2 |
| intercom | 6 | oauth2 |
| jira | 6 | oauth2 |
| linear | 6 | oauth2 |
| mailchimp-transactional | 5 | api_key |
| microsoft-teams | 14 | oauth2 |
| microsoft-work-iq | 5 | oauth2 |
| notion | 6 | oauth2 |
| onedrive | 4 | oauth2 |
| outlook-calendar | 4 | oauth2 |
| outlook-mail | 5 | oauth2 |
| paypal | 4 | api_key |
| quickbooks | 6 | oauth2 |
| salesforce | 6 | oauth2 |
| sentry | 6 | oauth2 |
| sharepoint | 10 | oauth2 |
| shopify | 6 | oauth2 |
| slack | 7 | oauth2 |
| square | 6 | oauth2 |
| stripe | 7 | oauth2 |
| supabase | 5 | oauth2 |
| webflow | 5 | oauth2 |
| wix | 4 | api_key |
| wordpress | 5 | oauth2 |
| xero | 6 | oauth2 |

Production JSON lives only in grengin-list/master-data/connector-plugins. The backend
contains runtime code, authoring examples and test fixtures, not embedded production
provider packages. Existing released backend distributions have not been modified.
The 0.9.8 draft remains outside master-data with both source pins null until review.
No catalog entry is automatically installed in an application's database.

## Generic Runtime Work

- Validate required non-secret configuration before server installation/update.
- Support constrained tenant URL overrides for Salesforce and Shopify.
- Inject credentials privately as Bearer, custom headers or a JSON-body field.
  Secrets are never mapping inputs; mapped headers/body fields cannot override them.
- Keep fixed GraphQL read documents for Linear and Shopify; HTTP 200 errors fail
  explicitly. No model-supplied GraphQL or arbitrary write queries are accepted.
- Add encrypted, per-user API-key/token storage using the EXISTING mcp_connections
  table and POST /mcp/connections/{server_id}/credentials. Access, rate limiting,
  required audit writes and no-store responses apply. No new migration/table.
- Chat now loads those per-user API credentials, not only OAuth connections.
- Extend generic OAuth configuration with JSON token requests, comma-separated
  authorization scopes and separate initial/refresh JSON-pointer selectors.
- Detail discovery returns a digest-verified installation descriptor, not secrets.

Runtime limits remain 100 tools/package, 256 KiB wrapper, 128 KiB arguments/body,
1 MiB response/result, 30-second total call timeout and 16 active calls per installed
connector. No Redis, external MCP process or shared vendor-response cache is added.
Tests use one Cargo job under a 4 GiB no-swap scope; Navikt and PostgreSQL containers
are capped at 384 MiB each. Only disposable test containers/DBs are removed.

## Security Findings Closed

The eight deliberately failing probes from the earlier Navikt review now pass:

1. Disabled servers expose no usable tool listing.
2. A disabled-server callback cannot install tokens.
3. OAuth start checks active user and current MCP access.
4. Access revoked during consent is checked again before persistence.
5. Duplicate callbacks cannot delete the valid in-flight attempt's state. Existing
   single-instance per-connection locks cover lookup/exchange/cleanup/store; no DB
   connection is held over the provider network call.
6. Empty, whitespace/control-containing or oversized access/refresh tokens fail.
7. Negative, malformed or overflowing expiry fails instead of becoming unlimited.
8. Provider descriptions/raw token error bodies do not enter backend logs.

The suite additionally verifies ownership, anonymous/replayed/expired states, PKCE
mismatch, exact callback URI, three client auth modes, retry after an unreachable
endpoint, transactional token/state storage, refresh/disconnect races, rotated
refresh-token handling, invalid OAuth bindings and malicious redirect rejection.

The three API-key providers are exercised through the real credential API and chat:
anonymous/invalid/expired inputs, encrypted storage, user isolation, no-store/no-secret
response, access denial and disconnect. Request-body audit redaction includes
credential. Vendor APIs are local fixtures, not real accounts.

## Additional Bugs Found During Completion

- Missing optional query/body fields caused mapping failures. Packages now explicitly
  omit absent parameters; all 206 wire contracts pass.
- API-key plugins were installed but chat skipped their saved credential lookup.
  Fixed and covered by a regression test plus actual Mailchimp/PayPal/Wix chat calls.
- Slack initial consent puts the user token under authed_user, but refresh can put
  it at the root. Separate selectors handle both without a bot-token fallback.
  All 31 OAuth providers now test expiry-triggered refresh, not just first consent.
- GitHub requests lacked its required User-Agent. It is declared and asserted in
  all nine GitHub request fixtures.
- New Xero apps need granular invoice/bank-transaction read scopes. Setup and tool
  metadata now use those scopes instead of the deprecated broad transaction scope.

## Limits And Real Credential Checklist

- Read-only scope is unchanged. Full official-MCP tool parity, write/delete tools,
  binary downloads and cursor-autopaging are NOT implied. Returned cursors/envelopes
  remain visible; reviewed list tools provide their documented page inputs.
- Work IQ is explicitly Graph-backed Microsoft 365 search, not Microsoft's proprietary
  engine or a licensing bypass. WordPress targets WordPress.com/Jetpack, not every site.
- PayPal currently accepts an expiring merchant REST token obtained separately via
  client_credentials. It does not automatically mint/renew that token; pass expires_at
  and renew it as an operator. Wix needs an account-level API key; Mailchimp needs a
  Transactional key. These are not fabricated interactive OAuth flows.
- Notion/Intercom permissions are configured in their app dashboards; empty scope
  arrays are intentional. Slack's default confidential web setup keeps PKCE off;
  its optional public-client PKCE mode needs vendor app settings and no client secret.
- Tenant-specific configuration still needs real values: Atlassian cloud_id, QBO
  realm_id, Xero tenant_id, Wix account_id, Salesforce instance and Shopify shop URL.
- Vendor app approval, Google verification, Microsoft admin consent, API enablement,
  rate quotas, paid account entitlements and production response variations need live
  testing. Credentials cannot bypass a vendor's product restrictions.
- Existing broker disconnect semantics and the frontend retry UX remain their own
  previously documented concerns. This task does not add frontend pages or deploy services.

## Reproduce

From grengin-api:

```bash
GRENGIN_CARGO_JOBS=1 GRENGIN_TEST_CODEGEN_UNITS=64 \\
  bash tests/connector-plugin/run.sh --http --broker --oauth-navikt \\
  --catalog ../grengin-list/master-data/connector-plugins
```

From grengin-list:

```bash
python3 -m unittest discover -s scripts/tests
bash scripts/test_connector_plugins.sh
bash scripts/validate_catalog.sh
```

The exact primary API reference is recorded per tool in coverage.json; versioned
setup.json contains the provider-specific configuration. Relevant late-review
references: [Slack rotation](https://docs.slack.dev/authentication/using-token-rotation/),
[Slack PKCE](https://docs.slack.dev/authentication/using-pkce/),
[Xero granular scopes](https://developer.xero.com/faq/granular-scopes), and
[GitHub OAuth](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps).
