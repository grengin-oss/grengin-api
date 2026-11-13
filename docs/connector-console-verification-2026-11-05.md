# Connector Console Verification

Date: 2026-11-05. Both repositories remain on `feat/connector-server-plugin`.
This patch is local and uncommitted. No pushes, deployments or R2 uploads.

Later verification supersedes the original resource-blocked results below:
the user approved 4 GiB, fresh build and 853 workspace tests passed, and 65 HTTP
plus 619 full OAuth checks passed. The fresh HTTP run also found/fixed camelCase
People-query projections for native/broker connections. See
[the current session-cache report](connector-session-cache-audit-2026-11-05.md).

## Implemented

- Safe native identity, selected authentication method, supported methods and
  optional suggested prompts in the existing signed-in server listing.
- A typed connection state for that listing, including expired-but-refreshable
  sessions and missing credentials. These are stored states, not vendor probes.
- Explicit administrator-selected token mode for OAuth manifests, including
  Notion internal integration/PAT tokens. Public-mode downgrade is refused.
- Auth mode, primary authentication or credential-delivery changes invalidate
  saved credentials. Presentation-only version updates retain them.
- Admin detail/people endpoints and a permission-checked, transactional tool
  switch, reusing existing tables and effective-access policies.
- Actual seven-day tool execution counts, not fabricated draft/PII metrics.
- Additive chat result metadata: connector/tool names, execution UUID, timestamp,
  duration and a bounded preview; the original output is unchanged.
- Preview serialization is bounded, rather than formatting an arbitrarily large
  response before truncating it. MCP routes use `private, no-store`.
- Execution-list `limit=0` is clamped, avoiding division by zero.
- New Gmail/Notion 1.0.1 packages and a separate **unpublishable** 0.9.8 candidate,
  `docs/drafts/grengin-api/0.9.8/connector-console-index.json` in the catalog repo.
  Existing 1.0.0 packages and released distribution files are unchanged.

The frontend contract is [connector-console-api.md](connector-console-api.md).
Handlers only extract/call/return. Console queries live in a typed service; DTOs
are separate, domain states are enums, and auth selection is shared with the
runtime. No migrations, distributed coordination or new secret storage is added.

## Checks Run

| Check | Result |
| --- | --- |
| `console.sh check` | PASS: entire workspace, all targets, under 2 GiB |
| `console.sh runtime` | PASS: 28 runtime tests + 1 tool-category test; 3 catalog tests intentionally ignored here |
| `console.sh catalog` | PASS: the 3 cross-repository tests, including all 206 tool contracts with two separate caller tokens |
| Catalog `scripts/test_connector_plugins.sh` | PASS: 15 Python tests, deterministic/immutable generation, all 36 versioned manifests, local distribution schemas |
| Draft release validation | PASS: intentionally rejects null source pins, preventing accidental release |
| `console.sh lint` | Completes; no diagnostics in the new console modules or connector runtime. Workspace warnings remain; no full baseline comparison was run |
| Formatting and `git diff --check` | PASS |
| Bash/Node syntax | PASS |
| New API HTTP tests | NOT VALIDATED: patched backend could not be built within the memory limit |
| Full backend/workspace unit execution | BLOCKED: backend test compilation exceeded the memory limit |
| Existing API 8080, native API 8083, webapp 5173 | Still HTTP 200; not restarted or upgraded by this patch |
| Existing native discovery after compatibility fix | PASS: Gmail/Notion return 200 with version 1.0.0 |

Every new Rust build/test process was cgroup-limited to 2 GiB, no swap and one
Cargo job. Backend code generation exceeded that limit despite removing debug
information. A compiler-only, single-thread/MIR/codegen experiment also failed;
those experimental flags are not persisted or required by the source. Do not
raise the user's limit or call this merge/staging ready based on type checking.

## HTTP Harness Incident

The first stale-binary guard used `rg` inside process substitution. The systemd
manager's PATH lacked `rg`, and that failure did not stop the shell. The suite
therefore started the **old** binary against a disposable database, passed ten
baseline checks, then failed with 404 at the first new route. This was not a
validation of the patch and is not included in the passing counts above.

Fixed the wrapper to preserve PATH and explicitly fail when source enumeration
fails. The final guard test correctly refused the stale binary before starting
another database/server. The temporary container and server from the first run
were cleaned up. Existing live databases, browser accounts and services were not
used by the HTTP fixtures or modified.

The expanded HTTP suite is checked in as source, not claimed as passed. It covers
admin-only roster access, secret omission, pagination boundaries, inactive users,
refreshable and empty credentials, exact user/connection counts, private cache
headers, readable persisted tool results, tool toggle permissions/rollback, token
isolation, auth-mode changes and protection against public-auth downgrades.

## Running Catalog Compatibility

The already-running local backend is older than this patch and rejects the new
optional presentation field. Serving the freshly generated candidate temporarily
made Gmail/Notion discovery return 503. This was found with a read-only check.
The old package bytes and installed connections were never changed.

Saved the new compatibility index as `connector-console-index.json` and restored
only our changes to the active local fixture `docs/drafts/grengin-api/0.9.8/index.json`.
The fixture stays on the previous committed package set. After the existing
five-minute metadata cache expired, both local discovery requests returned 200
with version 1.0.0, without restarting services or touching credentials.

The generator now accepts a validated `--draft-name connector-console-index.json`
option. Its tests verify that a new candidate cannot overwrite the active fixture,
that path traversal is refused, and that released distributions remain protected.
`console.sh local-compatibility` reproduces the existing-backend check. The new
candidate requires a matching rebuilt backend and review before publication.

## Still Outside This Patch

- UI implementation, reminder notifications, write tools and per-action write
  confirmation. Unsupported flags are explicit so the frontend can hide them.
- Shared native OAuth broker integration in grengin-service. Existing broker
  bindings remain separate; preloading credentials does not integrate native tools.
- Real Notion consent or integration-token testing. Authentication behavior is
  based on the vendor's documented bearer-token protocol and local runtime tests.
- Full backend HTTP validation with a freshly built binary. This remains a
  merge/deployment gate; prior six-provider live results do not validate new routes.

## Reproduce

```bash
bash tests/connector-plugin/console.sh check
bash tests/connector-plugin/console.sh runtime
bash tests/connector-plugin/console.sh catalog
bash tests/connector-plugin/console.sh lint
```

The remaining commands still enforce the same memory limit and are expected to
need a backend build that fits it before the new HTTP cases can run:

```bash
bash tests/connector-plugin/console.sh unit
bash tests/connector-plugin/console.sh build
bash tests/connector-plugin/console.sh http
```
