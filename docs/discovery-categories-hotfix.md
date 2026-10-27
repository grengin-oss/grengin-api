# Discovery Category Metadata

`GET /discovery/ai-providers` merges the optional presentation sidecar at
`distributions/grengin-api/{distribution_version}/categories.json`.

```json
{
  "distribution_version": "0.9.5",
  "catalog_type": "ai_providers",
  "catalog_version": "1.0.0",
  "categories": [{"id": "frontier", "label": "Frontier Labs", "description": "Foundation model labs"}],
  "providers": [{"id": "openai", "category": "frontier"}]
}
```

Other existing provider fields remain in the response; the example omits them.
Run `GRENGIN_CATEGORY_TESTS=1 bash tests/security/run.sh --live-only` for the
HTTP smoke on a disposable database after building the patched backend and
publishing the sidecar. No production database writes or provider calls are
needed by the discovery assertions themselves.
AI provider entries may also contain `category` directly in `index.json`.
It takes precedence over the sidecar mapping and remains available if the
sidecar is missing. Sidecar labels are accepted only when its mapping agrees
with the inline categories. Legacy indexes without this field remain supported.
Category definitions are data, not a hardcoded provider list. Membership must
match the selected distribution exactly. Categories cannot alter package pins,
credentials, model policy, or installation state. Auth discovery is unchanged.

Versioned packages remain immutable. Missing, invalid, timed-out,
or unavailable sidecars omit the optional category fields, not the providers.
Successes and failures are cached for five minutes with single-flight fetching.
The existing discovery response ETag covers the merged category fields too.

## Same-Version Container Hotfix

The initial sidecar hotfix did not change the 0.9.5 archives or original index. An explicitly
requested container hotfix can reuse that index through the manual Docker
Publish workflow's `compatibility_revision` input. Ordinary pushes still require
an exact index/source revision match. A hotfix must descend from the indexed
source; every indexed package is downloaded from the metadata origin, digest
checked, and parsed against the new runtime before image publication. Image
revision labels identify the patched commit, not the original index source.

```bash
gh workflow run docker-publish.yml --repo grengin-oss/grengin-api --ref main \
  -f compatibility_revision=2e574faf1ed7e6c8f08b6adcedcaffd4647511c8
```

This republishes `latest`, `0.9.5` and `v0.9.5` container tags with a new digest;
digest-pinned deployments stay unchanged. It does not publish a Git tag or
replace release archives. Existing source/archive consumers retain the original
behavior until they adopt a newer release.

## Approved 0.9.5 Index Cutover

The user subsequently requested categories inside the published 0.9.5 index.
This is a deliberate exception to index immutability, not a package update.
Publish and deploy the accepting parser before publishing the enriched index.
All running containers must first be upgraded: both the original release and
the earlier sidecar-only hotfix reject inline categories. Archived binaries
remain unchanged and cannot consume the enriched index. No older distribution
is modified. The index backend source pin advances to the accepting parser
commit; its catalog source, package versions and hashes remain unchanged.

The CI host build uses the development profile so workspace tests can reuse
it. Both delivered native container images still compile production binaries with
`--release`.
