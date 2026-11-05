# Gemini tool schema fix (2026-11-05)

## Cause and scope

Gemini's `parameters` field is Google's restricted Schema object, not arbitrary
JSON Schema. Forwarding an MCP schema containing `$schema` or
`additionalProperties` reproduced HTTP 400 `INVALID_ARGUMENT` from Google.
The bundled Gemini manifest now sends `parametersJsonSchema` instead, preserving
the original object. It never sends both mutually exclusive fields.

Reference: [Google FunctionDeclaration](https://ai.google.dev/api/generate-content#FunctionDeclaration).

The bundled Gemini manifest advances from version 1.1 to 1.2, and the backend's
overlay loader now preserves that version in the effective provider descriptor.
The matching grengin-list change creates Gemini package 1.6. No schema conversion,
credential changes, database migrations, endpoint changes, web-search changes,
or changes to other providers are included.

## Verification

- `llm-plugin`: 134 tests passed; 44 opt-in live/catalog tests ignored by default.
- Explicit catalog tests passed: Gemini JSON Schema/tool continuation and the
  existing all-provider streaming mock check (18 current provider manifests).
- Rust manifest validation passed for current manifests and historical snapshots.
- New local cases preserve parameterless schemas, nullable parameters, `$schema`,
  `$defs`/`$ref`, nested objects, arrays and `additionalProperties: false`.
  A negative test deliberately restores `parameters` and reproduces the 400.
- Real Google streaming and synthetic tool-result continuation passed on
  `gemini-3.1-flash-lite`, separately using the bundled and catalog manifests.
  These exercise the real plugin runtime, not a reimplemented HTTP client.
- Other live results are NOT counted as passes: `gemini-3.5-flash-lite` returned
  503 during continuation; `gemini-3.8-flash` returned 503 at stream start;
  `gemini-2.5-flash` returned 404 at stream start. An earlier direct 3.8 request
  timed out. These are model/service availability results, not schema rejections.
- The original credential reached 402 depleted prepaid credits after the schema
  correction. Tests above used the replacement credential. Nothing here asserts
  or changes the project's billing/free-tier status.
- No real connector was called: the test returns a synthetic fixture code. No
  existing API process, user database, frontend or production catalog was changed.

## Repeatable checks

`tests/providers/gemini-tools.sh` uses one Rust job, a 2 GiB systemd memory cap
(4 GiB for the full backend workspace), and no swap. It requires Linux user
systemd, Cargo and Node for the mock server.

```bash
bash tests/providers/gemini-tools.sh suite
bash tests/providers/gemini-tools.sh backend-suite
bash tests/providers/gemini-tools.sh mock
export GRENGIN_PROVIDER_CATALOG_DIR=/path/to/grengin-list/master-data/ai-providers
bash tests/providers/gemini-tools.sh catalog
bash tests/providers/gemini-tools.sh catalog-all
bash tests/providers/gemini-tools.sh validate
GRENGIN_GEMINI_SECRETS_FILE=/path/to/private/secrets.sh \
  GEMINI_TEST_MODEL=gemini-3.1-flash-lite \
  bash tests/providers/gemini-tools.sh live
```

Omit `GRENGIN_PROVIDER_CATALOG_DIR` to live-test the bundled manifest. Credentials
are sourced inside the capped process, never passed as CLI arguments or URL
parameters. Live tests are opt-in and may consume provider quota.

Released distribution indexes are immutable. Publishing a new catalog package
alone does not change existing backends' pinned plugin versions; promote package
1.6 through a reviewed new distribution before claiming deployed discovery users
have the fix. Tests do not merge, release or publish to R2.

## Pre-merge audit follow-ups

- Propagate the chat overlay's version into the effective backend manifest.
  Regression coverage checks all four built-in descriptors and ensures the
  merged Gemini configuration retains `parametersJsonSchema`.
- The catalog's separate opt-in HTTP diagnostic now refuses redirects rather
  than risk forwarding its API-key header. Tests cover blocked redirects,
  redacted errors, malformed/HTML error responses and transport failures.
- Existing release indexes and historical package bytes are unchanged.
