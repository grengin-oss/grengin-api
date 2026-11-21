# Explicit AI Engine Installation

Startup loads existing AI engines. Discovery lists available packages.
Install a provider with `POST /admin/ai-engines`.

```json
{"engine_key":"groq","api_key":"YOUR_PROVIDER_KEY"}
```

Alternatively supply `plugin_config` for a custom engine. Direct providers use
one of these two installation sources. Catalog installations keep `pluginConfig`
null, and secrets remain encrypted with APP_KEY. No database migration or separate plugin CRUD API
is required. Explicit DELETE supports catalog-backed rows; restart does not recreate them.

A first key enables the engine unless `is_enabled` is explicit. Omitted model
policy selects at most three generators: up to two chat models plus one image
model, or three chat models without image support. Valid launch dates determine
newest first; model IDs break ties. Embeddings and explicitly retired/deprecated
models are excluded. Without release metadata, ordering is deterministic rather
than a guarantee of newest release. Providers without declared models need an
explicit whitelist; empty never means allow all.

Explicit whitelists, including `[]`, take precedence. Key rotation preserves
existing enabled state and model choices. Adding the first key preserves existing
allowlists of three or fewer entries, including a deny-all list. Larger legacy
lists may be narrowed to the starter set, but only within their previously allowed
models; unknown-only lists remain unchanged. Blank keys are rejected; key deletion
has its own existing DELETE endpoint.

Existing rows are not automatically purged, because they may contain intentional
administrator policy. No production database is modified by this change.
SSE and notification behavior are unchanged; the proposed fallback was withdrawn.

### Grengin workspace funding

Install the `grengin` catalog package on a workspace already connected to the
service and enabled for workspace-funded inference. Set `GRENGIN_IDENTITY_FILE`
to the absolute path of its owner-only (0600) identity file. The backend signs
refresh proofs locally and keeps the resulting short-lived bearer in memory.
The existing admin connection test checks workspace status without generating content.

Select models by their exact `provider/native_model_id` from the engine's model
list. Install disabled to review available models before enabling it.

```json
{"engine_key":"grengin","is_enabled":false,"whitelisted_models":[]}
```

An enabled customer-owned key for an allowed model takes precedence. On the
workspace-funded route the backend sends the grant and the native model inputs
to the identity file's service origin. The gateway supplies the upstream credential.
Department policies, tool permissions and per-user usage stay in the workspace.

## Tests

```bash
bash tests/ai-engines/run.sh --live
bash tests/staging/run.sh
```

The engine runner empties engines only in a disposable PostgreSQL clone, verifies
startup/listing do not add rows, tests installation/defaults/encryption/rotation,
and restarts the backend to verify deleted catalog entries remain absent.
Only public catalog metadata is fetched; model calls use local fixtures. The
combined staging runner covers existing security, sharing and notifications.
Rust uses two jobs; the developer database and production remain untouched.
