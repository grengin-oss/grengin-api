# Connector OAuth Review With Navikt

Historical review. Its eight probes are now fixed and passing in the
[all-34 verification report](connector-plugins-all-34-report.md), dated 2026-11-05.
The original findings and evidence below are retained as an audit trail.

Date: 2026-11-04, Asia/Kolkata.
Branch: `feat/connector-server-plugin`. Scope: single backend instance only.
Verdict: **not merge-ready**. Eight reproducible failing probes, grouped into
five findings below. This pass changes tests/documentation, not production code.
No commit, push, deployment, R2 upload or migration of a shared database occurred.

## Findings

### 1. Medium: Direct OAuth Ignores Current Server Access

Probes: `OAUTH-02`, `OAUTH-03`, `OAUTH-04`.

Locations: `src/services/mcp_helpers.rs:678`, `:693`, `:800`, `:826`.
`start_oauth_authorization` checks enabled state but not the caller's effective
server permission. `complete_oauth_authorization` reads the server but checks
neither enabled state nor current caller permission before exchanging/storing.

Observed through real HTTP routes:

- Set `default_access: admin_only`; a plain User can still start direct OAuth
  with 200 and receive a new authorization state.
- Start consent, disable the server, redeem the genuine Navikt code: callback
  returns 200 and persists a connected session.
- Start consent, revoke the user's server access, redeem: callback likewise
  returns 200 and persists a connected session.

The separate plugin tool-execution guard still blocks both disabled/revoked
cases. No unauthorized vendor tool request occurred in either probe. This is
an OAuth admission/connection policy bypass, not demonstrated cross-user data theft.

Recommended fix: use one current enabled/transport/effective-access check at
start and callback, then check again before storing a response received after
an asynchronous exchange. Reject stale OAuth bindings as well. Use existing MCP
access resolution and existing not-found errors; no new permissions or tables.

### 2. Medium: Duplicate Callbacks Destroy A Valid Connection Attempt

Probe: `OAUTH-05`. Locations: `src/services/mcp_helpers.rs:810`, `:818`, `:874`, `:906`.

Deterministic single-instance reproduction:

1. First callback redeems a valid code at Navikt; hold its successful response
   in the test relay before delivering it to the backend.
2. Send the identical callback again. Navikt rejects the spent code.
3. The second callback deletes the shared state through `finish_oauth_state`.
4. Deliver the first response. Its transaction cannot delete the already-missing
   state, so it refuses to save the valid tokens.

Observed: original callback 404, duplicate 503, no connected session and no state
left for retry. Both requests reached the same backend process; this does not
depend on distributed deployment or clock synchronization.

Recommended fix: serialize the full validation/exchange/cleanup/redemption flow
per state or per user/server using the existing weakly held async lock pattern.
Re-read the state after acquiring the lock. The duplicate must see consumed state
without contacting the token endpoint or deleting another callback's state.
Do not hold a database transaction or pooled connection over the network call.
Redis is not required for the supported single-instance deployment.

### 3. Medium: Malformed Token Responses Are Saved As Connected

Probes: `OAUTH-06`, `OAUTH-07`. Location: `src/services/mcp_client.rs:261`.

The test relay obtains genuine token responses from Navikt and changes only the
field being tested; this part is fault injection, not vendor-clone behavior.

- `access_token: ""` returns callback 200, stores an encrypted empty string and
  sets `connected: true`. The connection API claims success although the plugin
  has no usable credential.
- `expires_in: -1` also returns 200. Casting the negative number to `u64`, then
  dropping the conversion error, yields `expires_at: None`. The backend treats
  the credential as having no known expiry and will not refresh it on schedule.
  This does not extend the actual lifetime enforced by a vendor.

Recommended fix: one validated token conversion for code and refresh responses.
Reject blank/control-containing access credentials, invalid optional refresh
credentials and explicitly malformed/out-of-range expiry. Use checked timestamp
arithmetic. Distinguish omitted expiry from malformed expiry; retain support for
valid integer/string expiry formats already accepted. Zero must mean immediately
expired, not immortal. Add whitespace, missing-field, negative, zero, overflow
and refresh-response regression cases when implementing the fix.

### 4. Medium: Provider Error Content Reaches Backend Logs

Probe: `OAUTH-08`. Locations: `src/services/mcp_client.rs:290`, `:432`,
`src/services/mcp_helpers.rs:821`.

The relay returned a 400 OAuth error with a synthetic sensitive marker in
`error_description`. The marker appeared verbatim in the backend log. The API
itself returned a generic 503; the leak is in operator logging. No real secret
was used. Raw non-JSON error snippets can follow the same logging path.

Recommended fix: log provider/server context, HTTP status and a bounded safe
error classification, not provider descriptions or raw bodies. Bound token
response reads before parsing as well: exchange and refresh currently use
unbounded `response.text()`. The latter is a code-inspection observation, not
a memory-exhaustion test performed in this run.

### 5. Low: Disabled Server Still Appears In Tool Listing

