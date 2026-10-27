import assert from 'node:assert/strict';

const base = process.env.GRENGIN_SECURITY_TEST_URL ?? 'http://localhost:8080';
const metadata = process.env.GRENGIN_METADATA_BASE_URL ?? 'https://meta.grengin.com';
const first = await fetch(`${base}/discovery/ai-providers`);
assert.equal(first.status, 200);
const listing = await first.json();
const sidecarResponse = await fetch(`${metadata}/distributions/grengin-api/${listing.distribution_version}/categories.json`);
assert.equal(sidecarResponse.status, 200);
const sidecar = await sidecarResponse.json();
assert.deepEqual(listing.categories, sidecar.categories);
assert.equal(listing.providers.length, Object.keys(sidecar.providers).length);
for (const provider of listing.providers) {
  assert.equal(provider.category, sidecar.providers[provider.id]);
  assert.ok(listing.categories.some(category => category.id === provider.category));
}
console.log(`PASS: ${listing.providers.length} providers have matching categories over HTTP`);

for (const suffix of ['?version=1', '?version=1.0']) {
  const response = await fetch(`${base}/discovery/ai-providers${suffix}`);
  assert.equal(response.status, 200);
  const result = await response.json();
  for (const provider of result.providers) {
    assert.equal(provider.category, sidecar.providers[provider.id]);
  }
}
console.log('PASS: version selectors retain categories');

const etag = first.headers.get('etag');
assert.ok(etag);
const unchanged = await fetch(`${base}/discovery/ai-providers`, {headers: {'If-None-Match': etag}});
assert.equal(unchanged.status, 304);
console.log('PASS: category-enriched response supports conditional GET');

const auth = await fetch(`${base}/discovery/auth-providers`);
assert.equal(auth.status, 200);
const authListing = await auth.json();
assert.equal(authListing.categories, undefined);
assert.ok(authListing.providers.every(provider => provider.category === undefined));
console.log('PASS: auth discovery unchanged');

const invalid = await fetch(`${base}/discovery/ai-providers?version=invalid`);
assert.equal(invalid.status, 400);
console.log('PASS: invalid version still rejected');
