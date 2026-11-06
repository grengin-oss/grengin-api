# Connector Session Cache Audit

Date: 2026-11-05. Local, uncommitted work on `feat/connector-server-plugin` in
grengin-api and grengin-list. Single-instance scope, no Redis. Nothing committed,
pushed, merged, deployed or uploaded to R2. No real credentials were used by these
tests; existing frontend, databases and real consent sessions were preserved.

## Findings And Changes

| Finding | Resolution | Regression evidence |
| --- | --- | --- |
| The remote HTTP MCP session cache treated 256 retained users as a capacity ceiling | Typed, configurable LRU/idle-TTL retention with reconnect-on-demand; distinct bounded work admission | 1,024 identities through four retained entries; 300 concurrent real SDK clients with eight operation permits/four retained entries |
| Dropping an active cache entry could create a second session for the same user | Weak active-slot lookup survives strong-cache eviction; per-key lease serializes that user, not everyone | Eviction during an active lease preserves its value for a queued request; another user's slow call does not block peers |
| Closed SDK sessions could stay selected while their token was unchanged | Recreate the session when the SDK reports closure, not only on token rotation | Closed session, explicit invalidation and token rotation each produce a new handshake with exactly one tool call per requested invocation |
| Successful disconnect did not release its cached remote session | Invalidate only after the connection database write succeeds | Cache/transport invalidation tests pass; actual DB disconnect route still requires the fresh-backend HTTP gate below |
| Miro's new unpublished setup used the wrong code-grant format | Support typed POST query grants independently of refresh format; Miro code=query, refresh=form | Local wire-format assertions, unreachable-endpoint secret canaries, Navikt Miro exchange/refresh, catalog setup validation |
| OAuth scope checks allowed non-whitespace controls | Reject all control characters as well as empty/whitespace/oversized scopes | Actual production config tests reject NUL/newline/space and private OAuth destinations |
| Full backend test compilation exceeded the initial 2 GiB cap | Add an actual-source transport test target; after the user raised the cap to 4 GiB, also build and test the real backend | Lightweight target uses production modules; the fresh full workspace and HTTP suites now also pass |
| Connector console People returned 503 once connections existed | Explicit camelCase database aliases on native and broker row projections | The first fresh HTTP run reproduced the error; after correction, native people, broker people/overview and revoked broker account checks pass |

The lightweight transport target remains useful for quick checks, but is no
substitute for the now-executed database, route and permission tests below.

## Cache Contract

Native connectors already share a compiled manifest and a credential-free HTTP
pool. They do not retain one SDK session per user. Every invocation receives that
user's token after the existing database/access checks. There is no new decrypted
OAuth-token cache, browser-session cache, result cache or cross-user response cache.
The encrypted database connection remains the source of truth for refresh and
disconnect. Existing single-instance refresh locks and conditional token saves
were preserved; this audit did not remove the database check for performance.

Remote authenticated MCP uses a separate reusable `SessionCache<K, V>` with
`SessionLease<V>` RAII ownership. Installed remote server configuration:

```json
{
  "transport_config": {
    "session_cache": {
      "max_entries": 256,
      "idle_ttl_seconds": 300,
      "max_in_flight": 64,
      "wait_timeout_seconds": 30
    }
  }
}
```

These are defaults, not a 256-user limit. Validation bounds are 0-16384 retained
entries, 1-86400 idle seconds, 1-1024 in-flight operations and 1-120 wait seconds.
Zero entries disables idle retention. Unknown policy keys or invalid types fail
closed. This remote-only configuration must not be mixed into a native manifest.

- LRU eviction releases idle retained sessions. Active/queued requests own their
  slots independently, so eviction does not corrupt their token/session binding.
- A per-user mutex is acquired before a work permit. Same-user waiters do not
  consume every operation permit. No cache-wide lock is held across network I/O.
- Admission waits are bounded; a timeout returns a retryable busy error and later
  requests can proceed. Cancellation releases guards and permits.
- Idle expiry and dead weak-index cleanup are lazy, triggered by later lookups,
  at intervals no longer than the smaller of ten seconds or the configured TTL.
  This is a retention budget, not an exact total-byte cap or a background sweeper.
- Active leases can exceed the idle-retention count. Waiters also consume memory
  until completion/timeout. Increasing retention or concurrency still requires
  deployment-specific memory sizing; 300 concurrent tests are not a production SLO.
- The extra token comparison copy is a SHA-256 digest, not a plaintext token.
  The SDK necessarily retains its authentication material while a session lives.
- Disconnect or an admin update cannot recall HTTP work already dispatched.
  Completed disconnect clears the retained session; subsequent requests still
  check database state. Vendor-side consent revocation is a separate concern.
- The cache adds no tool replay/retry. A closed connection is recreated before
  the next requested operation. Underlying SDK protocol recovery is unchanged.
- Native plugins retain their separate 16-call-per-installed-connector limit and
  30-second call timeout. Those are resource backpressure, not a user-count ceiling.

## Executed Tests

Reproduce with `bash tests/connector-plugin/session-audit.sh` from grengin-api.
It needs the sibling grengin-list checkout, Docker and the systemd user manager.
The script stops on a failed gate. It does not modify credentials or start a
replacement app. No paid calls or external provider API keys are involved.

