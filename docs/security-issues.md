# Security and Correctness Issues

## Purpose

This file tracks the 30 issues found during a code walkthrough of grengin-api and how each was
resolved. The walkthrough came from reading the code, and only issue 1 had been checked by hand,
so every issue was verified against the current code before it was fixed. Several reported line
numbers had drifted; the locations below are the current ones.

Six issues (1 to 6) let one user reach another user's data or rights. Issues 1, 2, 5, 7 and 18
had to be fixed before Share Chat ships, because a public link makes chat and file ids easy to
find. All five are fixed.

Fixes are on the `fix/security-issues` branch.

## Status Legend

- **Fixed**: confirmed in the current code and fixed, with tests.
- **Fixed (partial report)**: part of the report no longer applied; the part that did, or a
  related hole found while verifying, is fixed.
- **Not reproducible**: the current code does not have the problem; regression tests were added
  where useful.

## Summary

| # | Severity | Area | Issue | Status |
|---|----------|------|-------|--------|
| 1 | Critical | Chat | Any signed-in user can post into another user's chat and get its history, summary and project docs back through the model | Fixed |
| 2 | Critical | Files | Attachments are read from disk by client-sent file name, with no DB lookup | Fixed (partial report) |
| 3 | Critical | Files, skills | Upload paths use the client file name unsanitised (path traversal on write) | Fixed (partial report) |
| 4 | Critical | RBAC | `roles:assign` can grant Super Admin; a department-scoped assigner creates org-wide grants | Fixed |
| 5 | Critical | Auth | Refresh tokens (7 days) pass as access tokens on every route | Fixed |
| 6 | Critical | Auth | Sign-in links an IdP login to an existing account by unverified email; with no email the domain allow-list is skipped | Fixed |
| 7 | High | Chat | Image generation loads any `files` row by id and sends it to the provider | Fixed (partial report) |
| 8 | High | Projects | Project sources accept any `file_id`, so another user's file is chunked into your project | Fixed |
| 9 | High | Projects | Team-project writes (sources, artifacts) check only read access | Fixed |
| 10 | High | Skills | Personal skills can be read and used by any user | Fixed |
| 11 | High | Analytics | A department admin who passes their own department id gets every department's analytics | Fixed |
| 12 | High | Departments | A scoped admin can pull any user into their department, reparent freely, and change budget mode | Fixed |
| 13 | High | RBAC | Deleting a department turns roles scoped to it into org-wide roles (FK `SET NULL`) | Fixed |
| 14 | High | AI engines | A deleted AI key stays live until restart (missing `.await`) | Not reproducible (related race fixed) |
| 15 | High | System | Binary update installs from any request-supplied URL, checksums optional, as root | Fixed |
| 16 | High | MCP | sqlx-mcp's read-only guard is a keyword scan with a bypass, plus a raw-query fallback | Fixed |
| 17 | High | MCP | MCP OAuth tokens stored in plaintext; callback has login CSRF; `redirect_uri` is an open redirect | Fixed |
| 18 | High | Files | Downloads are served inline with the client's Content-Type and no `nosniff` (stored XSS) | Fixed |
| 19 | Medium | MCP | `GET /mcp/tools` needs no login | Fixed |
| 20 | Medium | Auth | Deactivated or deleted users keep access for up to 7 days: no revocation or status check | Fixed |
| 21 | Medium | LLM | No HTTP timeouts on any provider or catalog call | Fixed (partial report) |
| 22 | Medium | Chat, budgets | The stream never checks the admin model whitelist; models missing from the catalog cost 0 and skip budgets | Fixed |
| 23 | Medium | Chat | About 15 `.expect()` calls in the stream panic on DB errors; every text chunk rewrites the message row | Fixed |
| 24 | Medium | Chat | Cancel stops working after the first tool round | Fixed |
| 25 | Medium | Search | Search snippet slices by bytes and can panic on non-ASCII text | Fixed |
| 26 | Medium | RAG | Changing the embedding dimension breaks every insert (column fixed at 1536) | Fixed |
| 27 | Medium | Departments | Reparenting a department through PUT leaves descendants on the old ltree path, with no cycle check | Fixed |
| 28 | Medium | Budgets | Departments with no budget are treated as exhausted and spam notifications | Fixed |
| 29 | Low | Image gen | Gemini image API key goes in the URL and can surface in `ai_error` | Not reproducible |
| 30 | Low | Audit | Audit IP trusts `X-Forwarded-For`; redaction hides token counts but stores tool calls and titles | Fixed |

