# Provider Discovery Catalog

Provider discovery is the compatibility boundary between `grengin-api` and the public metadata
published at `meta.grengin.com`. It does not read installation configuration and never returns
credentials.

## Endpoints

- `GET /discovery/auth-providers`
- `GET /discovery/auth-providers/{provider}`
- `GET /discovery/ai-providers`
- `GET /discovery/ai-providers/{provider}`

All endpoints accept `?version=1` for the newest compatible release in major version 1, or an exact
release such as `?version=1.1.1`. A missing artifact may fall back only to an older compatible
release in the same requested major. Exact requests never fall back.

API envelopes use snake case. The embedded auth template or AI plugin keeps its native JSON field
names. Responses include a strong ETag and public five-minute revalidation policy.

## Compatibility

The backend hardcodes supported contract versions, not provider releases:

- Discovery index schema: `1.0`
- Auth template schema: `1.0`
- OIDC configuration: `1.0` and `1.1`
- AI provider manifest: the `llm-plugin` crate's `SUPPORTED_MANIFEST_VERSION`

Unsupported entries are omitted from list responses and cannot be fetched through detail routes.
Provider IDs are validated and artifact paths are constructed by the backend, so catalog URLs
cannot redirect requests to arbitrary hosts.

## Publishing

Each release is stored at an immutable path:

```text
auth-providers/{provider}/versions/{version}/provider.json
ai-providers/{provider}/versions/{version}/plugin.json
```

Indexes declare each release's schema version, runtime contract version, and SHA-256. The API checks
the digest and then parses the artifact with the production runtime type before returning it.
Changing an existing release requires a new provider version; the catalog generator rejects byte
changes under an existing version path.

The CDN's current aliases remain available for older clients. During the `providers` to
`ai-providers` transition, publishing also refreshes both path families so deployed instances do not
freeze on stale model or pricing metadata.
