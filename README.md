<p align="center">
  <a href="https://grengin.com">
    <img src="swagger-overrides/grengin-logo.svg" width="520" alt="Grengin">
  </a>
</p>

<h1 align="center">Grengin API</h1>

<p align="center">
  The open-source Rust backend for the self-hosted Grengin AI platform.
</p>

<p align="center">
  <a href="https://grengin.com">Website</a> |
  <a href="https://grengin.com/docs">Documentation</a> |
  <a href="https://github.com/grengin-oss/grengin">Grengin application</a> |
  <a href="https://github.com/grengin-oss/grengin/releases">Releases</a> |
  <a href="LICENSE">License</a>
</p>

<p align="center">
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/badge/license-Apache--2.0-blue"></a>
  <img alt="Rust" src="https://img.shields.io/badge/Rust-2024-000000?logo=rust&amp;logoColor=white">
  <img alt="Axum" src="https://img.shields.io/badge/Axum-0.8-1f2937">
  <img alt="PostgreSQL" src="https://img.shields.io/badge/PostgreSQL-pgvector-4169E1?logo=postgresql&amp;logoColor=white">
  <img alt="Docker" src="https://img.shields.io/badge/Docker-amd64%20%7C%20arm64-2496ED?logo=docker&amp;logoColor=white">
</p>

`grengin-api` provides Grengin's authentication, authorization, AI model
routing, streaming chat, projects, retrieval, MCP integration, administration,
analytics, and audit APIs. It is built with Rust, Axum, SeaORM, and PostgreSQL
with pgvector.

## Capabilities

- **Multi-provider AI**: OpenAI, Anthropic, Mistral, and Gemini chat and image models
- **Streaming chat**: server-sent events, tool execution, attachments, and artifacts
- **Authentication and SSO**: JWT sessions, Google OAuth, and Microsoft Entra ID
- **Authorization**: roles, permissions, departments, scoped assignments, and model policies
- **Projects and RAG**: project sources, semantic retrieval, summaries, skills, and full-text search
- **MCP integration**: HTTP, SSE, and stdio servers with OAuth and per-tool access rules
- **Administration**: users, budgets, prompts, branding, providers, and system metrics
- **Analytics and audit**: usage accounting, exports, retention, and administrative audit logs
- **OpenAPI**: generated API schema and a bundled Swagger UI
- **Database lifecycle**: versioned SeaORM migrations applied automatically at API startup

## Grengin Releases

