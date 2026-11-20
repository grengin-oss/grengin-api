// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Helpers for the chat specs (specs/chat/*). Selectors come from webapp `main`: Chat.svelte,
// ChatMessage.svelte, MessageInput.svelte, SidebarChatSection.svelte, SidebarSearchModal.svelte.
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { expect, type Locator, type Page } from '@playwright/test';
import { USERS } from '../mocks/users.mjs';
import { api, sql, tokenFor } from './api';
import { env } from './env';
import { dismissOnboarding } from './login';

// The suite shares one database with other areas: every text a chat spec sends carries a
// random suffix, and assertions only look at chats the spec created.
export const unique = (label: string) => `${label}-${Math.random().toString(36).slice(2, 10)}`;
// Chat search splits queries on anything but letters, so a searchable word is letters only.
export const word = (prefix: string) =>
  prefix + Array.from({ length: 10 }, () => String.fromCharCode(97 + Math.floor(Math.random() * 26))).join('');

export async function ownerToken() {
  const [owner] = await sql<{ id: string }>(
    'SELECT id FROM users WHERE lower(email) = lower($1) ORDER BY "createdAt" LIMIT 1',
    [USERS.owner.email],
  );
  return tokenFor(owner.id, USERS.owner.email);
}

export async function openNewChat(page: Page) {
  await page.goto('/');
  await composer(page).waitFor();
  await dismissOnboarding(page);
}

export const composer = (page: Page) => page.getByRole('textbox', { name: 'Message input' });
export const transcript = (page: Page) => page.getByRole('log');
export const messages = (page: Page) => transcript(page).locator('.message');
export const userMessage = (page: Page, text: string | RegExp) => transcript(page).locator('.message.user').filter({ hasText: text });
export const assistantMessage = (page: Page, text: string | RegExp) =>
  transcript(page).locator('.message.assistant').filter({ hasText: text });
export const versions = (page: Page) => transcript(page).getByRole('group', { name: 'Message versions' });

export async function send(page: Page, text: string) {
  const input = composer(page);
  await input.fill(text);
  await input.press('Enter');
}

// The conversation id the webapp puts in the URL once the first message created it.
export async function currentChatId(page: Page): Promise<string> {
  let id: string | null = null;
  await expect
    .poll(() => {
      id = new URL(page.url()).searchParams.get('chatId');
      return id;
    })
    .toBeTruthy();
  return id as unknown as string;
}

export async function conversation(chatId: string) {
  const response = await api(await ownerToken(), 'GET', `/chat/${chatId}`);
  return response;
}

// A sidebar row: the chat button (named by its title) plus its "Chat options" menu.
export const sidebarChat = (page: Page, title: string) =>
  page.locator('.chat-item').filter({ has: page.getByRole('button', { name: title, exact: true }) });

export async function openChatOptions(page: Page, title: string): Promise<Locator> {
  const row = sidebarChat(page, title);
  // The options button only takes pointer events while its row is hovered.
  await row.hover();
  await row.getByRole('button', { name: 'Chat options' }).click();
  return row.getByRole('menu');
}

// A second AI engine backed by mocks/service-chat-llm.mjs, registered through the admin API like
// support/global-setup.ts registers the main mock. It is enabled only while the specs that use it
// run, so it never becomes the default model of other specs' new chats.
export const CONTEXT_ENGINE = 'e2e-chat-context';
export const CONTEXT_MODEL = 'e2e-context';
export const CONTEXT_MODEL_NAME = 'E2E Context Mock';

export async function enableContextEngine() {
  const token = await ownerToken();
  const engines = await api(token, 'GET', '/admin/ai-engines');
  if (JSON.stringify(engines.json ?? '').includes(`"${CONTEXT_ENGINE}"`)) {
    const updated = await api(token, 'PUT', `/admin/ai-engines/${CONTEXT_ENGINE}`, { is_enabled: true });
    if (updated.status >= 300) throw new Error(`enabling ${CONTEXT_ENGINE} failed: ${updated.status} ${JSON.stringify(updated.json)}`);
    return;
  }
  const base = JSON.parse(await readFile(path.join(env.catalogDir, 'ai-providers/deepseek/plugin.json'), 'utf8'));
  const template = base.models[0];
  const port = 9420 + Number(process.env.E2E_PORT_OFFSET ?? 0);
  const manifest = {
    ...base,
    id: CONTEXT_ENGINE,
    name: 'E2E Context LLM',
    description: 'Mock model that answers with the context it received (chat specs).',
    baseUrl: `http://127.0.0.1:${port}/v1/`,
    models: [
      { ...template, id: CONTEXT_MODEL, name: CONTEXT_MODEL_NAME, capabilities: { chat: { ...template.capabilities.chat, reasoning: false } } },
    ],
  };
  const created = await api(token, 'POST', '/admin/ai-engines', {
    plugin_config: { manifest, configuration: {}, allowInsecureHttp: true, allowPrivateNetwork: true },
    api_key: 'e2e-key',
    is_enabled: true,
    whitelisted_models: [CONTEXT_MODEL],
    default_model: CONTEXT_MODEL,
  });
  if (created.status >= 300) throw new Error(`creating ${CONTEXT_ENGINE} failed: ${created.status} ${JSON.stringify(created.json)}`);
}

export async function disableContextEngine() {
  await api(await ownerToken(), 'PUT', `/admin/ai-engines/${CONTEXT_ENGINE}`, { is_enabled: false });
}

// Picks a model through the composer's model picker.
export async function selectModel(page: Page, name: string, key: string) {
  await page.getByRole('button', { name: 'Select model' }).click();
  const picker = page.getByRole('dialog', { name: 'Select model' });
  await picker.getByRole('textbox', { name: 'Search models…' }).fill(name);
  await picker.getByRole('button', { name: new RegExp(name) }).first().click();
  await expect(picker).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Select model' })).toContainText(key);
}
