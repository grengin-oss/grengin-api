# Native Expansion Verification

Local, uncommitted work on `feat/connector-server-plugin` in grengin-api and
grengin-list. No frontend edits, migrations, secret changes, pushes, deployments
or R2 publication. This extends the previous connector-console review candidate.

## Implemented

- Ten OAuth apps added locally: Google Sheets (6), Google Docs (5), Airtable (5),
  ClickUp (10), monday.com (8), Figma (7), Zoom (5), Zendesk (7), Trello (6),
  Mailchimp Marketing (8). Six general-purpose apps now extend this: Google
  Contacts (6), Microsoft To Do (5), Todoist (8), Miro (5), Calendly (6), Typeform (6).
  Total native inventory: 50 plugins / 309 read-only tools. GitLab/Bitbucket's
  unpublished candidates were removed at the user's request, not released.
- Versioned plugin/setup pairs, primary per-tool references, native listing,
  read/write groups, setup hashes, genuine sanitized brand artwork and provenance.
- Runtime 0.3.0 supports safe static resource:action paths. Extension-bearing
  paths use the existing mapping DSL rather than new URL interpolation rules.
- Typed opt-in JWT expiry hints; default opaque token behavior is unchanged.
  JWT claims are not used to authorize identity or considered signature-verified.
- Separate same-origin refresh endpoints and an optional omission of grant_type,
  preserving configured form/JSON and Basic/post authentication. OIDC rejects
  these custom refresh options rather than silently ignoring them.
- Basic-auth token requests can explicitly include only client_id in the body
  for Calendly; the secret is still sent only via Basic. Defaults are unchanged.
- Google Contacts exposes the documented empty-query warmup before searching.
  Microsoft To Do uses delegated Tasks.Read on /me/todo only; Todoist uses API v1.
- New first-party v1 packages cannot introduce write/destructive tools or
  PUT/PATCH/DELETE requests. No v2 packages or write-confirmation claims added.
- Full Navikt integration harness extended for the larger native catalog,
  argument-requiring tools, raw Authorization values and separate refresh routes.
  This modified full harness has NOT executed against a fresh backend binary.
- All HTTP/OAuth harness entry points now reject binaries older than source.

## Executed Checks

| Command | Result |
| --- | --- |
| console.sh check | Workspace/all-target type check passed under MemoryMax=2G, no swap, one Rust job |
| console.sh runtime | Latest cache-audit run: 66 passed, five external/mock tests excluded here and run explicitly below |
| console.sh catalog | All 4 external catalog tests passed: schema/runtime compilation, inventory/scopes, all 309 HTTP contracts for two distinct users, new identifier/page bounds |
| oauth-navikt.sh protocols | All 47 OAuth setups completed code exchange and refresh through the actual backend transport source; mocked endpoints/scopes, not real vendor consent |
| scripts/test_native_expansion.sh in grengin-list | Catalog (16), expansion (8), artwork (5) tests plus deterministic generation and immutable hash checks passed |
| connector-related Python discovery | 75 passed after the final six; separate expansion suite adds 8, for 83 checks across these groups |
| cargo fmt --all -- --check | Passed |
| cargo clippy -p connector-server-plugin --all-targets --no-deps | Passed without diagnostics in the capped run |
| node --check oauth-navikt.mjs / bash -n | Passed |
| git diff --check, both repositories | Passed |
| console.sh local-compatibility | Gmail/Notion still 200 / 1.0.0 on the old running API |
| console.sh build | FAILED again after final six: run-u1171393.service, Result=oom-kill, MemoryMax=2147483648, MemorySwapMax=0. No higher-memory retry |
| console.sh http | Correctly refused stale binary before creating test resources |
| Later user-approved 4 GiB build/unit | Fresh build passed; 853 workspace tests passed, zero failed, 66 explicitly ignored |
| Later fresh HTTP/full Navikt | 65 HTTP checks and 619 full OAuth/backend checks passed after fixing native/broker console database column aliases |
| Existing broker regression | 54 checks and isolated migration round trip passed; service source used read-only archive |

The backend's same-origin refresh validation, JWT-expiry selection, Figma refresh
requests and Calendly client_id body tests have now executed through the lightweight
actual-source transport target. Miro query-grant/form-refresh and scope-control
regressions were added. This supersedes the earlier type-check-only result for
those unit tests, but not the separate full-backend build/HTTP gate.

The earlier build OOM did not mean the small runtime tests failed. That gate was
subsequently closed after the user authorized 4 GiB: fresh-backend HTTP/OAuth checks
now pass. These new results, not the old running binary's historical results, are
the staging-test evidence. Full vendor/catalog maturity is a separate open gate.
The current cache contract, exact test
boundaries and remaining gates are recorded in
[the session-cache audit](connector-session-cache-audit-2026-11-05.md).

## Boundaries

The expanded candidate is grengin-list's
`docs/drafts/grengin-api/0.9.8/connector-console-index.json`, with null source pins.
The active fixture `index.json`, all tracked 1.0.0 release files and existing
released distributions remain unchanged. Local ports 8080, 8083 and 5173 still
return 200; old real consent sessions were not modified.

The sixteen new apps have not had real credential/consent tests. Contract fixtures
exercise serialization, auth isolation and limits, not provider entitlements or
exact vendor-emulator fidelity. Native shared-broker support is still separate.
Mailchimp datacenter routing remains installation-scoped; Wix's installation
OAuth adapter remains deferred by explicit user decision. V1 is main read-only
workflow coverage, not exhaustive vendor API coverage.
