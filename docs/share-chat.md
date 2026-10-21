# Share Chat Backend

Implements the backend requirements on pages 110-111 of `analysis.pdf`.
This is a single-instance feature and depends on the security hardening branch.

## Contract

- `POST /chat/{chat_id}/shares`: owner plus `chats:share`; creates a frozen snapshot.
- `GET /chat/{chat_id}/shares`: owner lists links, including revoked links.
- `PUT /chat/{chat_id}/shares/{share_id}`: owner plus permission; changes access/expiry,
  optionally replaces the snapshot with `refresh_snapshot: true`.
- `DELETE /chat/{chat_id}/shares/{share_id}`: owner revokes, idempotently, even if their
  sharing permission was removed.
- `GET /share/{token}`: reads the snapshot, not the source conversation.
- `GET /share/{token}/files/{file_id}`: only an attachment/artifact in that snapshot.
- `POST /share/{token}/fork`: active signed-in viewer creates a private independent chat.
- `GET /me/shared-chats/{share_id}/link`: eligible signed-in recipient resolves a notification
  into a link, with the same live scope/expiry/revocation checks as viewing the share.

Access is `organization` (default: an active account in this deployment) or `department`
with a nonempty `department_ids` array (at most 100 IDs). There is no public, per-user
or role-targeted access. Department IDs are validated and deduplicated transactionally.
Department membership means the viewer's current primary department, matched exactly:
child departments and scoped role assignments do not implicitly grant access. The owner
can preview their own link even when outside the selected departments. Other users get 404
when not in an allowed department; this applies equally to views, downloads and forks.
Department deletion removes its grants, never broadens access to the organization.
Expiry is optional; expired, revoked, deleted-source and inactive-owner links return 404.
Viewing, downloading and continuing always require a valid access token and active user.

## Privacy and Persistence

Snapshots include only visible user/assistant messages from the selected branch, approved
attachment metadata and artifact content. System prompts, tool arguments/results, hidden
branches, provider request IDs, private project/skill links and internal metadata are excluded.
Snapshot text is immutable until explicitly refreshed. Original attachments remain subject to
their owner's deletion: an unavailable file is omitted from views, captures and copies, while
its direct download returns 404. Other text and available attachments remain usable.
Inline chat attachments are captured with the snapshot, not resolved through a client-sent path.

Links use 256-bit random bearer tokens. Only a hash is indexed for lookup; the recoverable token
is encrypted with APP_KEY for the owner's link list. Tokens and message contents must not enter
audit logs. Share responses disable caching/indexing and use a no-referrer policy. File responses
reuse the safe download policy (nosniff, byte-sniffed inline media, attachment otherwise).

Forking copies messages and files under new UUIDs for the viewer, creates a fresh parent chain,
and resets usage/cost. It copies no project, skill, MCP, summary or embedding associations.
If RAG is enabled, indexing is rebuilt in the background from the committed private copy with a 30-second budget;
it first checks the viewer's active status, department budget and allowed summary model.
Denied or failed indexing does not undo the copy or import any source RAG state. Database writes are transactional;
failed forks remove newly written files. Existing/source files are never removed by rollback.
Files are read and staged before opening a database transaction. Access and snapshot identity
are rechecked under the share lock before committing. At most two copies run per process;
copy saturation returns 429 with `Retry-After: 1` and `error: "share_fork_busy"`. Copy slots
are released at commit. Optional indexing has two independent slots; if those are busy,
indexing is skipped without blocking or failing the committed copy. No unbounded queue is
created. Completion, failure, timeout and cancellation release index slots. A
snapshot changed during staging returns 409. An ambiguous commit failure retains new bytes
rather than risking deletion of a committed file.
Reverse proxies and analytics must also redact share-token URLs from their own request logs.

A process crash between staging bytes and inserting their metadata can leave orphaned files.
Normal failure paths remove them, but crash recovery/age-based orphan cleanup is not implemented.
Do not indiscriminately delete staged-looking paths: a copy may still be using them.

## Bounds and Testing

Snapshots have message, attachment, artifact and serialized-size limits.
Each chat can hold at most 100 share records (including revoked links). Limits apply only
after access-token and active-user validation, per signed-in user, never per IP address.
Each 60-second window allows 120 views/link lookups/lists, 600 file downloads, 10 forks,
60 creates/updates and 60 revocations, in independent buckets with Retry-After. Authentication
failures never allocate a bucket. The bounded cache evicts its oldest authenticated bucket
when full instead of blocking every new viewer. There is no Redis or distributed guarantee.

Tests must cover owner/permission checks, organization/department authorization, typed-token rejection,
expiry/revocation, snapshot immutability and branches, file membership/ownership, audit token
redaction, fork isolation/rollback and rate limiting. Use isolated PostgreSQL and local files;
never migrate the developer database to test this feature. Rust commands use at most two jobs.

Frontend rendering of `/share/{token}` is a separate integration; these are backend APIs and
OpenAPI contracts, not a new webapp page.

## Frontend Integration

Create a link with `POST /chat/{chat_id}/shares` and
`{ "access": "department", "department_ids": ["<department UUID>"] }`, or use
`{}` for organization-wide access. Switch scopes through the same fields on the update API;
switching to organization clears department grants, while department access needs targets.
The response contains `id` and `share_url`; do not log the URL or send it to analytics.

The shared page reads `GET /share/{token}` with an access token. The response has `chat.messages`, with
`text`, `files[].download_url`, and `artifacts[].content`. Treat all text, Markdown and
artifact content as untrusted; sanitize it and never inject artifact HTML into the page.
A link returns 401 when sign-in is needed; an unavailable or out-of-department link returns 404.