## How the Fixes Were Verified

- **Cargo tests:** each access or validation decision was moved into a pure function and unit
  tested for the allowed case, the denied case and edge cases. `cargo test --workspace` passes:
  607 passed, 0 failed, 53 ignored (tests that need live services). `cargo build --workspace`
  reports no warnings, `cargo fmt --check` is clean, and `cargo clippy` reports no warnings that
  `main` doesn't already have. There is no test database or sea-orm mock in this repo, so the DB
  wiring itself is covered by the live checks below instead.
- **Live checks:** the branch build ran against a throwaway copy of a dev database, with real
  Keycloak logins, for a signed-in owner and a second plain user. Confirmed over HTTP: issue 1
  (404, nothing streamed or written to the other user's chat, and the owner can still continue
  their own chat), 2, 4, 5, 8, 18, 19 and 20. The new migration (issue 13) applied cleanly to
  the copy.
- **API suite:** `tests/api/run.sh` gave the same result on the branch and on `main` (246 of 259
  passed). The 13 failures are identical on both and come from the test environment: gemini is
  not configured, the Flint Chart Author skill is not seeded, and some checks are out of date.
- **First-login end-to-end scripts:** all checks pass on the branch, including the first-user
  and bootstrap-owner paths, after the OIDC changes for issues 5, 6 and 20.
- **Review follow-ups:** live checks confirmed the migration that demotes scoped Super Admins on
  seeded data, the scoped Super Admin grant refusal, prompt redaction in audit rows, the filtered
  tool listing, a rejected edit keeping its history, and five concurrent logins for one user.
  Edits, on the final design: when the provider fails after the edit began (invalid provider
  key), the original messages are untouched and the attempt exists only as hidden rows. When
  saving the reply fails (forced by a temporary trigger), the client gets one persistence error,
  no success event, and an unchanged history. A successful edit running alongside another turn in
  the same chat hides exactly the replaced messages and leaves the other turn's messages visible.
  With a seeded summary containing a codeword, a normal turn repeats the codeword, an edit before
  the summary's end does not and the summary is dropped at commit, and an edit after the summary's
  end still uses it and keeps it. After successful edits the stored counters match the visible
  messages, and a failed edit leaves them unchanged.
  The MCP OAuth callback was driven against a mock token endpoint: when the endpoint is
  unreachable the state is kept and the same callback then succeeds; a rejected code consumes the
  state; when storing the tokens fails nothing is committed, and the provider's rejection of the
  retried code then consumes the state.

## Details

### 1. Cross-user chat access through the stream (Critical, Chat)

- **Status:** Fixed.
- **Cause:** the stream loaded the conversation by id with no owner filter, and the edit
  endpoint let the body's `conversation_id` override the path it had checked.
- **Fix:** `services/conversation_access.rs` (`can_access_conversation`,
  `find_active_conversation_for_user`) applies the same owner rule as `GET /chat/{id}` to the
  stream, the edit endpoint and cancel, and returns the same not-found error. The edit endpoint
  (now in `services/message_helpers.rs`) always uses the path's chat id and refuses archived
  chats. It no longer deletes anything itself: it hands a `PendingEdit` to the stream. The
  original messages stay visible throughout. The stream records the exact ids of the messages
  being replaced and leaves them out of the model's context: recent history, retrieval and the
  plain history query all stop at the edited message, and the conversation summary is used only
  if it ends before the edited message. The new turn's messages are written hidden, with their
  ids recorded. Only when the turn completes, or the user cancels it, does one transaction
  (`EditInProgress::commit`) hide the replaced ids, reveal the new ids, drop a summary that
  covers the edited message (so it is rebuilt from visible messages), and recount the
  conversation's `message_count` (visible user messages) and `last_message_at`. Embedding and
  summary updates for the turn run after that commit. The commit requires the assistant reply, or
  the image message, to have been saved. If anything fails first (provider start or a mid-stream
  provider error, a failed write, failed image generation, a failed commit, or the client
  disconnecting), nothing was hidden or counted, so there is nothing to undo, and messages written
  to the conversation by anything else are never touched. Linked projects are filtered to projects the user can still
  read. Skills for the stream are loaded for the requesting user only.
