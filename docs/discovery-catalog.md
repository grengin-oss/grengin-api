# Provider Discovery Catalog

Provider discovery is the compatibility boundary between `grengin-api` and the public metadata
published at `meta.grengin.com`. It does not read installation configuration and never returns
credentials. Each backend release consumes one version-scoped package distribution selected by
its Cargo package version.

## Endpoints

- `GET /discovery/auth-providers`
- `GET /discovery/auth-providers/{provider}`
- `GET /discovery/ai-providers`
- `GET /discovery/ai-providers/{provider}`

All endpoints accept `?version=1` for the newest compatible release in major version 1, or an exact
release such as `?version=1.1.1`. A missing artifact may fall back only to an older compatible
release in the same requested major. Exact requests never fall back.

API envelopes use snake case. The embedded auth template or AI plugin keeps its native JSON field
names. Every response includes `distribution_version` so operators can trace the compatibility
decision back to the running backend release. Responses also include a strong ETag and public
five-minute revalidation policy.

## Distribution Resolution

Provider packages are stored once, while each backend release receives its own compatibility
index:

```text
distributions/grengin-api/{CARGO_PKG_VERSION}/index.json
```

The distribution identifies the backend release and records the catalog revision plus every
supported auth and AI package release and digest. Provider release compatibility is not maintained
as a Rust allowlist. The distribution itself is the allowlist, and `?version` selectors operate
only inside it. The default selector starts at each provider's pinned `defaultVersion`; it advances
only when that backend distribution is deliberately updated after compatibility testing, never
merely because a newer package appears in the global catalog.

Production does not fall back to another backend distribution. Debug builds may set
`GRENGIN_PROVIDER_DISTRIBUTION_VERSION` to exercise a local fixture. Provider IDs and versions are
validated and package paths are constructed by the backend, so distribution metadata cannot point
the server at an arbitrary origin.

The distribution format version remains a typed host protocol boundary. Every selected package is
also SHA-256 checked and parsed by the production `OidcProviderConfiguration` or
`ProviderManifestV1` type before it is returned. A malformed or unsupported package therefore
fails closed even if a bad distribution references it.

## Publishing

Each release is stored at an immutable path:

```text
auth-providers/{provider}/versions/{version}/provider.json
ai-providers/{provider}/versions/{version}/plugin.json
distributions/grengin-api/{grengin-api-version}/index.json
```

Indexes declare each release's schema version, runtime contract version, and SHA-256. The API checks
the digest and then parses the artifact with the production runtime type before returning it.
Changing an existing provider package requires a new provider version. A backend distribution may
add and select a newer compatible package without rebuilding the backend, but it must retain the
same backend version and commit and move to a new catalog revision. Catalog generators reject byte
changes under an existing package version and reject distribution changes that are not valid for
that backend contract.

The runtime revalidates the distribution and package at a five-minute interval by default. Only
enabled discovery-backed providers are candidates. The candidate digest and manifest are checked
and compiled before an atomic registry replacement; database policy is re-read after network I/O,
and every failure leaves the active provider untouched. Set
`GRENGIN_PLUGIN_REFRESH_INTERVAL_SECONDS=0` to disable refresh or set a positive number to override
the interval.

The CDN's current aliases remain available for older clients. During the `providers` to
`ai-providers` transition, publishing also refreshes both path families so deployed instances do not
freeze on stale model or pricing metadata.
