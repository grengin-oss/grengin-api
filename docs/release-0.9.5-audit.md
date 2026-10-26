# Grengin 0.9.5 Release Audit

Date: 2026-10-26. Scope: single-instance Grengin API, combined security/share-chat
branch, explicit AI-engine installation, and public release preparation.

## Verdict

The initial audit (originally labelled 0.9.6) held publication for dependency,
version/catalog and runtime-resolution findings. The target was corrected to
0.9.5, and the user subsequently requested merge and release after those findings
were reported. Release preparation now bumps Cargo and adds a version/source-pin
gate before GHCR tag publication. Dependency advisories and the runtime catalog
gap below remain known issues, not resolved findings. This is not a claim of
bug-free software or production verification.

Initial candidate: `feat/share-chat`, HEAD `aec50a3`, plus the uncommitted tree.
The existing MCP-client edits were included in the initial build/tests, but are
excluded from the release commit and its clean-checkout validation.
`origin/main` (`804f09a`) and the security branch are already ancestors. A merge-tree
check against fetched main reported no conflicts. The original audit did not
commit, push, merge or publish; the subsequent release request authorizes those
operations for 0.9.5.

## Findings Still Open

### High: dependency advisory gate fails

`cargo deny check advisories` exited 1: 23 unique advisory IDs, 26 failure entries
(some ZIP advisories repeat across versions), and two yanked-version warnings.
Two of the advisory IDs concern unmaintained crates rather than vulnerabilities.
`Cargo.toml` and `Cargo.lock` are unchanged from main, so these are existing
dependency findings, not new dependencies introduced by this patch.

Examples:

| Dependency | Advisory | Reported fixed version |
| --- | --- | --- |
| bytes | RUSTSEC-2026-0007 | >=1.11.1 |
| h2 | RUSTSEC-2026-0258 | >=0.4.16 |
| rustls | RUSTSEC-2026-0285, -0346, -0348, -0349 | >=0.23.46 covers these reports |
| tokio | RUSTSEC-2026-0386 | >=1.51.6,<1.52.0 or >=1.53.3 |

The full scan also flags anyhow, crypto-bigint, rmcp, rsa, rustls-webpki, time,
webpki-roots, ZIP, proc-macro-error2 and ttf-parser. Do not interpret all reports
as confirmed exploitable application paths: the rmcp report concerns its HTTP
server transport, while this application uses MCP clients and the SQL helper's
stdio transport. ZIP is also pulled into Swagger's build tooling. Each needs
upgrade or documented reachability review; do not blanket-ignore the scan.

References: `Cargo.lock:508` (bytes), `:1514` (h2), `:3442` (rustls), `:4515`
(tokio). Full evidence: `/tmp/grengin-096-advisories.log`.

### Addressed in Preparation: Cargo Version and Publication Gate

The initial checkout declared `0.9.4`. The main-branch image workflow publishes
`latest`, the Cargo version and `v<Cargo version>` (`docker-publish.yml:49-51`).
Merging without changing Cargo would republish the 0.9.4 image tags. Preparation
changes Cargo.toml/Cargo.lock to 0.9.5 and verifies the live compatibility index
matches both that version and the exact image source commit before publishing.

The 0.9.5 compatibility draft contains the same package allowlists as 0.9.4. Its
provisional source pin must be replaced by the final reviewed backend commit and
published before image tags. The publication gate rejects an absent/mismatched
index. Tests cover a valid index and eight invalid/missing cases.

### Medium: installation and runtime select different manifests

`src/services/ai_plugin.rs:183` installs a package validated through
`DiscoveryCatalog::ai_provider`, but leaves catalog `pluginConfig` null as intended.
Later, `provider_runtime.rs:154` uses embedded manifests for the four built-ins
and `provider_manifests.rs:126` loads other manifests from the unversioned catalog
URLs. That fetch parses the manifest but does not verify it against the selected
distribution's digest/version.

An enable/reload can therefore use a different manifest from the one validated
at installation; a later catalog edit can affect an older backend unexpectedly.
This is confirmed by the code paths, not an injected production failure test.
Use the selected distribution consistently for hydration, model metadata and
refresh, and add a test where root/latest differs from the selected package.
Make the built-in native-manifest policy explicit and consistent at creation and
reload. This pre-existing gap was documented, not silently refactored in this
database-persistence fix.

### Medium: release preparation only partially validates the catalog

Fetched public Grengin main is `69fb494`. Its `scripts/prepare-release.sh` checks
catalog format, version and nonempty provider arrays, but not `grenginApiCommit`
against the selected image revision, or the complete runtime-required document.
Its passing fixture omits `grenginApiCommit` and `catalogCommit` altogether, while
`DiscoveryCatalog` requires both. The preparation script can accept a document
the backend later rejects. Add a full catalog preflight and source-commit check
before automated tagging. No frontend/release-workflow source was changed here.