- **Tests:** `conversation_access`, `message_helpers`.

### 2. Attachments read by client-sent file name (Critical, Files)

- **Status:** Fixed (partial report).
- **Cause:** the reported files no longer exist. Chat attachments were already resolved through
  the `files` table by owner (`file_storage::read_attachments_for_user`). Verifying found a live
  hole elsewhere: `add_project_source` stored any `file_id`, so another user's file was indexed
  into your project and readable through project chat, and could be deleted through
  `origin: "artifact"`.
- **Fix:** see issue 8. `file_storage::find_file_for_user` is now the single owner lookup.
- **Tests:** issue 8's tests.

### 3. Path traversal on upload (Critical, Files and skills)

- **Status:** Fixed (partial report).
- **Cause:** `..`, absolute paths and `/` were already rejected. `safe_file_name` still accepted
  NUL, backslashes, trailing separators, whitespace-only names and names over 255 bytes, and the
  write path had no containment check.
- **Fix:** `safe_file_name` rejects all of those, and `new_file_path` checks that the final path
  stays inside the storage root. Every write path uses them: uploads, skill knowledge, chat
  artifacts, generated images and project artifacts. Bad names get a 400.
- **Tests:** `file_storage`, `skills_helpers`.

### 4. Privilege escalation through role assignment (Critical, RBAC)

- **Status:** Fixed.
- **Cause:** an org-wide grant was checked against the target user's department
  (`target_scope = req.scope_department_id.or(target_user.department_id)`), and the role's
  content was never checked, so any `roles:assign` holder could grant Super Admin, even to
  themselves.
- **Fix:** `services/role_assignment.rs` and `services/permission_grants.rs`. `roles:assign`
  is checked at the exact assignment scope (no scope means org-wide). Only an org-wide Super
  Admin may grant or revoke Super Admin. The assignment scope and the target user's department
  must be inside the assigner's scope. A non-Super-Admin may only grant roles whose every
  permission they hold at that scope. Role create and update follow the same rule. This covers
  assign, revoke, department admin sync and the department creator's automatic grant. Denials
  are audited. Super Admin can only be granted org-wide (`ensure_role_grant_allowed`); revoking a
  legacy scoped row still works. Role-name checks (`user_has_role_name`, `user_roles_map`) count
  Super Admin only when org-wide. Migration
  `m20261007_000002_demote_scoped_super_admin_assignments` turns existing scoped Super Admin
  grants into Department Admin grants on the same department and clears the affected users'
  cached permissions; its down migration is a no-op, because restoring them would bring the
  escalation back.
- **Tests:** `role_assignment`, `permission_grants`, `department_helpers`, `authorization`,
  migration statement tests.

### 5. Refresh tokens accepted as access tokens (Critical, Auth)

- **Status:** Fixed.
- **Cause:** `Claims` had no type marker, so a refresh token decoded as access claims.
- **Fix:** every new token carries a `token_use` claim (`access` or `refresh`), checked on
  decode. For tokens issued before the deploy: an untyped token is a refresh token only with
  `refresh: true`, and an access token only with no `refresh` key and an expiry within the
  access lifetime (1 hour plus 60 seconds). Old access tokens keep working until they expire;
  old refresh tokens still refresh but are never accepted as access tokens. The JSON shape of
  the auth responses is unchanged.
- **Tests:** `auth::claims`.

### 6. Account takeover through email linking (Critical, Auth)

- **Status:** Fixed.
- **Cause:** Azure defaulted a missing `email_verified` to true. On a multi-tenant authority
  (`common`, the DB default), any Entra tenant can assert any email, so it could take over
  accounts, including the bootstrap owner. Apple also defaulted it to true. Proxy assertions
  hard-coded it to true and were accepted even when the provider was not in proxy mode. A login
  with no email skipped the domain allow-list.
- **Fix:** Azure uses the `xms_edov` claim when present; otherwise the email counts as verified
  only for a single-tenant authority. Apple no longer defaults to true. Proxy assertions honour
  an `email_verified` claim and are accepted only for providers with `use_grengin_proxy` set. A
  configured allow-list rejects logins with no email or a placeholder email, and a placeholder
  email never links. GitHub was already correct. The link checks and the identities update run
  in one transaction against the user row locked with `SELECT ... FOR UPDATE`, so concurrent
  callbacks can't link a different subject past each other's check or drop each other's identity.
