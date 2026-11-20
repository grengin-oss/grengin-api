// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Setup and restore helpers for the admin specs. Tests drive the product through the UI; these
// only prepare state the UI under test does not create, read state back, and put shared state
// (the sign-in provider, departments) back the way the rest of the suite expects it.
import { expect, type Page } from '@playwright/test';
import { USERS } from '../mocks/users.mjs';
import { api, sql, tokenFor } from './api';
import { env } from './env';
import { IDP_NAME, IDP_SLUG } from './login';

export { IDP_NAME, IDP_SLUG };

export function unique(prefix: string): string {
  return `${prefix} ${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
}

export async function ownerId(): Promise<string> {
  const [row] = await sql<{ id: string }>('SELECT id FROM users WHERE lower(email) = lower($1) ORDER BY "createdAt" LIMIT 1', [
    USERS.owner.email,
  ]);
  if (!row) throw new Error('owner not found');
  return row.id;
}

export async function ownerToken(): Promise<string> {
  return tokenFor(await ownerId(), USERS.owner.email);
}

async function ok<T = any>(promise: Promise<{ status: number; json: any }>, what: string): Promise<T> {
  const response = await promise;
  if (response.status >= 300) throw new Error(`${what} failed: ${response.status} ${JSON.stringify(response.json)}`);
  return response.json as T;
}

export async function createDepartment(name: string, parentId: string | null = null): Promise<{ id: string; name: string }> {
  const token = await ownerToken();
  return ok(api(token, 'POST', '/admin/departments', { name, description: 'E2E admin spec', parent_id: parentId }), `creating ${name}`);
}

export async function setDepartmentBudget(id: string, amount: number) {
  const token = await ownerToken();
  await ok(
    api(token, 'PUT', `/admin/departments/${id}`, { budget_allocated: amount, budget_period: 'monthly', action_on_exceed: 'warn' }),
    `setting the budget of ${id}`,
  );
}

export async function deleteDepartment(id: string) {
  const token = await ownerToken();
  const response = await api(token, 'DELETE', `/admin/departments/${id}`);
  if (response.status >= 300 && response.status !== 404) {
    throw new Error(`deleting department ${id} failed: ${response.status} ${JSON.stringify(response.json)}`);
  }
}

type ProviderRow = { id: string; provider: string; isEnabled: boolean };

export async function providers(): Promise<ProviderRow[]> {
  return sql<ProviderRow>('SELECT id, provider, "isEnabled" FROM sso_providers ORDER BY provider');
}

// A custom OIDC provider pointing at the mock IdP, created disabled (as POST /admin/sso-providers
// always does). The admin UI has no "add provider" flow, so this is setup.
export async function createOidcProvider(slug: string, name: string): Promise<string> {
  const token = await ownerToken();
  const created = await ok(
    api(token, 'POST', '/admin/sso-providers', {
      provider: slug,
      name,
      client_id: 'grengin-e2e',
      client_secret: 'e2e-client-secret',
      issuer_url: env.oidcUrl,
      redirect_url: `${env.webUrl}/auth/${slug}/callback`,
      allowed_domains: [],
      jit_provisioning: true,
    }),
    `creating provider ${slug}`,
  );
  return created.id;
}

export async function deleteProvider(id: string) {
  const [row] = await sql<ProviderRow>('SELECT id, provider, "isEnabled" FROM sso_providers WHERE id = $1', [id]);
  if (!row) return;
  const token = await ownerToken();
  if (row.isEnabled) await ok(api(token, 'PUT', `/admin/sso-providers/${id}`, { is_enabled: false }), `disabling ${row.provider}`);
  await ok(api(token, 'DELETE', `/admin/sso-providers/${id}`), `deleting ${row.provider}`);
}

// Puts the suite's sign-in provider back: present and enabled. Every other spec signs in with it.
export async function ensureE2eIdpEnabled() {
  const token = await ownerToken();
  let [row] = (await providers()).filter((p) => p.provider === IDP_SLUG);
  if (!row) {
    const id = await createOidcProvider(IDP_SLUG, IDP_NAME);
    row = { id, provider: IDP_SLUG, isEnabled: false };
  }
  if (row.isEnabled) return;
  const validated = await ok(api(token, 'POST', `/admin/sso-providers/${row.id}/validate`, {}), 'validating e2e-idp');
  if (!validated?.valid || !validated?.validation_token) throw new Error(`validating e2e-idp failed: ${JSON.stringify(validated)}`);
  await ok(api(token, 'PUT', `/admin/sso-providers/${row.id}`, { is_enabled: true, validation_token: validated.validation_token }), 'enabling e2e-idp');
}

// Organization → Teams tree → the department's details panel.
export async function openDepartment(page: Page, name: string) {
  await page.goto('/admin/departments');
  const find = page.getByRole('textbox', { name: 'Find a department...' });
  await find.fill(name);
  const node = page.getByRole('treeitem', { name: new RegExp(`^${escapeRegExp(name)},`) });
  await node.click();
  await expect(node).toHaveAttribute('aria-selected', 'true');
  return page.getByRole('region', { name: 'Details' });
}

export function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
