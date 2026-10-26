# Explicit AI Engine Installation

Startup loads existing AI engines only. Discovery lists available packages; it
does not install all 18 providers. Install one with `POST /admin/ai-engines`:

```json
{"engine_key":"groq","api_key":"YOUR_PROVIDER_KEY"}
```

Alternatively supply `plugin_config` for a custom engine. Exactly one of those
sources is required. Catalog installations keep `pluginConfig` null, and secrets
remain encrypted with APP_KEY. No database migration or separate plugin CRUD API
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
