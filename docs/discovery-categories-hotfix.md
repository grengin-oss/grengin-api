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
Category definitions are data, not a hardcoded provider list. Membership must
match the selected distribution exactly. Categories cannot alter package pins,
credentials, model policy, or installation state. Auth discovery is unchanged.

The index and versioned packages remain immutable. Missing, invalid, timed-out,
or unavailable sidecars omit the optional category fields, not the providers.
Successes and failures are cached for five minutes with single-flight fetching.
The existing discovery response ETag covers the merged category fields too.

## Same-Version Container Hotfix

The 0.9.5 release archives and original index are not changed. An explicitly
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
