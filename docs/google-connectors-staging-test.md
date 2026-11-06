# Google Connector Staging Test

Verified on 2026-11-04. This is a pre-consent readiness report, not proof of
Google token exchange or MCP tool access on staging.

## Test Environment

- Webapp: http://localhost:5173/settings?tab=integrations
- Backend: https://test-devel.grengin.com (0.9.7, migration head matches)
- OAuth broker: https://service-staging.grengin.com
- Local Google broker mode is disabled. The unused local Google test service
  was stopped; its isolated database and encrypted broker state were retained.

The operator applied the merged CONNECTOR_OAUTH_CLIENTS registry through
Wrangler. The prepared registry retained the saved Slack configuration and
added Google using the existing client credentials. CONNECTOR_TOKEN_KEYS and
the original secret backup were not modified. No source deployment or remote
database migration was performed by this work.

Google's registered callbacks are:

```text
https://service-staging.grengin.com/v1/connectors/callback/gmail
https://service-staging.grengin.com/v1/connectors/callback/google-calendar
https://service-staging.grengin.com/v1/connectors/callback/google-drive
```

## Results

- All three live Google broker start checks passed: configured client ID,
  exact staging callback, offline consent, PKCE, idempotency, replay refusal,
  signer ownership, pending-token refusal, and cleanup.
- All 12 existing live Slack broker checks passed after the registry update.
- The local webapp proxy returns the real VM's health and auth provider list.
- All 19 existing broker/callback unit tests passed.
- Isolated browser tests passed for Google admin setup, UUID binding, staging
  consent URLs, owner-bound popup completion, tool-sync controls, preserved
  Slack visibility, reload without duplicate writes, and mobile containment.
  These browser tests intercept API and vendor requests; they do not prove
  live backend writes or consent.
- Frontend tests, Vite production build, Node TypeScript check and whitespace
  checks passed. Full Svelte typecheck still has the previously observed
  ProjectDetailPage.svelte generic $props error and unrelated warnings.

## Manual Test

1. Reload the local webapp and sign in to the VM account. The VM currently
   advertises Azure enabled and Google SSO disabled; this work did not change
   either setting. Google connectors are independent of the sign-in method.
2. For each Google connector, use Set up if no server is configured. Choose
   access deliberately; the default is administrator-only. Existing matching
   server configurations are reused, not overwritten.
3. Click Connect and approve Google consent. The broker returns the popup to
   /mcp/oauth/callback on the local webapp, which verifies browser context and
   completes the connection. Confirm Connected, then check tool synchronization.
4. Test an actual tool call. Google API enablement, preview eligibility, and
   the real vendor token/tool response remain manual gates for staging.

## Repeatable Checks

```bash
bash tests/connectors/google-staging-prepare.sh
bash tests/connectors/google-staging-smoke.sh
bash tests/connectors/google-staging-ui-check.sh
```

Preparation only writes a private local registry file; it never uploads secrets.
The live smoke test creates and disconnects temporary pre-consent broker rows.
No Google consent or model call is performed by these scripts. No code was
committed or pushed.

## Setup Button Follow-up

The VM's read-only MCP lookup showed Slack configured but no Google servers.
The demo also checked mcp_servers:manage instead of the backend's actual
mcp_servers:admin permission, hiding setup from authorized administrators.
The frontend permission constant is now corrected. Missing server bindings
show Not configured, and Connect remains disabled until explicit admin setup.
No VM rows were created by this fix.

The earlier UI fixture incorrectly granted the nonexistent permission and
masked this mismatch. It now uses the real backend key: the test failed before
the fix and passed after it. Additional browser checks cover ordinary users,
the obsolete permission key, and scoped grants; none can configure a server
or bypass missing setup. Existing setup, callback, reload and mobile checks pass.

A second frontend deadlock was found in the saved-connection states: setup was
disabled for every existing connection, even when it had no chat server, while
connect required that missing server. Setup no longer depends on account
connection existence. Explicit reconnection retires an old connection before
changing its server binding; missing/deleted bindings are not selected as valid
servers. List/status requests are bounded to 20 seconds, and status failures
preserve the loaded account and server choices. Tests cover failed, expired,
revoked, pending and connected unbound accounts, plus a stalled status request.
The local dev server was reloaded to discard pre-fix component state.