- **Tests:** `oidc_service`, `auth::azure`.

### 7. Image generation reads any file by id (High, Chat)

- **Status:** Fixed (partial report).
- **Cause:** the owner check already existed, but soft-deleted files were accepted.
- **Fix:** `can_use_input_image` requires the owner and status `Uploaded`.
- **Tests:** `image_gen_helpers`.

### 8. Project sources accept another user's file (High, Projects)

- **Status:** Fixed.
- **Fix:** `project_helpers::ensure_source_file_attachable` allows a file only if it is the
  caller's own uploaded file, or already a source of the same project; otherwise 404.
- **Tests:** `project_helpers`.

### 9. Team-project writes check only read access (High, Projects)

- **Status:** Fixed.
- **Fix:** adding sources and adding, updating or deleting artifacts now require
  `ensure_project_content_access`: the project owner or any explicit member. Users who can only
  read a team project cannot change it. Managing project MCP servers stays owner-level
  (`ensure_project_write_access`). Member roles are now an enum (`ProjectMemberRole`).
- **Tests:** `project_helpers`.

### 10. Personal skills readable by any user (High, Skills)

- **Status:** Fixed.
- **Fix:** `services/skill_access.rs`: a personal skill is visible only to its owner. This
  applies to get, link, list, conversation skills, admin update and delete, and the chat
  stream. Org and department rules are unchanged.
- **Tests:** `skill_access`.

### 11. Department analytics scope bypass (High, Analytics)

- **Status:** Fixed.
- **Fix:** `services/analytics_scope.rs`: org-wide `analytics:view` callers keep the
  all-departments view. Scoped admins get only their administered departments, and a passed
  department id must be inside that scope.
- **Tests:** `analytics_scope`.

### 12. Scoped department admin overreach (High, Departments)

- **Status:** Fixed.
- **Fix:** `services/department_access.rs`. Adding users needs `departments:manage` over both
  the target department and each user's current department. Reparenting needs the new parent in
  scope. Changing a department's budget amount, period or exceed action, or moving a department
  that has a budget, needs `budget:allocate` on its parent (org-wide for top-level departments).
- **Tests:** `department_access`.

### 13. Department deletion widens scoped roles (High, RBAC)

- **Status:** Fixed.
- **Fix:** migration `m20261007_000001_cascade_scoped_role_assignments` changes the
  `user_role_assignments.scopeDepartmentId` FK to `ON DELETE CASCADE`; down restores
  `SET NULL`. Department deletion and its grant cleanup now run in one transaction. Other FKs to
  `departments` were checked; none widens access this way.
- **Not fixable by migration:** grants already widened by past deletions look like real
  org-wide grants. `auth.role_assigned` audit events can help find them.
- **Tests:** migration SQL tests in `migration`.

### 14. Deleted AI key stays live until restart (High, AI engines)

- **Status:** Not reproducible (related race fixed).
- **Cause:** the missing `.await` was fixed in commit 2567501. Verifying found a race: the key
  was evicted before the DB row was disabled, so a concurrent chat could register it again.
- **Fix:** the handler now saves to the DB first, then evicts. It is an ordering change only and
  has no cargo test.

### 15. Unrestricted binary update source (High, System)

- **Status:** Fixed.
- **Fix:** `services/reconfigure.rs`. Update URLs must be https, without userinfo, query or
  fragment, on the configured release origin (`GRENGIN_RELEASE_BASE_URL` or
  `RELEASE_BASE_URL`, default `https://releases.grengin.io`) or the official origin. The URL is
  passed to the script in canonical form. Checksum verification can no longer be turned off.
- **Tests:** `reconfigure`.

### 16. sqlx-mcp read-only bypass (High, MCP)

- **Status:** Fixed.
- **Cause:** the scanner treated `\'` as an escape, which Postgres does not, so a write hidden
  after a backslash passed the scan and then ran through the raw-query fallback.
- **Fix:** every query, including schema tools, runs in a `BEGIN READ ONLY` transaction and is
  rolled back. The raw fallback is removed. The scanner follows Postgres string, dollar-quote
  and comment rules and also blocks `SELECT ... INTO`.
- **Tests:** `sqlx-mcp` `read_only` and `db_manager`.

