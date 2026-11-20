// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// MCP-area helpers: setup through the admin API where the webapp has no control for it yet,
// assertions against the mock MCP server (mocks/service-mcp.mjs), and the UI steps the specs share.
import { createHash, randomBytes } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { expect, type Browser, type Page } from '@playwright/test';
import { USERS } from '../mocks/users.mjs';
import { api, sql, tokenFor } from './api';
import { env } from './env';
import { dismissOnboarding, signIn } from './login';

export const MCP_ENDPOINT = `${env.mcpUrl}/mcp`;

export type DefaultAccess = 'all_users' | 'admin_only' | 'explicit_only';

export interface McpCall {
  name: string;
  arguments: Record<string, unknown>;
}

export const uniqueName = (label: string) => `E2E MCP ${label} ${randomBytes(3).toString('hex')}`;

export async function userId(user: keyof typeof USERS): Promise<string | null> {
  const [row] = await sql<{ id: string }>(
    'SELECT id FROM users WHERE lower(email) = lower($1) ORDER BY "createdAt" LIMIT 1',
    [USERS[user].email],
  );
  return row?.id ?? null;
}

export async function tokenOf(user: keyof typeof USERS): Promise<string> {
  const id = await userId(user);
  if (!id) throw new Error(`${USERS[user].email} has not signed in yet`);
  return tokenFor(id, USERS[user].email);
}

async function ok<T = any>(response: Promise<{ status: number; json: any }>, what: string): Promise<T> {
  const { status, json } = await response;
  if (status >= 300) throw new Error(`${what} failed: ${status} ${JSON.stringify(json)}`);
  return json as T;
}

// The webapp's Add Server form cannot create a server (see mcp-servers.spec.ts), so the other
// specs add the mock server the way the form would if it sent a valid default_access.
export async function createMcpServer(name: string, defaultAccess: DefaultAccess = 'all_users') {
  const owner = await tokenOf('owner');
  const server = await ok<{ id: string; name: string }>(
    api(owner, 'POST', '/admin/mcp-servers', {
      name,
      description: 'Mock remote MCP server for end-to-end tests',
      transport_type: 'http',
      url: MCP_ENDPOINT,
      connection_config: {},
      enabled: true,
      default_access: defaultAccess,
    }),
    'creating the MCP server',
  );
  return server;
}

// Webapp main has no Sync Tools control (see mcp-servers.spec.ts); this is the API it would call.
export async function syncTools(serverId: string) {
  const owner = await tokenOf('owner');
  const result = await ok<{ success: boolean; total_tools?: number }>(
    api(owner, 'POST', `/admin/mcp-servers/${serverId}/sync-tools`),
    'syncing MCP tools',
  );
  expect(result.success, JSON.stringify(result)).toBe(true);
  return result;
}

export async function deleteMcpServer(serverId: string | undefined) {
  if (!serverId) return;
  const owner = await tokenOf('owner');
  await api(owner, 'DELETE', `/admin/mcp-servers/${serverId}`);
}

export async function serverTools(serverId: string) {
  return sql<{ id: string; name: string; description: string | null; enabled: boolean }>(
    'SELECT id, name, description, enabled FROM mcp_tools WHERE "serverId" = $1 ORDER BY name',
    [serverId],
  );
}

// Chat names MCP tools mcp__<server prefix>__<tool>__<hash>; the mock LLM calls the offered tool
// whose name ends with the [tool:...] marker, so the marker carries the unique "<tool>__<hash>".
export function toolMarker(serverId: string, tool: string): string {
  const hash = createHash('sha256').update(`${serverId}:${tool}`).digest('hex').slice(0, 8);
  return `${tool}__${hash}`;
}

export async function mcpCalls(): Promise<{ count: number; held: number; calls: McpCall[] }> {
  const response = await fetch(`${env.mcpUrl}/calls`);
  return response.json();
}

export async function resetMcpCalls() {
  await fetch(`${env.mcpUrl}/calls/reset`, { method: 'POST' });
}

export async function releaseHeldCall() {
  await fetch(`${env.mcpUrl}/hold/release`, { method: 'POST' });
}

// Setup shortcuts for tests whose subject is not the access UI itself (access.spec.ts covers it).
export async function setDefaultAccessViaApi(serverId: string, defaultAccess: DefaultAccess) {
  await ok(
    api(await tokenOf('owner'), 'PUT', `/admin/mcp-servers/${serverId}/access/default`, { default_access: defaultAccess }),
    'setting default access',
  );
}

