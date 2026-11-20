# PR 102 follow-up: authorization and model capabilities

Tool replacements acquire their ordered tool-row locks before reading the actor's
current admin status and server access ceiling. All checks and mutations then use
the transaction's own connection. A queued replacement cannot reuse permissions
revoked during its lock wait, and cannot stall waiting for another pooled connection.
Existing-rule ownership checks remain under the same locks; bulk requests remain
atomic and reject unknown or wrong-server tools before writing.

The new PostgreSQL regressions observe `pg_stat_activity` before revoking either
server access or the actor's admin role. They assert rejection, unchanged
inheritance, and preservation of another creator's rules. They use a one-connection
pool; the existing two-connection contention regression remains in place.

Run database regressions only against a disposable database:

```sh
GRENGIN_SECURITY_TEST_DATABASE_URL='postgres://.../grengin_security_test' \
  cargo test --locked --bin grengin-api services::mcp_access::tests \
  -- --include-ignored --test-threads=1
```

Search eligibility is now model-specific. Explicit installed-manifest metadata
takes precedence over the global catalog, a missing flag may fall back to catalog
metadata, and an unknown model cannot gain search from its provider's name. Malformed
manifest flags fail closed. The existing request/skill search opt-in is still required.

This patch does not add new browsing providers, audio APIs, credit purchasing, or
provider-balance monitoring. The catalog companion adds validation rather than
enabling unsupported features. Neither patch deploys or publishes anything.

## Validation and limits

On 2026-11-19, full-backend `cargo check --locked` and formatting/diff checks passed.
A low-memory harness compiled the production authorization, access, model and
error/DTO modules without mocked authorization or database code: 23 regular tests,
four PostgreSQL access-race tests and the actual migration race test passed. The
database was disposable, with schema and non-personal RBAC seeds only.

Both new revocation tests were also run against the original `e930982` access
implementation with the same test code; both failed, confirming the regressions.
The existing exhausted-pool and protected-rule regressions pass with the fix.

Full binary builds were attempted but stopped by a 2.2 GB memory cap and then a
watchdog preserving 2 GB of available system memory. This is not a claim that the
whole workspace suite or a new release binary was built. Re-run those on the VM
before merging or deploying.

Clippy on Rust 1.98 reports existing warnings across the repository, including the
unchanged SSE `drain_collect` site. The edited crate and all targets type-check with
Clippy without `-D warnings`, with no diagnostics on the edited lines; strict
Clippy is not green. No unrelated lint cleanup or suppression was added.