### 17. MCP OAuth token storage and callback (High, MCP)

- **Status:** Fixed.
- **Fix:** `services/mcp_oauth.rs`. Tokens are stored encrypted (`enc:v1:` plus the APP_KEY
  cipher); existing plaintext rows still work and are re-encrypted on first read. The callback
  requires sign-in, and the OAuth state must belong to the user who started the flow and
  expires. The provider redirects the browser to the webapp route `/mcp/oauth/callback`, which
  calls the API with the user's bearer token, so requiring sign-in doesn't break the flow. The
  state is kept when the token endpoint can't be reached, because only then is the code still
  unused. When the code is redeemed, removing the state and storing the tokens happen in one
  transaction: both or neither. If that write fails, the state remains for a code that is now
  spent; a retry is rejected by the provider, and that rejection consumes the state. For a
  rejected code or a provider-reported error, removing the state is best effort, since the
  provider won't accept the code again and the state expires on its own. Redirects are allowed
  only to relative paths or the `REDIRECT_URL` origin; an invalid redirect is dropped and logged.
- **Tests:** `mcp_oauth`, `handlers::mcp`.

### 18. Inline downloads with client Content-Type (High, Files)

- **Status:** Fixed.
- **Fix:** `services/file_download.rs`. Every download sends `X-Content-Type-Options: nosniff`
  and a safe `Content-Disposition`. A file is inline only if its stored bytes are PNG, JPEG,
  GIF, WebP or PDF; everything else, including HTML and SVG, downloads as an attachment.
- **Tests:** `file_download`.

### 19. Unauthenticated MCP tool listing (Medium, MCP)

- **Status:** Fixed.
- **Fix:** `GET /mcp/tools` requires sign-in, and lists only tools the caller may use under the
  MCP access policies (`mcp_tools::filter_tools_for_user`, the same rule as the chat path's
  `tool_is_usable`). The frontend already sends a token.
- **Tests:** `handlers::mcp`, `mcp_tools`.

### 20. No revocation for deactivated or deleted users (Medium, Auth)

- **Status:** Fixed.
- **Fix:** the `Claims` extractor checks the user's status through `SessionGuard`
  (`services/auth_session.rs`), mounted on the whole router: deactivated or suspended users get
  401, pending users 403, deleted or missing users 401. The refresh endpoint refuses them too.
  The cost is one primary-key lookup per authenticated request, done once per request even when
  both a route layer and the handler extract `Claims`.
- **Tests:** `auth_session`.

### 21. No HTTP timeouts on outbound calls (Medium, LLM)

- **Status:** Fixed (partial report).
- **Cause:** the shared client already had a 15-second total timeout, and LLM provider streams
  set their own timeouts. The real gap was the MCP HTTP transport, which had none.
- **Fix:** `services/http_client.rs`. Short calls: 10 s connect, 10 s read, 15 s total. MCP
  streams: 10 s connect only, because MCP keeps SSE streams open and long tool calls can be
  silent for minutes.
- **Tests:** `http_client`.

### 22. Stream skips the model whitelist and budgets (Medium, Chat and budgets)

- **Status:** Fixed.
- **Fix:** the stream enforces the admin whitelist with the same rule as `GET /models`
  (`chat_stream_helpers::is_model_whitelisted`) and returns `DepartmentModelNotAllowed`.
  `GET /models` now applies the whitelist to plugin models too, so the list and the stream
  agree. The budget check already ran before any price lookup and still does. Requesting chat on
  an image-only provider returns an error instead of panicking.
- **Tests:** `chat_stream_helpers`.

### 23. Panics and per-chunk writes in the stream (Medium, Chat)

- **Status:** Fixed.
- **Fix:** the DB `.expect()` calls are replaced: a failed insert sends `ai_error` then `done`,
  and a failed update stops the loop with `ai_error` (code 5001) and runs the normal wrap-up.
  Text saves are throttled to one every 500 ms; tool events, usage, end of stream and cancel
  still save immediately, and a drop guard saves unsaved text if the client disconnects.
- **Tests:** `chat_stream_helpers`.

### 24. Cancel stops after the first tool round (Medium, Chat)

- **Status:** Fixed.
- **Cause:** the cancel handle was removed when the first round ended, and
  `StreamCancel::cancelled()` ignored a cancel sent while nothing was waiting.