After sign-in, `POST /share/{token}/fork` returns `{ "conversation_id": "..." }`.
Navigate to that private conversation. Never continue a shared conversation using its
source IDs. Link management uses the owner's original chat ID and the share record ID.

## Sharing Notifications

Department shares default to one durable `chat.shared` inbox notification per eligible active
user, excluding the owner; use `notify: false` to keep a particular mutation silent.
Organization shares are silent by default. Explicit `notify: true` with organization access
requires an org-wide Super Admin and targets every active non-owner; ordinary users get 403
if they request that broadcast. Department scope targets current, exact primary-department
members only. Inactive/pending/suspended/
deleted accounts and role-only or child-department matches are not recipients.
Multiple selected departments still notify all their eligible members, subject to the
existing 100-target request bound. There is no new notification-specific department cap or
Super Admin requirement. These are in-app inbox/SSE notices, not emails; the organization
broadcast restriction is not a claim that an equivalent recipient set cannot span departments.

Notifications are saved in the same transaction as the share and required audit event;
if notification persistence fails, the entire share mutation rolls back. Successful commits
broadcast newly inserted notifications through the existing `/me/notifications/stream` SSE
channel. `/me/notifications` remains the durable source of truth if a client disconnects or
the process stops between commit and broadcast. No Redis, email service or new delivery
credentials are needed.

A unique `(shareId, userId)` key prevents duplicate notifications on refreshes/retries of the
same share update and preserves read state. A separately created link is a new share.
For a mutation with notifications enabled, share updates add newly eligible recipients.
Existing rows are retained for deduplication, even while the recipient is ineligible;
the inbox and SSE recheck live scope/expiry/owner status before showing them. Scope toggles
cannot reset read state or repeatedly broadcast an existing share. Revocation deletes share notices;
source deletion cascades them. New accounts and membership changes do not trigger a background
fan-out; they are considered on the next explicit share update with notifications enabled.
Previously delivered events cannot be recalled, so
opening a notification always revalidates access.

Titles/bodies are generic: no source chat title, text, file names, owner email or bearer link.
The payload is `{ "share_id": "<UUID>" }`. For `kind: "chat.shared"`, the frontend calls
`GET /me/shared-chats/{share_id}/link` with its access token, then navigates to `share_url`.
The server does not treat possession of a notification as an authorization grant.

## Reproduce Tests

```bash
bash tests/share-chat/run.sh --live
```

The script runs formatting, workspace tests and a workspace build with two Cargo jobs,
then runs HTTP/security regressions against a disposable PostgreSQL copy. It tests migration
down/up, uses a local model fixture (no paid LLM calls), and removes its server/database/files.
`--live-only` reuses an already-built debug binary; use `--live` after changing Rust code.
The developer's database is only read by `pg_dump`; migrations target the disposable copy.

## Audit Follow-up Verification (2026-10-21)

- Workspace: 731 passed, 0 failed; 60 opt-in tests ignored by default.
- Live HTTP: 375 sharing checks, 93 combined security/chat checks and 23 existing
  security checks passed. Eight OAuth protocol mocks and eight database regressions passed.
- Two deliberately blocked file reads confirm copy saturation returns retryable 429,
  holds no share-row lock, rechecks revocation and restores copy capacity after failure.
- Blocked indexing futures confirm copy capacity is independent from indexing.
  Index success, failure, timeout and cancellation release their own permits.
- Deleted-file handling, retained read state through scope toggles, ordinary department
  notifications, explicit Super Admin organization broadcasts, per-user/operation quotas
  and all eight OpenAPI operations are covered live.
- Build, formatting and diff checks pass. Clippy retains existing repository warnings.
  Live HTTP uses local fixtures with RAG disabled; real post-fork provider/index calls
  are not claimed as tested. See [staging-validation.md](staging-validation.md).
- Validation performed no push, deployment or developer database migration. The combined work is on `feat/share-chat`.

## Verification Before Audit Follow-up (2026-10-21)

- Workspace: 725 passed, 0 failed, 60 external-service/database cases ignored by default.
- HTTP: 189 Share Chat checks and 23 existing security checks passed; 8 explicit database
  regressions also passed. Build, formatting and migration down/up passed.
- Combined staging regression: 93 additional HTTP checks and 8 credential-free OAuth
  protocol checks passed. Run `bash tests/staging/run.sh`; see
  [staging-validation.md](staging-validation.md) for evidence and coverage limits.
- Removed public, user and role scope values are rejected. Anonymous reads/downloads/forks
  require sign-in. Department changes, exact membership (not child departments or roles),
  multiple targets, failed scope-change rollback and department deletion are covered live.
- Notifications: inbox and live SSE delivery, recipient filtering, read state, duplicate
  suppression, scope expansion/restriction, secure link resolution and failed-write rollback
  passed. Fan-out exceeding 500 recipients passed; migration rollback preserved budget
  notifications and removed sharing notifications without orphaned rows.
- This validation ran before committing/pushing `feat/share-chat`. Tests used disposable
  data and files; the developer database was not migrated.

## Previous Verification (2026-10-20, Before Scope Restriction)

- Workspace: 723 passed, 0 failed, 60 external-service/database cases ignored by default.
- HTTP: 104 Share Chat checks and 23 existing security checks passed.
- Explicit database regressions: 8 passed (MCP refresh, upload concurrency, SQLx MCP).
- Migration down/up, concurrent share limits, required-audit rollback and copied-byte cleanup passed.
- Workspace build, formatting and diff checks passed. Clippy passed with existing repository
  warnings and none in the new Share Chat modules. Cargo reports the existing
  `proc-macro-error2` future-compatibility notice.
- Live checks use a local model fixture with RAG disabled. The optional provider-backed
  RAG rebuild is not exercised by this HTTP suite; no paid model calls are made.
- No developer database migrations, production writes, deployment, commit or push performed.
