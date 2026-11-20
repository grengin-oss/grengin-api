// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Shared pieces of the AI engines specs: a second plugin engine ("E2E Picker LLM", served by
// mocks/service-engines-llm.mjs) with one web-search model and one without, plus page helpers for
// the admin AI Engines page and the chat model picker. The engine is set up and removed through the
// admin API; the specs drive everything else through the UI.
import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { expect, type Locator, type Page } from '@playwright/test';
import { USERS } from '../mocks/users.mjs';
import { api, sql, tokenFor } from './api';
import { env } from './env';
import { dismissOnboarding } from './login';

export const MOCK_ENGINE = { key: 'e2e-mock', name: 'E2E Mock LLM', model: 'e2e-chat', modelName: 'E2E Chat' };
// The key sorts after e2e-mock, so while it exists e2e-chat stays the first model a new chat opens on.
export const PICKER_ENGINE = { key: 'e2e-picker', name: 'E2E Picker LLM' };
export const SEARCH_MODEL = { id: 'e2e-search', name: 'E2E Search' };
export const PLAIN_MODEL = { id: 'e2e-plain', name: 'E2E Plain' };
export const PICKER_LLM_URL = `http://127.0.0.1:${9410 + Number(process.env.E2E_PORT_OFFSET ?? 0)}`;

export async function ownerToken(): Promise<string> {
  const [owner] = await sql<{ id: string }>(
    'SELECT id FROM users WHERE lower(email) = lower($1) ORDER BY "createdAt" LIMIT 1',
    [USERS.owner.email],
  );
  return tokenFor(owner.id, USERS.owner.email);
}

// A plugin manifest built from the catalog's DeepSeek manifest (as global-setup builds e2e-mock),
// pointed at the given base URL.
export async function mockManifest(id: string, name: string, baseUrl: string) {
  const base = JSON.parse(await readFile(path.join(env.catalogDir, 'ai-providers/deepseek/plugin.json'), 'utf8'));
  const template = base.models[0];
  const model = (modelId: string, modelName: string, supportsWebSearch: boolean) => ({
    ...template,
    id: modelId,
    name: modelName,
    capabilities: { chat: { ...template.capabilities.chat, reasoning: false } },
    metadata: { ...template.metadata, comment: `E2E model ${modelId}`, supportsWebSearch },
  });
  return {
    ...base,
    id,
    name,
    description: 'Second deterministic mock provider for the AI engines specs.',
    baseUrl,
    models: [model(SEARCH_MODEL.id, SEARCH_MODEL.name, true), model(PLAIN_MODEL.id, PLAIN_MODEL.name, false)],
  };
}

// Creates the picker engine, or puts it back to its starting state: enabled, both models
// whitelisted, e2e-search as its default model.
export async function resetPickerEngine() {
  const token = await ownerToken();
  const state = {
    is_enabled: true,
    whitelisted_models: [SEARCH_MODEL.id, PLAIN_MODEL.id],
    default_model: SEARCH_MODEL.id,
  };
  const existing = await api(token, 'GET', `/admin/ai-engines/${PICKER_ENGINE.key}`);
  const response =
    existing.status === 404
      ? await api(token, 'POST', '/admin/ai-engines', {
          plugin_config: {
            manifest: await mockManifest(PICKER_ENGINE.key, PICKER_ENGINE.name, `${PICKER_LLM_URL}/v1/`),
            configuration: {},
            allowInsecureHttp: true,
            allowPrivateNetwork: true,
          },
          api_key: 'e2e-picker-key',
          ...state,
        })
      : await api(token, 'PUT', `/admin/ai-engines/${PICKER_ENGINE.key}`, state);
  if (response.status >= 300) {
    throw new Error(`preparing ${PICKER_ENGINE.key} failed: ${response.status} ${JSON.stringify(response.json)}`);
  }
}

export async function removeEngine(key: string) {
  const response = await api(await ownerToken(), 'DELETE', `/admin/ai-engines/${key}`);
  if (response.status >= 300 && response.status !== 404) {
    throw new Error(`removing ${key} failed: ${response.status} ${JSON.stringify(response.json)}`);
  }
}

export async function engineRow(key: string) {
  const [row] = await sql<{ isEnabled: boolean; whitelistModels: string[]; defaultModel: string; apiKeyStatus: string }>(
    'SELECT "isEnabled", "whitelistModels", "defaultModel", "apiKeyStatus" FROM ai_engines WHERE "engineKey" = $1',
    [key],
  );
  return row;
}

// ---- Admin -> AI Engines ----

export async function openEnginesPage(page: Page) {
  await page.goto('/admin/ai-engines');
  await expect(page.getByRole('heading', { name: 'AI Engines' })).toBeVisible();
  await expect(page.getByText('Loading AI engines...')).toHaveCount(0);
}

// A card under "Your Engines" (the page opens on the grid view).
export function engineCard(page: Page, displayName: string): Locator {
  return page.getByRole('listitem').filter({ has: page.getByText(displayName, { exact: true }) });
}

export async function openConfigure(page: Page, displayName: string): Promise<Locator> {
  await engineCard(page, displayName).getByRole('button', { name: 'Configure Engine' }).click();
  const dialog = page.getByRole('dialog', { name: `Configure ${displayName}` });
  await expect(dialog).toBeVisible();
  return dialog;
}

// ---- Chat ----

export async function openNewChat(page: Page) {
  await page.goto('/');
  await page.getByRole('textbox').first().waitFor();
  await dismissOnboarding(page);
}

export function modelChip(page: Page): Locator {
  return page.getByRole('button', { name: 'Select model', exact: true });
}

// Opens the model picker and searches it; returns the picker panel.
export async function searchModels(page: Page, query: string): Promise<Locator> {
  const picker = page.getByRole('dialog', { name: 'Select model' });
  if (!(await picker.isVisible())) await modelChip(page).click();
  await expect(picker).toBeVisible();
  await expect(picker.getByText('Loading models...')).toHaveCount(0);
  await picker.getByRole('textbox', { name: 'Search models…' }).fill(query);
  return picker;
}

export async function selectModel(page: Page, modelName: string, modelId: string) {
  const picker = await searchModels(page, modelName);
  await picker.getByRole('button', { name: modelName }).click();
  await expect(modelChip(page)).toContainText(modelId);
  if (await picker.isVisible()) await page.keyboard.press('Escape');
}

export async function send(page: Page, text: string) {
  const composer = page.getByRole('textbox', { name: 'Message input' });
  await composer.fill(text);
  await composer.press('Enter');
}