export async function grantUserViaApi(serverId: string, user: keyof typeof USERS) {
  await ok(
    api(await tokenOf('owner'), 'POST', `/admin/mcp-servers/${serverId}/access/rules`, {
      access_type: 'user',
      user_id: await userId(user),
      permission: 'full',
    }),
    'granting access',
  );
}

export async function clearRulesViaApi(serverId: string) {
  const owner = await tokenOf('owner');
  const access = await ok<{ rules: { id: string | null }[] }>(
    api(owner, 'GET', `/admin/mcp-servers/${serverId}/access`),
    'reading access rules',
  );
  for (const rule of access.rules) {
    if (rule.id) await ok(api(owner, 'DELETE', `/admin/mcp-servers/${serverId}/access/rules/${rule.id}`), 'removing a rule');
  }
}

export async function visibleServerNames(user: keyof typeof USERS): Promise<string[]> {
  const response = await ok<{ servers: { name: string }[] }>(api(await tokenOf(user), 'GET', '/mcp-servers'), 'listing MCP servers');
  return response.servers.map((server) => server.name);
}

// A second AI engine whose model calls an MCP tool twice in one turn (served by service-mcp.mjs),
// so a test can change access between the two dispatches. Created per test and removed after.
export const GATED_ENGINE = 'e2e-mcp-gated';
export const GATED_MODEL = 'e2e-mcp-gated-chat';
export const GATED_MODEL_NAME = 'E2E Gated MCP';

export async function createGatedEngine() {
  const owner = await tokenOf('owner');
  await api(owner, 'DELETE', `/admin/ai-engines/${GATED_ENGINE}`);
  const base = JSON.parse(await readFile(path.join(env.catalogDir, 'ai-providers/deepseek/plugin.json'), 'utf8'));
  const template = base.models[0];
  const manifest = {
    ...base,
    id: GATED_ENGINE,
    name: 'E2E Gated MCP LLM',
    description: 'Mock model that calls one MCP tool twice in a turn.',
    baseUrl: `${env.mcpUrl}/v1/`,
    models: [{ ...template, id: GATED_MODEL, name: GATED_MODEL_NAME, capabilities: { chat: { ...template.capabilities.chat, reasoning: false } } }],
  };
  await ok(
    api(owner, 'POST', '/admin/ai-engines', {
      plugin_config: { manifest, configuration: {}, allowInsecureHttp: true, allowPrivateNetwork: true },
      api_key: 'e2e-key',
      is_enabled: true,
      whitelisted_models: [GATED_MODEL],
      default_model: GATED_MODEL,
    }),
    'creating the gated AI engine',
  );
}

export async function deleteGatedEngine() {
  await api(await tokenOf('owner'), 'DELETE', `/admin/ai-engines/${GATED_ENGINE}`);
}

// Accounts created by IdP sign-in (JIT) wait for admin approval. The access specs need a usable
// member, so they approve it and afterwards put back whatever status it had (see restoreStatus).
export async function ensureActive(browser: Browser, user: keyof typeof USERS): Promise<string> {
  if (!(await userId(user))) {
    const context = await browser.newContext({ storageState: { cookies: [], origins: [] }, baseURL: env.webUrl });
    await signIn(await context.newPage(), user);
    await context.close();
    await expect.poll(() => userId(user)).not.toBeNull();
  }
  const id = (await userId(user))!;
  const [{ status }] = await sql<{ status: string }>('SELECT status FROM users WHERE id = $1', [id]);
  if (status !== 'active') {
    await ok(api(await tokenOf('owner'), 'PATCH', `/admin/users/${id}/status`, { status: 'active' }), `approving ${user}`);
  }
  return status;
}

// The status API cannot set "pending" (it is only ever set by sign-in), so this restores it directly.
export async function restoreStatus(user: keyof typeof USERS, status: string | undefined) {
  const id = await userId(user);
  if (id && status && status !== 'active') await sql('UPDATE users SET status = $1 WHERE id = $2', [status, id]);
}

// ---- UI ----

export async function openChat(page: Page) {
  await page.goto('/');
  await page.getByRole('textbox', { name: 'Message input' }).waitFor();
  await dismissOnboarding(page);
}

// A fresh browser context signed in as `user` (the member has no stored session).
export async function signedInPage(browser: Browser, user: keyof typeof USERS) {
  const context = await browser.newContext({ storageState: { cookies: [], origins: [] }, baseURL: env.webUrl });
  const page = await context.newPage();
  await signIn(page, user);
  await page.getByRole('textbox', { name: 'Message input' }).waitFor();
  await dismissOnboarding(page);
  return { context, page };
}

