# Full-stack end-to-end tests

Playwright drives the real webapp against a real grengin-api and PostgreSQL. Only the outside
world is mocked:

| Service | Mock | Port |
| --- | --- | --- |
| Identity provider (OIDC, PKCE) | `mocks/oidc.mjs`, login page with one button per user in `mocks/users.mjs` | 9401 |
| LLM provider (OpenAI-compatible chat completions) | `mocks/llm.mjs`, steered by `[slow:N]`, `[tool:NAME]`, `[fail]` in the prompt | 9402 |
| Metadata catalog (meta.grengin.com) | `mocks/static.mjs` serving a grengin-list `master-data` checkout | 9403 |

grengin-api listens on 18200 and the webapp (`vite preview`, proxying `/api`) on 4173.

## Run

On a Docker host (the staging VM):

```bash
WEBAPP_DIR=/path/to/webapp CATALOG_DIR=/path/to/grengin-list bash tests/e2e/stack/run.sh
```

Without `WEBAPP_DIR` / `CATALOG_DIR` it clones `WEBAPP_REF` (default `main`) and `CATALOG_REF`
(default `main`). `E2E_GREP` limits the run to matching tests. Results land in
`$E2E_WORK/report` (HTML report, JUnit, traces and videos of failures) and logs in `$E2E_WORK/logs`.

## How a run starts

1. grengin-api starts with `BOOTSTRAP_SUPER_ADMIN_EMAIL=owner@e2e.grengin.test`, so the owner exists
   before anyone signs in.
2. `support/global-setup.ts` uses a short-lived token for that owner to add the mock IdP as a sign-in
   provider and the mock LLM as a custom AI engine, through the real admin API.
3. `specs/login.setup.ts` signs the owner in through the UI and the IdP; other specs reuse that session.
