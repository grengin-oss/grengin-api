// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Helpers for the projects / skills specs. UI helpers drive the webapp the way a user does; the
// API helpers only prepare state the UI cannot reach and read state back for assertions.
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { expect, test, type Browser, type Page } from '@playwright/test';
import { USERS } from '../mocks/users.mjs';
import { api, sql, tokenFor, waitFor } from './api';
import { env } from './env';
import { dismissOnboarding, signIn } from './login';

// mocks/service-projects-llm.mjs: reports the E2EP… markers it finds in the system prompt.
export const ECHO_ENGINE_KEY = 'e2e-projects-echo';
export const ECHO_MODEL_ID = 'e2e-projects-echo-chat';
const echoUrl = `http://127.0.0.1:${9450 + Number(process.env.E2E_PORT_OFFSET ?? 0)}`;

export const unique = (prefix: string) => `${prefix}-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
export const marker = () => `E2EP${Math.random().toString(36).slice(2, 10).toUpperCase()}`;
export const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

export async function userId(user: keyof typeof USERS): Promise<string | undefined> {
  const [row] = await sql<{ id: string }>(
    'SELECT id FROM users WHERE lower(email) = lower($1) ORDER BY "createdAt" LIMIT 1',
    [USERS[user].email],
  );
  return row?.id;
}

export async function tokenOf(user: keyof typeof USERS): Promise<string> {
  const id = await userId(user);
  if (!id) throw new Error(`${user} has not signed in yet`);
  return tokenFor(id, USERS[user].email);
}

// A second custom engine pointing at the echo mock. Its key sorts after the shared `e2e-mock`,
// so the shared engine stays the registry's first (default) model while this one exists.
export async function ensureEchoEngine() {
  await waitFor(`${echoUrl}/v1/models`, 30_000);
  const token = await tokenOf('owner');
  const engines = await api(token, 'GET', '/admin/ai-engines');
  if (JSON.stringify(engines.json ?? '').includes(`"${ECHO_ENGINE_KEY}"`)) return;
  const base = JSON.parse(await readFile(path.join(env.catalogDir, 'ai-providers/deepseek/plugin.json'), 'utf8'));
  const template = base.models[0];
  const manifest = {
    ...base,
    id: ECHO_ENGINE_KEY,
    name: 'E2E Projects Echo',
    description: 'Echoes the markers found in the system prompt (projects/skills specs).',
    baseUrl: `${echoUrl}/v1/`,
    models: [
      {
        ...template,
        id: ECHO_MODEL_ID,
        name: 'E2E Projects Echo Chat',
        capabilities: { chat: { ...template.capabilities.chat, reasoning: false } },
      },
    ],
  };
  const created = await api(token, 'POST', '/admin/ai-engines', {
    plugin_config: { manifest, configuration: {}, allowInsecureHttp: true, allowPrivateNetwork: true },
    api_key: 'e2e-key',
    is_enabled: true,
    whitelisted_models: [ECHO_MODEL_ID],
    default_model: ECHO_MODEL_ID,
  });
  if (created.status >= 300) throw new Error(`creating the echo engine failed: ${created.status} ${JSON.stringify(created.json)}`);
}

export async function removeEchoEngine() {
  const removed = await api(await tokenOf('owner'), 'DELETE', `/admin/ai-engines/${ECHO_ENGINE_KEY}`);
  if (removed.status >= 300 && removed.status !== 404) {
    throw new Error(`removing the echo engine failed: ${removed.status} ${JSON.stringify(removed.json)}`);
  }
}

export function composer(page: Page) {
  return page.getByRole('textbox', { name: 'Message input' });
}

export async function send(page: Page, text: string) {
  const box = composer(page);
  await box.fill(text);
  await box.press('Enter');
}

// The echo mock's answer to `question` (keep questions under 40 characters).
export function echoReply(page: Page, question: string) {
  return page.getByRole('log').getByText(`Echo ${question}: markers=`);
}

export async function chooseEchoModel(page: Page) {
  const chip = page.getByRole('button', { name: 'Select model' });
  await expect(chip).not.toHaveAttribute('aria-busy', 'true');
  await chip.click();
  const picker = page.getByRole('dialog', { name: 'Select model' });
  await picker.getByRole('textbox', { name: 'Search models…' }).fill('echo');
  await picker.getByRole('button', { name: /echo/i }).first().click();
  await expect(chip).toContainText(/echo/i);
  if (await picker.isVisible()) await page.keyboard.press('Escape');
}

export async function conversationIdFromUrl(page: Page): Promise<string> {
  await page.waitForURL(/[?&]chatId=[0-9a-f-]{36}/);
  return new URL(page.url()).searchParams.get('chatId')!;
}

export async function createProject(page: Page, name: string, description = ''): Promise<string> {
  await page.goto('/projects');
  await page.getByRole('main').getByRole('button', { name: 'New Project', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'New Project' });
  await dialog.getByLabel('Project name').fill(name);
  if (description) await dialog.getByLabel('Description').fill(description);
  await dialog.getByRole('button', { name: 'Create Project' }).click();
  await expect(page.getByText(`${name} created successfully`)).toBeVisible();
  await expect(dialog).toBeHidden();
  const [row] = await sql<{ id: string }>('SELECT id FROM projects WHERE name = $1', [name]);
  expect(row, `project ${name} stored`).toBeTruthy();
  return row.id;
}

export async function openProject(page: Page, id: string, name: string) {
  await page.goto(`/projects/${id}`);
  await expect(page.getByRole('heading', { level: 1, name })).toBeVisible();
}

export async function saveInstructions(page: Page, text: string) {
  await page.getByRole('tab', { name: 'Instructions' }).click();
  await page.getByPlaceholder(/Always respond in a formal tone/).fill(text);
  await page.getByRole('button', { name: 'Save Instructions' }).click();
  await expect(page.getByText('Instructions saved')).toBeVisible();
}

// The webapp cannot file a chat under a project (see projects.spec.ts); tests that need such a
// chat link it through the API the webapp calls, with the body the API documents.
export async function linkChatToProject(conversationId: string, projectId: string) {
  const linked = await api(await tokenOf('owner'), 'POST', `/conversations/${conversationId}/projects`, { project_id: projectId });
  if (linked.status >= 300 && linked.status !== 409) {
    throw new Error(`linking the chat failed: ${linked.status} ${JSON.stringify(linked.json)}`);
  }
}

// Registers a text file as a project source the way the API supports it: store the file
// (POST /files), then add it to the project (POST /projects/{id}/sources).
export async function registerTextSource(projectId: string, fileName: string, content: string) {
  const token = await tokenOf('owner');
  const bytes = Buffer.from(content, 'utf8');
  const file = await api(token, 'POST', '/files', {
    attachment: { file: bytes.toString('base64'), name: fileName, type: 'text/plain' },
  });
  if (file.status >= 300) throw new Error(`storing the file failed: ${file.status} ${JSON.stringify(file.json)}`);
  const source = await api(token, 'POST', `/projects/${projectId}/sources`, {
    file_name: fileName,
    file_type: 'text/plain',
    file_size: bytes.length,
    file_id: file.json.id,
  });
  if (source.status >= 300) throw new Error(`adding the source failed: ${source.status} ${JSON.stringify(source.json)}`);
  return source.json as { id: string; processing_status: string };
}

export async function openSkillsSettings(page: Page) {
  await page.goto('/settings?tab=skills');
  await expect(page.getByRole('button', { name: 'New Skill' })).toBeVisible();
}

export async function writeSkill(page: Page, skill: { name: string; description: string; instructions: string }) {
  await openSkillsSettings(page);
  await page.getByRole('button', { name: 'New Skill' }).click();
  await page.getByRole('menuitem', { name: /Write on the app/ }).click();
  const editor = page.getByRole('dialog', { name: 'Skill editor' });
  await editor.getByPlaceholder('e.g. Research Assistant').fill(skill.name);
  await editor.getByPlaceholder('A short summary of what this skill does').fill(skill.description);
  await editor.getByPlaceholder(/meticulous research assistant/).fill(skill.instructions);
  await editor.getByRole('button', { name: 'Create skill' }).click();
  await expect(page.getByText('Skill created')).toBeVisible();
  await expect(editor).toBeHidden();
  await expect(page.getByRole('article').filter({ hasText: skill.name })).toBeVisible();
}

export async function setComposerSkill(page: Page, name: string, on: boolean) {
  const chip = page.getByRole('button', { name: 'Select connectors' });
  await chip.click();
  const toggle = page.getByRole('button', { name, exact: true });
  await expect(toggle).toBeVisible();
  if ((await toggle.getAttribute('aria-pressed')) !== String(on)) await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', String(on));
  await chip.click();
  await expect(toggle).toBeHidden();
}

// Signs the member in from a separate browser context. Their first sign-in only creates the
// account, which then waits for an administrator's approval; approve it through the admin API and
// put the previous status back afterwards (other specs may rely on it).
export async function signInMember(browser: Browser) {
  const context = await browser.newContext({ baseURL: env.webUrl, storageState: { cookies: [], origins: [] } });
  const page = await context.newPage();
  if (!(await userId('member'))) await signIn(page, 'member');
  const id = await userId('member');
  if (!id) throw new Error('the member account was not created by signing in');
  const [{ status }] = await sql<{ status: string }>('SELECT status FROM users WHERE id = $1', [id]);
  const restore = async () => {
    await context.close();
    if (status !== 'active') await sql('UPDATE users SET status = $1 WHERE id = $2', [status, id]);
  };
  try {
    if (status !== 'active') {
      const approved = await api(await tokenOf('owner'), 'PATCH', `/admin/users/${id}/status`, { status: 'active' });
      if (approved.status >= 300) throw new Error(`approving the member failed: ${approved.status} ${JSON.stringify(approved.json)}`);
    }
    await signIn(page, 'member');
    await dismissOnboarding(page);
    await expect(composer(page)).toBeVisible();
  } catch (error) {
    await restore();
    throw error;
  }
  return { page, restore };
}

// Evidence for known bugs: records the API calls the page makes to matching paths (request body,
// status, response body) as test annotations and in the test's output.
export function recordApiCalls(page: Page, path: RegExp) {
  page.on('response', async (response) => {
    const request = response.request();
    const url = new URL(response.url());
    if (request.method() === 'GET' || !url.pathname.startsWith('/api/') || !path.test(url.pathname)) return;
    const info = test.info();
    const body = await response.text().catch(() => '');
    const description = `${request.method()} ${url.pathname} ${(request.postData() ?? '').slice(0, 200)} -> ${response.status()} ${body.slice(0, 300)}`;
    info.annotations.push({ type: 'api call', description });
    console.log(`[api call] ${description}`);
  });
}
