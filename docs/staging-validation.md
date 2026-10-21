# Combined Security and Share Chat Validation

Date: 2026-10-21. Target: one backend instance, PostgreSQL, local file storage.

## Verdict

The backend is ready for a staging deployment from the combined `feat/share-chat`
branch after review and commit. The audit's shared copy/index capacity bug is fixed
and covered by regression tests. No further blocking issue was reproduced in this run.
This is not a guarantee that every production failure mode is covered.

The branch already contains `origin/fix/security-issues` at `5fd228b` and the fetched
`origin/main` at `804f09a`. Neither needed merging again. Validation ran before the
Share Chat commit; nothing was pushed or deployed as part of the validation run.

## Fresh Results

| Gate | Result |
| --- | --- |
| Locked workspace tests, two Cargo jobs | 731 passed, 0 failed, 60 ignored by default |
| Locked workspace build | Passed |
| Formatting and whitespace diff | Passed |
| Clippy, workspace/all targets | Passed with existing warnings, not warning-free |
| navikt OAuth browser/mobile/provider protocol tests | 8 passed |
| Existing security HTTP regressions | 23 passed |
| Additional combined HTTP regressions | 93 passed |
| Sharing and notification HTTP checks | 375 passed |
| Explicit PostgreSQL regressions | 8 passed |

The 491 HTTP checks include response-status assertions and persisted-state assertions;
they are not 491 separate product workflows. The 16 opt-in OAuth/database tests are
among the 60 normally ignored tests, not additional workspace unit tests.

## Coverage

- Access-token type, deactivated accounts, private-chat ownership and attachment
  ownership; refresh tokens cannot act as access tokens.
- Independent file metadata with deduplicated bytes, safe HTML downloads, rejected
  uploads/continuations with no partial writes, and artifact deletion rollback.
- Role create/update/delete, grant/revoke and department create/update fail atomically
  when required audit or permission writes fail. HR Admin cannot grant or deactivate
  a Super Admin; scoped Super Admin assignments are refused.
- Concurrent department admin replacements keep only the later admin set. An admin
  update waiting for a database lock preserves a newly committed budget. Lock
  contention returns 503 after approximately five seconds without a partial write.
- Team project detail returns only the caller's linked chats.
- Failed provider calls and failed reply persistence leave edit history intact.
  Successful edits expose versions; switching restores the original thread; another
  user cannot edit, switch, or cancel it. Stop persists `user_cancelled`, including
  when sent after the HTTP stream disconnects.
- A pre-edit share remains frozen through edits and branch switches.
- MCP OAuth rejects wrong users, expired states and replays; stores tokens encrypted;
  preserves state only for unreachable endpoints; retries after recovery; and rolls
  state deletion back together with a failed token write.
- Sharing supports only organization and exact primary departments, requires active
  sign-in, and enforces ownership, expiry, revocation, response privacy and rate limits.
- Forks get independent identities and files, no private source associations or RAG
  state, reset usage, and remain usable after source revocation. Forced persistence
  failures roll back rows and copied bytes.
- Deleted/missing attachments are omitted from captures, views and copies without
  losing text; foreign files remain inaccessible. Two FIFO-controlled disk reads
  exhaust copy slots without holding share-row locks or blocking health checks.
  A third valid copy returns 429 with Retry-After; invalid links still return 404.
  Revocation during staging prevents both commits, removes staged bytes and restores
  copy capacity. Slot recovery is verified by a subsequent successful HTTP copy.
- Unit tests use blocked indexing futures to prove both copy slots are released
  while both independent index slots are occupied. Excess optional indexing is not
  queued/polled. Completion, error, timeout and task cancellation restore index capacity.
- Notifications exercise durable inbox/SSE delivery, eligible recipients, no content
  or tokens in payloads, read-state preservation, deduplication, scope changes,
  transaction rollback and no uncommitted broadcasts. Fan-out spans multiple batches.
  Organization broadcasts are explicit and Super Admin-only. Department notifications
  remain available to ordinary sharers; retained recipient rows are filtered by live
  eligibility rather than deleted/recreated when scopes toggle.
- Authenticated per-user rate limits separate reads, downloads, changes and revocation.
  Anonymous requests never allocate quotas, one user's read limit does not affect
  another user on the same address, and exhausting changes does not prevent revocation.
- All eight OpenAPI operations explain access and errors; request examples are checked.
- Share migration down/up preserves budget notifications and removes sharing
  notifications without orphan rows. Readiness rejects an outdated schema.
- Opt-in database tests cover MCP token-refresh races, concurrent uploads, read-only
  SQL execution, statement timeout, cancellation and session-lock cleanup.
- OAuth mocks cover authorization code, PKCE, nonce/JWKS validation, replay, Android
  and iOS redirect URIs, Auth0, Keycloak, LinkedIn and Apple form-post behavior.

## Reproduce

```bash
cd /home/anurag/work/grengin-api
bash tests/staging/run.sh
```

Requires Docker, PostgreSQL client tools, Node.js and Rust. Secrets are sourced from
`/home/anurag/work/secrets/grengin.sh` without printing them. Override that path with
`GRENGIN_SECURITY_SECRETS_FILE`. The source database is only read by `pg_dump`;
all writes, triggers and migrations target a disposable loopback PostgreSQL container.
Copied MCP servers are disabled before backend startup. Test servers, containers and
files are removed on exit. No real LLM credentials or paid generation are needed.

The runner saves logs under `/tmp/grengin-staging-validation-*`. This run's complete
gates are in `/tmp/grengin-staging-validation-9d31Ns`; the combined console log is
`/tmp/grengin-fork-slots-staging.log`.

## Boundaries and Deployment Notes

- Protocol-compatible OAuth mocks are not live Google/Entra/Apple endpoints or native
  SDK tests. Browser UI and email delivery were not tested; notifications use inbox/SSE.
- LLM responses and MCP token endpoints are controlled local fixtures. The HTTP suite
  disables RAG; optional post-fork embedding rebuilds and paid providers are not live
  validated. Index capacity/lifecycle is tested with controlled futures, not a real
  provider. There is no claim of distributed/Redis support.
- The legacy local API suite was not rerun or counted; its old results are not fresh
  evidence for this branch. Optional tests not explicitly enabled remain unverified.
- Clippy retains existing repository warnings and the existing dependency
  future-incompatibility notice. It is not a clean `-D warnings` gate.
- Commit all new feature files, especially
  `migration/src/m20261019_000001_create_chat_shares.rs`, with `migration/HEAD` and
  the migration registration. Back up staging before applying the migration. Its
  down migration intentionally deletes shares and their notifications, so running
  it on deployed data is not a lossless rollback.
- Notification recipients reconcile when a share is created or explicitly updated;
  membership changes do not trigger automatic fan-out. Link resolution rechecks live
  eligibility, and old generic notices never grant access by themselves.
- Optional indexing skips work when its two slots are occupied; a successfully
  copied chat remains usable without that rebuild. It cannot reserve copy capacity.
- A server crash between staging bytes and their metadata insertion can leave
  unreferenced files. Normal errors clean them up; crash/orphan garbage collection
  remains unimplemented and must not be mistaken for a guaranteed cleanup path.
- Frontend sharing/notification actions still need corresponding UI integration and
  a staging smoke test against the deployed backend. This verdict covers the API.
