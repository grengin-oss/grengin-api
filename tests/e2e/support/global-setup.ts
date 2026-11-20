// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Prepares what a fresh installation needs before anyone can sign in through the UI:
// the mock IdP as a sign-in provider and a mock LLM as an AI engine. Both are created through
// the real admin API with a short-lived token for the bootstrapped owner (BOOTSTRAP_SUPER_ADMIN_EMAIL).
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import path from 'node:path';
import { USERS } from '../mocks/users.mjs';
import { api, sql, tokenFor, waitFor } from './api';
import { env } from './env';

export const IDP_SLUG = 'e2e-idp';
export const IDP_NAME = 'E2E IdP';
export const ENGINE_KEY = 'e2e-mock';
export const MODEL_ID = 'e2e-chat';

export default async function globalSetup() {
  await waitFor(`${env.apiUrl}/`);
  await waitFor(`${env.webUrl}/`);
  await waitFor(`${env.oidcUrl}/.well-known/openid-configuration`);

  const [owner] = await sql<{ id: string }>(
    'SELECT id FROM users WHERE lower(email) = lower($1) ORDER BY "createdAt" LIMIT 1',
    [USERS.owner.email],
  );
  if (!owner) throw new Error(`bootstrapped owner ${USERS.owner.email} not found (BOOTSTRAP_SUPER_ADMIN_EMAIL)`);
  const token = tokenFor(owner.id, USERS.owner.email);

  const providers = await api(token, 'GET', '/admin/sso-providers');
  const existing = JSON.stringify(providers.json ?? '').includes(`"${IDP_SLUG}"`);
  if (!existing) {
    const created = await api(token, 'POST', '/admin/sso-providers', {
      provider: IDP_SLUG,
      name: IDP_NAME,
      client_id: 'grengin-e2e',
      client_secret: 'e2e-client-secret',
      issuer_url: env.oidcUrl,
      redirect_url: `${env.webUrl}/auth/${IDP_SLUG}/callback`,
      allowed_domains: [],
      jit_provisioning: true,
    });
    if (created.status >= 300) throw new Error(`creating the E2E IdP failed: ${created.status} ${JSON.stringify(created.json)}`);
    const id = created.json?.id;
    if (id && created.json?.is_enabled === false) {
      // Enabling requires a successful validation of the stored configuration, as in the admin UI.
      const validated = await api(token, 'POST', `/admin/sso-providers/${id}/validate`, {});
      if (validated.status >= 300 || !validated.json?.valid || !validated.json?.validation_token) {
        throw new Error(`validating the E2E IdP failed: ${validated.status} ${JSON.stringify(validated.json)}`);
      }
      const enabled = await api(token, 'PUT', `/admin/sso-providers/${id}`, {
        is_enabled: true,
        validation_token: validated.json.validation_token,
      });
      if (enabled.status >= 300) throw new Error(`enabling the E2E IdP failed: ${enabled.status} ${JSON.stringify(enabled.json)}`);
    }
  }

  const engines = await api(token, 'GET', '/admin/ai-engines');
  if (!JSON.stringify(engines.json ?? '').includes(`"${ENGINE_KEY}"`)) {
    const base = JSON.parse(await readFile(path.join(env.catalogDir, 'ai-providers/deepseek/plugin.json'), 'utf8'));
    const template = base.models[0];
    const manifest = {
      ...base,
      id: ENGINE_KEY,
      name: 'E2E Mock LLM',
      description: 'Deterministic mock model for end-to-end tests.',
      baseUrl: `${env.llmUrl}/v1/`,
      models: [{ ...template, id: MODEL_ID, name: 'E2E Chat', capabilities: { chat: { ...template.capabilities.chat, reasoning: false } } }],
    };
    const created = await api(token, 'POST', '/admin/ai-engines', {
      plugin_config: { manifest, configuration: {}, allowInsecureHttp: true, allowPrivateNetwork: true },
      api_key: 'e2e-key',
      is_enabled: true,
      whitelisted_models: [MODEL_ID],
      default_model: MODEL_ID,
    });
    if (created.status >= 300) throw new Error(`creating the mock AI engine failed: ${created.status} ${JSON.stringify(created.json)}`);
  }

  await mkdir('state', { recursive: true });
  await writeFile('state/setup.json', JSON.stringify({ ownerId: owner.id }, null, 2));
}