This repository is versioned as a backend component. Public product releases
are published from [`grengin-oss/grengin`](https://github.com/grengin-oss/grengin/releases)
under one Grengin version.

Each product release pins an exact `grengin-api` commit and includes:

- frontend and backend source in ZIP and TAR.GZ formats;
- static Linux amd64 and arm64 `grengin-api` binaries;
- static Linux amd64 and arm64 `sqlx-mcp` binaries; and
- a release manifest containing component versions and commit SHAs.

The backend is never pulled from a moving branch when a product release is
assembled.

## Architecture

```text
Grengin web application
        |
        | REST + Server-Sent Events
        v
Grengin API (Rust + Axum)
        |
        +-- PostgreSQL + pgvector
        +-- OpenAI / Anthropic / Mistral / Gemini
        +-- Google / Microsoft Entra ID
        +-- MCP servers and tools
        `-- SQLx MCP read-only database tools
```

## Local Development

### Requirements

- Rust 1.91 or newer
- PostgreSQL 16 or newer with `pgvector` and `ltree`
- `pkg-config`, a C compiler, and Perl for vendored OpenSSL builds

### Database

Start a local pgvector-enabled PostgreSQL instance:

```bash
docker run --name grengin-postgres \
  -e POSTGRES_USER=grengin \
  -e POSTGRES_PASSWORD=grengin \
  -e POSTGRES_DB=grengin \
  -p 5432:5432 \
  -d pgvector/pgvector:pg16
```

### Configuration

```bash
git clone https://github.com/grengin-oss/grengin-api.git
cd grengin-api
cp src/sample.env .env
```

Replace the sample secrets before starting the API:

```bash
openssl rand -hex 32
openssl rand -base64 32
```

Set the first value as `JWT_SECRET` and the second as `APP_KEY`. The four core
settings are:

| Variable | Purpose |
|---|---|
| `DATABASE_URL` | PostgreSQL connection URL |
| `JWT_SECRET` | Token signing secret |
| `APP_KEY` | Base64-encoded 32-byte encryption key |
| `REDIRECT_URL` | Public frontend origin and OAuth callback base |

Provider credentials, SSO settings, RAG controls, and optional maintenance
settings are documented in [`src/sample.env`](src/sample.env).

### Run

```bash
export SWAGGER_UI_OVERWRITE_FOLDER="$PWD/swagger-overrides"
cargo run --locked
```

Pending database migrations run automatically before the server starts.

- API root: `http://localhost:8080/`
- Swagger UI: `http://localhost:8080/swagger-ui`
- OpenAPI schema: `http://localhost:8080/openapi.json`

## Build And Test

Keep Rust builds limited to two parallel jobs in development and CI:

```bash
cargo check --locked --workspace --jobs 2
cargo test --locked --workspace --jobs 2
cargo build --release --locked --workspace --jobs 2
```

### Docker

Build the static runtime image:

```bash
docker build -t grengin-api .
```

Merges to `main` publish the multi-architecture image:

```text
ghcr.io/grengin-oss/grengin-api:latest
```

The image supports `linux/amd64` and `linux/arm64`.

## Database Migrations

The API applies all pending migrations on startup. For explicit migration
management:

```bash
cargo run --locked -p migration -- status
cargo run --locked -p migration -- up
cargo run --locked -p migration -- down
```

Review migration files before running `down`, `reset`, `refresh`, or `fresh` in
an environment that contains data.

## SQLx MCP Server

The workspace includes `sqlx-mcp`, a read-only PostgreSQL MCP server for schema
discovery and parameterized queries:

```bash
cargo run --locked -p sqlx-mcp -- \
  --database-url "$DATABASE_URL"
```

It exposes read-only tools for queries, table descriptions, foreign keys,
`ltree` columns, and database status over stdio.

## Repository Structure

```text
grengin-api/
|-- src/
|   |-- auth/              # JWT, OAuth, SSO, and permissions
|   |-- handlers/          # HTTP endpoint implementations
|   |-- routes/            # Axum routers
|   |-- services/          # Domain and integration services
|   |-- models/            # SeaORM entities
|   |-- dto/               # API request and response types
|   |-- llm/               # AI provider integrations
|   `-- docs/              # OpenAPI definitions
|-- migration/             # Versioned database migrations
|-- sqlx-mcp/              # Read-only PostgreSQL MCP server
|-- swagger-overrides/     # Branded Swagger UI assets
|-- Dockerfile             # Static multi-architecture image
`-- .github/workflows/     # Build, test, and GHCR publishing
```

## Security

- Provider and OAuth credentials are encrypted using `APP_KEY`.
- Authorization is enforced through scoped roles and permissions.
- MCP servers and tools have explicit access policies.
- Administrative changes are recorded in audit logs.
- CORS is restricted to `REDIRECT_URL` unless explicitly overridden.

Never commit `.env`, production secrets, provider keys, or database snapshots.
Report security issues privately through the contact details on
[grengin.com](https://grengin.com).

## Contributing

Issues and small pull requests are welcome; see
[CONTRIBUTING.md](CONTRIBUTING.md). There is no contributor agreement
to sign, since Apache 2.0 Section 5 already covers submissions.

## License

Grengin is free and open source software under the
[Apache License 2.0](LICENSE).

Use it, modify it, self-host it, embed it, resell it, or run it as a
managed service, commercially or not, at any company size. No usage
caps, no commercial license, nothing to sign. Releases published
before this one remain under the Grengin Sustainable Use License they
shipped with.

The Grengin name and logo are trademarks of Perter Technology
Solutions Private Limited and are not covered by the Apache license.
Forks and third-party distributions need their own name; see
[TRADEMARKS.md](TRADEMARKS.md).
