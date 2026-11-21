// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import assert from 'node:assert/strict';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

const worker = resolve(process.env.GRENGIN_WEBSITE_DIR ?? '../grengin-website', 'workers');
const require = createRequire(join(worker, 'package.json'));
const wrangler = require.resolve('wrangler');
const dependency = name => import(pathToFileURL(require.resolve(name, { paths: [wrangler] })));
const { build } = await dependency('esbuild');
const { Miniflare, convertV4MiniflareOptions } = await dependency('miniflare');
const config = await readFile(join(worker, 'wrangler.toml'), 'utf8');
const bundle = await build({
  stdin: { resolveDir: worker, contents:
    'import {handleRefreshConnection} from "./src/handlers/connection.ts"; export default {fetch: handleRefreshConnection};' },
  bundle: true, format: 'esm', platform: 'browser', target: 'es2022', write: false,
});
let input = '';
for await (const chunk of process.stdin) input += chunk;
const { proof, publicKey, mlDsaPublicKey } = JSON.parse(input);
const scratch = await mkdtemp(join(tmpdir(), 'grengin-refresh-'));
process.chdir(scratch);
process.env.WRANGLER_LOG_PATH = join(scratch, 'logs');
const runtime = new Miniflare(convertV4MiniflareOptions({
  modules: true, script: bundle.outputFiles[0].text,
  compatibilityDate: config.match(/^compatibility_date\s*=\s*"([^"]+)"/m)[1],
  compatibilityFlags: JSON.parse(config.match(/^compatibility_flags\s*=\s*(\[[^\n]+\])/m)[1]),
  bindings: { INSTANCE_CONNECTION_SECRET: crypto.randomUUID() },
  rootPath: scratch, cf: false, d1Databases: ['DB'], kvNamespaces: ['INSTALLATIONS'],
  outboundService: () => assert.fail('refresh must stay local'),
}));
try {
  const db = await runtime.getD1Database('DB');
  await db.prepare('CREATE TABLE installations (id, instance_uuid, organization_id, identity_public_key, ml_dsa_public_key, identity_key_version, connection_status, is_revoked, revoked_at)').run();
  await db.prepare('CREATE TABLE connection_proofs (proof_key TEXT PRIMARY KEY, instance_uuid, operation, expires_at, created_at)').run();
  await db.prepare("INSERT INTO installations VALUES ('installation', ?, 'org', ?, ?, 1, 'connected', 0, NULL)")
    .bind(proof.instance_uuid, publicKey, mlDsaPublicKey).run();
  const refresh = changes => runtime.dispatchFetch('https://workspace.test/v1/connect/refresh', {
    method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ ...proof, ...changes }),
  });
  assert.equal((await refresh({ signature: '0'.repeat(128) })).status, 401);
  assert.equal((await refresh({ ml_dsa_signature: '0'.repeat(6618) })).status, 401);
  const accepted = await refresh({});
  assert.equal(accepted.status, 200);
  assert.equal(typeof (await accepted.json()).connection_grant, 'string');
  assert.equal((await refresh({})).status, 409);
} finally {
  await runtime.dispose();
  process.chdir(tmpdir());
  await rm(scratch, { recursive: true, force: true });
}