## Fixes Completed and Verified

- Removed startup reconciliation that inserted every discovery provider.
- Explicit POST installs exactly one discovery key or custom manifest, not both.
  Catalog manifests remain outside the database; custom manifests and encrypted
  credentials retain the existing AI-engine persistence model.
- New engines default to at most three declared recent generators, excluding
  embeddings and explicitly retired entries. Missing dates use deterministic
  ordering, not a claim of release recency. A key enables an engine unless an
  explicit enabled state says otherwise; validation status stays NotValidated.
- First-key activation preserves small/empty existing allowlists. Larger legacy
  lists can only be narrowed within previously allowed models. Rotation preserves
  policy. Blank keys are rejected without writes.
- Same-key concurrent creates serialize through a PostgreSQL advisory transaction
  lock; three simultaneous requests produce one row and two 409 responses.
- Unrelated key rotation no longer writes a stale enabled flag. A controlled
  concurrent database disable remains disabled in the row and API response.
- Catalog-backed rows can be deleted explicitly and are not recreated on restart.
  Existing rows are not automatically purged: old disabled placeholders remain
  until an administrator deliberately deletes them.
- SSE/notification transport is unchanged. Proposed SSE fallback edits were removed.

## Fresh Verification

Command: `GRENGIN_ENGINE_INSTALL_TESTS=1 bash tests/staging/run.sh`.
Rust used two jobs. All database writes targeted disposable loopback PostgreSQL;
the source database was only read with pg_dump. Copied MCP connections were disabled.

| Gate | Result |
| --- | --- |
| Locked workspace tests | 736 passed, 0 failed; 60 ignored by default |
| Locked workspace build | Passed |
| Formatting and git diff whitespace checks | Passed |
| Clippy, all workspace targets | Completed with repository warnings; not warning-free |
| Explicit installation HTTP/state assertions | 42 passed, plus restart/no-reinstallation checks |
| Security HTTP assertions | 23 passed |
| Extended security/edit/role/department assertions | 93 passed |
| Share-chat and notification assertions | 375 passed |
| navikt browser/mobile/provider OAuth tests | 8 passed |
| Explicit PostgreSQL regressions | 8 passed |
| Public release-preparation fixture checks | Passed |
| cargo-deny licenses | Passed |
| cargo-deny advisories | Failed; see findings |

There are 533 HTTP/state assertions, not 533 independent user journeys. The
explicit OAuth/database tests are among the normally ignored tests; do not add
them to the workspace count as though they were new distinct tests.

Detailed logs: `/tmp/grengin-staging-validation-ekV3cZ`,
`/tmp/grengin-096-verified-tests.log`, `/tmp/grengin-096-release-preparation.log`,
`/tmp/grengin-096-licenses.log`, `/tmp/grengin-096-advisories.log`.

## Boundaries and Release Checklist

Clean 0.9.5 release validation subsequently passed 732 workspace tests, the same
533 HTTP/state assertions, 8 OAuth tests, 8 database tests and 9 publication-gate
cases. Four MCP-client tests belong to excluded local work, explaining the change
from the initial 736. Reports: `/tmp/grengin-staging-validation-xtPPD2` and
`/tmp/grengin-v095-release-validation.log`. This run explicitly selected the
existing 0.9.4 catalog for debug HTTP fixtures because its package allowlists are
identical to the prepared 0.9.5 index. Production release builds ignore that debug
override and require their matching 0.9.5 distribution.

- This is single-instance testing. No Redis/distributed-runtime support is claimed.
- No paid-provider, production OAuth, deployed Lambda, native mobile SDK or browser
  UI validation was performed. Local provider/token endpoints are controlled fixtures.
- Native ARM64 and production Docker images were not rebuilt in this audit. The
  workflow uses native AMD64/ARM64 runners; successful local tests do not replace
  the release-image builds or a deployed staging smoke test.
- Preserve existing user MCP/catalog edits when preparing commits. The public
  frontend checkout remains on its own branch with its pre-existing changes;
  fetched release automation was inspected/tested in temporary files only.
- Back up staging before the share migration. Its down migration deletes shares
  and their notifications and is not a lossless rollback.
- Existing documented share limitations remain: optional indexing can be skipped
  at capacity, and a crash while staging file copies can leave orphan bytes.
- Resolve the findings, prepare reviewed version/catalog pins, then obtain staging
  approval before merge/publication. Publish the approved compatibility index
  before releasing the corresponding backend image and public Grengin bundle.
