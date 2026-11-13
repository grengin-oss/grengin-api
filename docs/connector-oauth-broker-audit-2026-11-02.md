# Connector OAuth Re-Audit: 2026-11-02

Follow-up: [the latest-docs and sync review](connector-oauth-latest-docs-and-sync-audit-2026-11-02.md)
confirms these fixes, but records unresolved resource/issuer contracts for
strict MCP authorization and actual main-merge conflicts. Use that review for
the latest synchronization and standards-alignment verdict.

## Verdict

The checked-in candidate had three additional reproducible Worker lifecycle
defects. They are patched locally, with six Worker regression tests and six new
compiled-backend HTTP checks. No further blocker was found in the reviewed
single-instance connector contract after the patches. This is not a claim of
bug-free code, complete vendor interoperability or production approval.

Controlled, isolated staging testing is supported after reviewing these patches,
applying the additive migrations and configuring the operator bindings. Do not
overwrite a newer shared service with this older branch without integrating and
retesting its main changes. Remediation and regression tests are recorded on the
feature branches. No deployment, remote migration, push or merge was performed.

## Tested Revisions And Scope

- Backend: `89d512701cd95fef1b68165f194b04d2cadd3a45`, plus the local HTTP-test and
  documentation changes. Backend production Rust code is unchanged.
- Service: `0680fc3bbb5b6e3af437c4bfcb4476a3a8cba116`, containing the runtime
  fixes and six regressions on top of `fe72fcea3c0018c6ed2ce54c95b425d66491d7f5`.
- Both branches: `feat/connector-oauth-broker`.
- Target: one persistent backend instance; no Redis/distributed support added.
- OAuth is brokered by the service; MCP traffic remains backend-to-vendor.
- No frontend files, real account settings or existing dev databases were changed.

## Reproduced Findings And Patches

### Medium: Newly Rotated Credentials Lost After A Failed Save

The vendor could issue a fresh rotating refresh token, then the required D1
audit/save transaction could fail. The database retained the spent credentials,
the connection stayed connected and the new credentials were lost. A later
disconnect could not revoke the actual new token.

Reproduction: reject `connector.refreshed` audit writes with a SQLite trigger.
The pre-patch request returned 503 while the stored state remained connected.
Both the available-provider and unavailable-provider tests failed before patching.

Patch: track issued credentials until their save succeeds. On failure, retain
them encrypted under the operation lease in failed/revoked state, with an atomic
audit event. Attempt provider revocation; an outage preserves disabled encrypted
credentials for cleanup retry. A concurrent disconnect remains revoked. If even
the recovery write fails, attempt direct vendor revocation without logging tokens.

Files: `grengin-website/workers/src/services/connector-broker.ts`, refresh and
`recoverRefreshCredentials`; regression tests in the adjacent Worker test file.

The compiled backend test forces the real Worker audit failure, verifies MCP
use fails, reconciles failed/revocation-pending state through GET /connectors,
and confirms disconnect later revokes the rotated rather than spent credential.
Total durable-storage failure combined with provider revocation failure remains
an explicit recovery limitation, not a solved guarantee.

### Medium: Incorrect Lifetime For Non-Refreshable Tokens

The early-refresh window rejected an otherwise valid token during its last
30 seconds even when the vendor supplied no refresh token. After actual expiry,
status still reported connected. Both conditions failed the new pre-patch tests.

Patch: use early refresh only if a refresh credential exists. Non-refreshable
tokens remain usable until their actual expiry. On a status/token request after
expiry, atomically mark the connection expired and clean up its credentials.
Recheck expiry before delivery. Expired refreshable tokens remain eligible for
the existing refresh path rather than being incorrectly discarded.

Files: service `expireConnection` and token checkout. Actual HTTP tests verify
that the backend can initialize MCP using a 20-second non-refreshable token and
then reconciles its expired state without losing a pending cleanup handle.

### Medium: Status Contradicted An Active Callback Lease

An exchange that began just before the consent deadline could still be active
after that deadline. Expiration correctly respected its lease, but the response
serializer independently labeled it expired. The backend could consequently
erase its connection grant while the callback was still issuing credentials.

Reproduction: hold the mock token response, advance the clock beyond the initial
deadline while preserving the live lease, then read status. It returned expired
before the fix. This was the fifth failing pre-patch reproduction assertion.