- **Fix:** the handle lives for the whole stream, `cancelled()` checks the flag, and the stream
  stops running further tools once cancelled.
- **Tests:** `chat_stream_helpers`.

### 25. Search snippet panics on non-ASCII text (Medium, Search)

- **Status:** Fixed.
- **Fix:** `truncate_snippet` cuts at 240 characters instead of 240 bytes.
- **Tests:** `search`.

### 26. Embedding dimension change breaks inserts (Medium, RAG)

- **Status:** Fixed.
- **Fix:** `embedding_helpers::apply_embedding_config_update` reads the real vector column
  dimensions from the catalog and rejects a configuration that doesn't match (400, code 6307).
  Changing the column was ruled out: it would mean re-embedding everything, and ivfflat cannot
  index more than 2000 dimensions.
- **Tests:** `embedding_helpers`.

### 27. Department reparenting leaves descendants behind (Medium, Departments)

- **Status:** Fixed.
- **Fix:** PUT and `/move` share `reparent_department`, which locks the department, new parent
  and descendants in one transaction and rewrites every path. Moving a department under itself
  or a descendant returns 409.
- **Tests:** `department_helpers`.

### 28. Departments without a budget treated as exhausted (Medium, Budgets)

- **Status:** Fixed.
- **Fix:** `budget_allocation::BudgetHealth::classify`: an allocated budget of 0 or less means no
  budget (unlimited), never exhausted. Notifications and the chat stream's warn/block gate both
  use it.
- **Tests:** `budget_allocation`, `chat_stream_helpers`.

### 29. Gemini image API key in the URL (Low, Image gen)

- **Status:** Not reproducible.
- **Cause:** Gemini image requests come from a plugin manifest that sends `x-goog-api-key` as a
  header, and manifests cannot map credentials into query parameters.
- **Tests:** regression tests in `llm-plugin/tests/runtime_http.rs`.

### 30. Audit IP and redaction gaps (Low, Audit)

- **Status:** Fixed.
- **Where:** `src/middleware/audit_log.rs`.
- **Fix:** the server now records the connecting address. Forwarding headers are read only from
  a trusted proxy, set by `TRUSTED_PROXIES` (IPs or CIDRs, or `none`; default loopback and
  private ranges). `CF-Connecting-IP` is never trusted. Redaction now hides chat content (text,
  tool calls and results, titles, attachment names, inline data) in conversation and message
  snapshots, and prompt bodies (`promptText`, `customPromptText`, `variables`) in role prompt and
  user prompt snapshots, while still recording which fields changed. Token counts stay visible,
  and OAuth `code` and `assertion` query values are hidden.
- **Tests:** `client_ip`, `audit_log`.

## Review Follow-ups

A review of the branch raised seven findings. Each was checked against the code before acting.

| # | Finding | Outcome |
|---|---------|---------|
| 1 | MCP OAuth callback requires a bearer token that a provider redirect can't send | Not a bug: the provider redirects to the webapp route, which calls the API with the bearer token |
| 2 | Audit logs store role prompt text and users' custom prompts | Fixed: prompt bodies are redacted (issue 30) |
| 3 | A department-scoped Super Admin grant acts org-wide through role-name checks | Fixed: refused, role-name checks require org-wide, existing rows migrated (issue 4) |
| 4 | Concurrent sign-ins can race on email linking and identity updates | Fixed: row lock in one transaction (issue 6) |
| 5 | `GET /mcp/tools` ignores MCP access policies | Fixed: filtered by the caller's access (issue 19) |
| 6 | A rejected edit has already deleted the history | Fixed: the new turn is written hidden and swapped in by exact ids only on success (issue 1) |
| 7 | MCP OAuth state is deleted before the token exchange | Fixed: kept while the token endpoint is unreachable; removed together with storing the tokens (issue 17) |

Two further reviews refined findings 6 and 7. The first fix for finding 6 still lost history
when the provider failed after the edit was applied. The second hid messages up front and
restored them from a background task on failure, which wasn't guaranteed to run and could hide
another tab's messages, and it committed even when the final reply save failed. The final design
above needs no rollback at all. A fourth review found that the cumulative conversation summary
could still carry replaced content into the model and stayed stale after the edit, that an image
message save failure still reported success, and that `message_count` drifted on edits; all three
are fixed as described above. For finding 7, the state is now removed in the same transaction
that stores the tokens.