Probe: `OAUTH-01`. Location: `src/services/mcp_tools.rs:312`.

`GET /mcp/tools?server_id=<disabled-plugin>` returns its enabled tool with
`total: 1`. Permission filtering resolves server access without first excluding
disabled servers. The execution path does reject it, so this is stale visibility
and inconsistent policy presentation, not a successful disabled-tool execution.

Recommended fix: filter enabled servers before resolving tool access, preserving
the existing read-only/full policy rules. Test disabled servers for both ordinary
users and administrators.

## Attribution

These eight probes exercise inherited MCP/OAuth code, not the new distribution
parser or the catalog move. `mcp_helpers.rs` and `mcp_tools.rs` are unchanged from
the committed base. The token-parser/error functions in `mcp_client.rs` are also
unchanged; this branch's changes there begin later with SDK/plugin integration.
The new plugin runtime reuses those paths, so the findings still matter before
merging/releasing this feature. They are not production fixes made in this pass.

## Executed Tests

Pinned mock: `ghcr.io/navikt/mock-oauth2-server:6.0.2`, the same version used in
`tests/auth/mock_oauth2.sh` and `tests/auth/mock_social_oauth.sh`.
Pulled image digest:
`sha256:b538810afd589d42fbfb856c588c2065eaeed1dc528d6c532972048e67fc2aff`.

| Gate | Result |
| --- | --- |
| Navikt-backed connector OAuth suite | 105 passed, 8 failing probes; exits 1 deliberately |
| Real mock token-endpoint requests | 30 |
| Real backend chat-to-local-tool requests | 18 |
| Existing plugin HTTP suite, rerun after harness change | 40 passed |
| Node and Bash syntax / diff whitespace | Passed |

The matrix explicitly installs clones of all eight version-1.0.0 catalog
manifests: Gmail, Google Calendar, Google Drive, Teams, OneDrive, Outlook Calendar,
Outlook Mail and SharePoint. For each, only the test copy's base URL is changed
to a loopback REST fixture and one existing no-argument read tool is selected.
Catalog files are not modified. Two users authorize separately and invoke the
tool through the actual `/chat/stream` loop with a fixture model.

Verified controls include PKCE S256 and verifier binding, exact callback URI,
user-bound state, anonymous/cross-user denial, encrypted credentials, connected
status APIs, code replay refusal, all three token-endpoint authentication modes,
per-user tool tokens, expiry, rejected PKCE, outage retry, real refresh grants,
omitted-refresh-token retention, disconnect during refresh, OAuth-binding change
invalidation, atomic persistence failure, and rejection of off-origin redirects.
An additional OIDC-shaped connector config exchanges a signed mock response; this
is not an assertion that connector code performs SSO ID-token/JWKS verification.

The first setup run exposed Navikt's requirement for an `openid` scope. Tests
now add `openid` and `offline_access` to in-memory fixture configuration only.
These are **not** Google vendor setup instructions. A subsequent harness check
incorrectly compared randomized ciphertext; it was corrected to compare the
decrypted fixture value. Those incomplete runs are not counted as passing.
The final run completed every case without harness errors.

No live vendor credentials, paid models, normal development database, Keycloak
realm or existing listening port was used or modified. The Navikt and PostgreSQL
containers each have a 384 MiB limit, while Node/backend test processes run in a
separate 1 GiB no-swap user scope. Tests reuse the existing backend binary; no
Rust rebuild was needed. Containers, fixtures and test servers were cleaned up.
The pulled, pinned Docker image remains cached for repeat runs.

The 795-test Rust workspace baseline was run in the preceding implementation
pass, not rerun here because this review changes no Rust code. Existing hosted
broker tests previously passed 54 cases using their fixtures; this new Navikt
matrix targets direct/custom-plugin OAuth, not a new real Slack/Google broker
consent run or a Cloudflare deployment.

## Repeat

```bash
systemd-run --user --scope --quiet -p MemoryMax=1G -p MemorySwapMax=0 \
  -p CPUWeight=20 bash tests/connector-plugin/oauth-navikt.sh
bash tests/connector-plugin/http.sh
```

`GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR` can override the sibling catalog path.
`GRENGIN_BACKEND_BINARY` can select a freshly built backend. The existing backend
binary must match the source being reviewed. `run.sh --oauth-navikt` also exposes
this suite after the usual build/test gates. Never treat its nonzero result as
an ignorable flake; it becomes a green gate only after these findings are fixed.

## Mock Boundaries And Sources

Navikt exercises authorization-code/refresh protocol behavior with independent
issuers, not Google/Microsoft account policies, real consent screens, restricted
scope verification or licensing. See its
[pinned documentation](https://github.com/navikt/mock-oauth2-server/blob/6.0.2/README.md).
The callback/PKCE/state checks follow the concerns in
[OAuth Security BCP](https://www.rfc-editor.org/rfc/rfc9700.html#section-2.1).
Real vendor consent remains a staging/manual requirement. Recovery was tested
for a relay failure before forwarding the exchange, not a response lost after
the provider already spent the code.