Patch: report the database lifecycle state after the lease-aware transition,
not a second time-based interpretation in serialization. An active exchange
remains publicly pending. A concurrent-start winner also takes the same expiry
path. Actual backend HTTP checks prove the encrypted grant survives polling and
the owner can complete consent afterwards.

## Results

| Check | Result |
| --- | --- |
| Pre-patch new reproductions | Five assertions failed, covering the three defects |
| Rust workspace | 758 passed, 0 failed, 61 environment-dependent tests ignored |
| Rust workspace build | Passed, maximum two Cargo jobs, 3 GiB scope |
| Worker full suite after patches | 414 passed, 0 failed; includes 69 broker tests |
| Compiled backend + actual Worker + local TLS vendor | 53 named HTTP checks passed |
| Model-to-MCP-to-model continuation | Passed with local fixture model and real MCP transport |
| Connector migration up/down/up | Passed in disposable PostgreSQL 16/pgvector |
| Worker migration chain | Passed using isolated local Wrangler storage |
| Worker TypeScript | Passed |
| Saved Slack settings and published template | Passed; no real vendor token request |
| Bash/JS syntax, Cargo formatting and both Git diffs | Passed |

The existing proof/body/path tampering, replay, timestamp, owner/grant isolation,
receipt replay, installation revocation, encrypted storage, client identity,
operator allowlists, endpoint binding, redirects, 429/backoff, outage recovery,
audit atomicity, deleted-owner cleanup and legacy-token suppression checks pass.
The catalog-shape suite covers 33 templates with mocked vendor responses; it
does not represent 33 real vendor integrations.

Clippy was not rerun: production Rust code is unchanged. The workspace still
reports the pre-existing proc-macro-error2 future-incompatibility warning. No
claim of a globally warning-free repository is made.

## Reproduce

```bash
cd /home/anurag/work/grengin-api
bash tests/connectors/audit-core.sh
```

The script privately sources `/home/anurag/work/secrets/grengin.sh` and the
backend `.grengin_auth_token`, then replaces DATABASE_URL with a disposable
loopback test database. Paid model calls are not made. Temporary test servers,
containers and scratch credentials are cleaned up; the previously running local
Slack/frontend services are left unchanged.

Evidence from the final cross-repo run, including all 414 Worker tests and 53
HTTP checks: `/tmp/grengin-connector-core-audit.4DBu4v/`. The independent Worker
rerun is `/tmp/grengin-connector-audit-2026-11-02-worker-final.log`.

## Deployment Gates And Remaining Limits

- Service origin/main is `f357f45`; the tested branch lacks 55 main commits.
  Backend origin/main is `b365aca` with one release-only commit not in the branch.
  Integrate through feature branches and rerun tests before replacing shared
  staging. This audit does not validate an untested merge result.
- Include Worker commit 0680fc3. Deploying fe72fce alone omits the new fixes.
- The public 0.9.7 catalog already pins backend 89d5127. No catalog change or new
  version is required by these Worker fixes. Do not rewrite a published index.
- Configure CONNECTOR_BROKER_ENABLED, token encryption keys, the OAuth client
  registry, service origin and the matching registered installation identity.
  Apply migration 0018 to the D1 binding actually used by staging, including EU
  storage where selected. Follow `connector-oauth-staging.md`.
- Register the real staging callback at the vendor. Recheck real Slack consent,
  owner confirmation, tool use and disconnect in staging. The earlier local
  Slack consent/tool-list result from 2026-10-30 is historical; no fresh vendor
  consent or paid model-led Slack tool execution was performed in this audit.
- The deployed frontend must implement explicit owner confirmation. The earlier
  local callback override is not a production implementation or permission to
  forward backend tokens to the service/vendor.
- Verify scheduled Worker maintenance after deployment. Backend deleted-owner
  cleanup needs a persistent process; frozen Lambda requires external execution.
- Unsupported revocation means local/broker credential removal, not withdrawal
  of vendor consent. Simultaneous durable-storage/vendor failure still needs
  operator recovery. Audit/tombstone retention needs a capacity policy before
  broader production use; this patch introduces no unapproved destructive purge.
- Service request quotas are per local user, whose identity a registered
  installation asserts. They are not an installation-wide abuse budget. Define
  aggregate installation limits and retention/capacity policy before exposing
  the shared broker to untrusted customer installations at production scale.