## Decisions to Confirm

These fixes change behaviour that people may rely on. Each one uses the safest default; any can
be reversed.

- **Issue 4:** only roles whose permissions the assigner holds can be granted. The seeded HR
  Admin can now grant only User and HR Admin, not Observer or Department Admin.
- **Issue 6:** with a multi-tenant Azure authority (`common`), email linking is off, and every
  Azure login fails when an allow-list is set, until the app registration adds the `xms_edov`
  optional claim or uses a single-tenant ID. Proxy assertions without an `email_verified` claim
  are still trusted; the proxy worker should send it before that default is tightened.
- **Issue 9:** any explicit project member can edit sources and artifacts; only owners manage
  project MCP servers.
- **Issue 10:** admins can no longer edit or delete another user's personal skill.
- **Issue 12:** a Department Admin can fund child departments but cannot raise their own budget
  or switch their own exceed action.
- **Issue 15:** a configured http release mirror is refused, and the webapp's checksum toggle
  now returns an error, so the toggle should be removed.
- **Issue 18:** BMP, AVIF and HEIC images download instead of displaying inline.
- **Issue 22:** models with no catalog or plugin price still cost 0. A non-whitelisted model
  reuses error 6002, whose message mentions the department.
- **Issue 28:** a budget of 0 with "block" no longer blocks all spending.
- **Issue 30:** with the default `TRUSTED_PROXIES`, clients on private networks can still fake
  their logged IP; behind CloudFront or Cloudflare, the edge IP is logged until those ranges are
  added.

## Release Notes

- Long-lived hand-made dev tokens without `token_use` stop working as access tokens; mint dev
  tokens with `"token_use": "access"`.
- New environment variables: `TRUSTED_PROXIES` (audit client IP) and
  `GRENGIN_RELEASE_BASE_URL` (binary update origin).
- New migrations: `m20261007_000001_cascade_scoped_role_assignments` and
  `m20261007_000002_demote_scoped_super_admin_assignments` (scoped Super Admin grants become
  Department Admin grants on the same department).
- Multi-tenant Azure deployments with an allow-list need the `xms_edov` optional claim.
- When the assistant reply, or a generated image's message, can't be saved, the client now
  receives a persistence `ai_error` instead of a success event, for normal chats as well as edits.

## Noticed, Not Fixed

- **Data audits:** existing `project_sources` rows may point at other users' files, and grants
  widened by past department deletions can't be told apart from real org-wide grants.
- **Auth:** JIT accepts unverified emails, so a pending account can be created under someone
  else's address. Refresh tokens are not rotated or individually revocable. Open SSE and chat
  streams survive deactivation. Nothing stops removing the last Super Admin.
- **Chat:** after a provider parse error the stream still runs its end-of-stream path. A failed
  edit attempt leaves its messages as hidden rows. The stored `message_count` counts user messages
  while the chat detail endpoint counts all visible messages; that difference predates this work.
- **MCP OAuth retry in the webapp:** the backend keeps the state when the token endpoint is
  unreachable, but the webapp's callback page closes or navigates away after any failure, so
  today a user still restarts the authorization. Offering "Try again" on that page would make the
  retry reachable.
- **Projects:** a conversation linked to a project keeps pulling that project's documents after
  the user loses access; project chat listings show other users' conversation titles to anyone
  who can read the project.
- **MCP and system:** `api_service_name` goes unvalidated into `systemctl restart`; the update
  script pulls the API image by tag and never verifies the downloaded `.sig`; sqlx-mcp has no
  `statement_timeout`.
- **AI engines:** a whitelist entry that matches no catalog model key or name (for example the
  alias `claude-haiku-4-5` while the catalog lists `claude-haiku-4-5-20251001`) exposes nothing;
  the model is neither listed nor usable. Admins should re-save those whitelists.
- **RAG:** an embedding model set only by environment variable uses its native dimension, so
  non-1536 models still fail inserts.
- **Departments:** `DepartmentUpdate.budget_allocated` is `f32` and loses precision;
  `create_department` is not transactional; `/move` doesn't check the new parent's available
  budget.
- **Startup:** the catalog manifest prefetch has no retry, so a connection reset from the CDN
  leaves that manifest to be fetched on demand later. This happens on `main` too.