export async function sendMessage(page: Page, text: string) {
  const composer = page.getByRole('textbox', { name: 'Message input' });
  await composer.fill(text);
  await composer.press('Enter');
}

export async function connectorToggle(page: Page, serverName: string) {
  const menu = page.getByRole('button', { name: 'Select connectors' });
  if ((await menu.getAttribute('aria-expanded')) !== 'true') await menu.click();
  await expect(page.getByText('Loading connectors...')).toHaveCount(0);
  return page.getByRole('button', { name: serverName, exact: true });
}

export async function closeComposerMenus(page: Page) {
  await page.keyboard.press('Escape');
  await expect(page.getByRole('button', { name: 'Select connectors' })).toHaveAttribute('aria-expanded', 'false');
}

// Turns the connector on in the composer's Tools menu (connected ones start on).
export async function enableConnector(page: Page, serverName: string) {
  const toggle = await connectorToggle(page, serverName);
  await expect(toggle).toBeVisible();
  if ((await toggle.getAttribute('aria-pressed')) !== 'true') await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');
  await closeComposerMenus(page);
}

// The picker lists the engine's live model listing (/v1/models), so it shows the model id.
export async function selectModel(page: Page, modelId: string) {
  await page.getByRole('button', { name: 'Select model' }).click();
  const picker = page.getByRole('dialog', { name: 'Select model' });
  await picker.getByRole('textbox', { name: 'Search models…' }).fill(modelId);
  await picker.getByRole('button', { name: new RegExp(modelId) }).first().click();
  await expect(page.getByRole('button', { name: 'Select model' })).toContainText(modelId);
}

// Admin → Connectors, list view, then the server's connector page.
export async function openConnector(page: Page, serverName: string) {
  await page.goto('/admin/mcp-servers');
  await page.getByRole('textbox', { name: 'Search servers…' }).fill(serverName);
  await page.getByRole('button', { name: 'List', exact: true }).click();
  await page.getByRole('button', { name: serverName, exact: true }).click();
  await expect(page.getByRole('heading', { level: 2, name: serverName })).toBeVisible();
}

export async function openAdvanced(page: Page, tab: 'Access Control' | 'Tool Access') {
  await page.getByRole('button', { name: /^Advanced/ }).click();
  await page.getByRole('tab', { name: tab }).click();
  await expect(page.getByRole('tab', { name: tab })).toHaveAttribute('aria-selected', 'true');
}

const DEFAULT_ACCESS_LABEL: Record<DefaultAccess, string> = {
  all_users: 'All Users',
  admin_only: 'Admin Only',
  explicit_only: 'Explicit Only',
};

export async function setDefaultAccess(page: Page, access: DefaultAccess) {
  const label = DEFAULT_ACCESS_LABEL[access];
  const radio = page.getByRole('radio', { name: new RegExp(`^${label}`) });
  if (await radio.isChecked()) return;
  // The radio input is visually hidden; its card (the label) is what an admin clicks.
  await page.locator('label').filter({ has: radio }).click();
  await expect(radio).toBeChecked();
  await expect(page.getByText('Default access updated').first()).toBeVisible();
}

export async function addUserRule(page: Page, email: string) {
  await page.getByRole('button', { name: 'Add Rules' }).click();
  const dialog = page.getByRole('dialog', { name: 'Add Rule' });
  await dialog.getByRole('button', { name: 'User', exact: true }).click();
  await dialog.getByPlaceholder('Search by name or email...').fill(email);
  await dialog.getByRole('button', { name: new RegExp(email.replace(/[.]/g, '\\.')) }).click();
  await expect(dialog.getByRole('radio', { name: /^Full Access/ })).toBeChecked();
  await dialog.getByRole('button', { name: 'Add Rule', exact: true }).click();
  await expect(page.getByText('Access rule added').first()).toBeVisible();
  await expect(dialog).toBeHidden();
}

// The specs' servers carry exactly one rule (the member's), so its Remove button is unique.
export async function removeOnlyRule(page: Page, email: string) {
  await expect(page.getByText(email).first()).toBeVisible();
  await page.getByRole('button', { name: 'Remove rule' }).click();
  await expect(page.getByText('Access rule removed').first()).toBeVisible();
  await expect(page.getByText('No access rules configured')).toBeVisible();
}