| Gate | Scope |
| --- | --- |
| Runtime tests | 66 passing: 10 library, 26 actual-source transport/config, one tool-category and 29 HTTP/mapping/security tests; five integration tests are excluded here and executed separately below |
| Explicit external catalog tests | Four passing: 50 manifest/setup pairs, 309 tools across two fixture identities, inventory/scopes/categories and invalid argument/page bounds |
| Navikt protocol test | One passing Rust test exercises all 47 OAuth setups through code exchange, token parsing and refresh |
| Catalog expansion script | 16 catalog tests + eight expansion tests + five icon tests, deterministic generation and immutable bytes/digests |
| Broader connector Python suite | 75 connector-related tests; overlaps the catalog/icon groups above rather than adding 75 unique checks |
| Workspace check | `cargo check --locked --workspace --all-targets -j 1`; known third-party proc-macro-error2 future-compatibility notice remains |
| Focused lint | Actual-source transport plus runtime crate: `cargo clippy --locked -p connector-server-plugin --all-targets --no-deps -j 1` |
| Formatting/syntax | Rust formatting, Node syntax, bash syntax, both repos' `git diff --check` |
| Full backend build | Passed at user-approved 4 GiB/no swap/one job, including the console row-projection fix |
| Full workspace tests | 853 passed, zero failed, 66 explicitly ignored live/external cases |
| Fresh backend HTTP suite | 65 checks passed on disposable PostgreSQL; permissions, encryption, console projection/counts, tool audit rollback, token modes, concurrent admin edits and actual chat tool calls |
| Full Navikt/backend suite | 619 checks, zero findings; 152 mock token requests and 149 local tool calls; ownership, replay, expiry, PKCE mismatch, outage retry, refresh/disconnect race and secret-redaction probes |
| Existing broker regression | 54 end-to-end checks plus isolated connector migration round trip, against a read-only archive of the matching website branch; no shared-service changes |

The 300-client test uses the real rmcp client against a local JSON-RPC server. The
server asserts that every bearer token matches that call's owner argument and the
caller checks the structured result. Cache tests also exercise disabled retention,
idle expiry, active eviction, cancelled waiters, admission timeout and recovery.

Navikt is `ghcr.io/navikt/mock-oauth2-server:6.0.2`, locally resolved to
`sha256:b538810afd589d42fbfb856c588c2065eaeed1dc528d6c532972048e67fc2aff`.
The protocol test replaces vendor endpoints and scopes with local issuers and
`openid`/`offline_access`. A local adapter normalizes grant formats to Navikt and
wraps Slack's user-token response. Explicit assertions preserve Calendly's Basic
plus client ID body, Figma's refresh-only form, Miro's query/form distinction and
monday's opt-in JWT expiry hint. This does not emulate vendor consent screens,
real scopes/entitlements, database callback ownership, replay controls or browser UI.

Core Rust commands run with one job, `MemoryMax=2G`, no swap. Protocol-only Navikt splits the
budget: 1536 MiB Rust + 384 MiB Docker/no swap. Its disposable container is removed
after testing; pre-existing containers remain running. Python uses a 256 MiB cap.
The first combined script run stopped at an unnormalized systemd working-directory
path; canonicalizing the sibling checkout fixed that harness error. The last
2 GiB build was OOM-killed (`run-u1179591.service`). The user then explicitly
authorized up to 4 GiB: full build/tests use that cap, full HTTP uses 3 GiB plus a
384 MiB disposable PostgreSQL, and full Navikt uses 3 GiB plus two 384 MiB
containers. All have swap disabled. Test scripts now accept only 2G/3G/4G caps.

## Release Gates And Limits

The earlier build/HTTP gate is closed for the tested single-instance backend:
it is suitable for controlled staging tests, not unconditional release approval
or proof that every vendor integration works. The new catalog tool audit found
separate scope/default bugs and main-workflow gaps. See grengin-list's
`docs/native-readonly-tool-audit-2026-11-05.md` before advertising mature coverage.
`bash tests/connector-plugin/session-audit.sh full` reproduces the build, workspace,
HTTP, full Navikt and broker gates within the user-approved 4 GiB limit.

Current upstream references for Miro's different formats:
[authorization-code exchange](https://developers.miro.com/reference/exchange-authorization-code-with-access-token)
and [refresh](https://developers.miro.com/reference/get-new-access-token-using-refresh-token).
Token-query requests require HTTPS outside loopback tests and return redacted
errors. Operators must also keep credential-bearing token URLs out of proxy/APM
logs. Only the unpublished Miro setup was corrected; tracked released files were
not rewritten.

The new sixteen providers still need real credential/consent and representative
vendor-response checks. All 50 are main read-only workflow implementations, not
exhaustive vendor APIs. Shared native OAuth via grengin-service is not implemented
by this change; plugins currently use instance-owned OAuth applications. Wix's
installation-specific OAuth adapter remains deferred. No CRUD support is implied.

The expanded compatibility candidate remains outside master-data, with null source
pins: grengin-list/docs/drafts/grengin-api/0.9.8/connector-console-index.json. The
active original index fixture and every tracked released package/index remain
unchanged. Commit pins, user audit and separate publication approval are still
required. Do not deploy that draft as a completed release catalog.
