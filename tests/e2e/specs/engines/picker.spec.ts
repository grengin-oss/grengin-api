// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { api, sql } from '../../support/api';
import {
  MOCK_ENGINE,
  PICKER_ENGINE,
  PLAIN_MODEL,
  SEARCH_MODEL,
  engineCard,
  engineRow,
  modelChip,
  openConfigure,
  openEnginesPage,
  openNewChat,
  ownerToken,
  removeEngine,
  resetPickerEngine,
  searchModels,
  selectModel,
  send,
} from '../../support/engines';

const unique = () => `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;

async function assistantRow(contentPrefix: string) {
  const [row] = await sql<{ model: string; provider: string }>(
    `SELECT "modelName" AS model, "modelProvider" AS provider FROM messages
     WHERE role = 'assistant' AND "messageContent" LIKE $1 ORDER BY "createdAt" DESC LIMIT 1`,
    [`${contentPrefix}%`],
  );
  return row ?? null;
}

test.describe('chat model picker', () => {
  test.beforeEach(resetPickerEngine);
  test.afterAll(() => removeEngine(PICKER_ENGINE.key));

  test('the selected model answers the next message and is recorded on it', async ({ page }) => {
    const id = unique();
    await openNewChat(page);
    await selectModel(page, SEARCH_MODEL.name, SEARCH_MODEL.id);
    await send(page, `picker one ${id}`);
    const searchReply = `Picker reply from ${SEARCH_MODEL.id}: picker one ${id}`;
    await expect(page.getByRole('log').getByText(searchReply)).toBeVisible();
    await expect.poll(() => assistantRow(searchReply)).toEqual({ model: SEARCH_MODEL.id, provider: PICKER_ENGINE.key });

    // A new chat opens on the model used last; switching back to the first engine's model
    // sends the next chat there.
    await openNewChat(page);
    await expect(modelChip(page)).toContainText(SEARCH_MODEL.id);
    await selectModel(page, MOCK_ENGINE.modelName, MOCK_ENGINE.model);
    await send(page, `picker two ${id}`);
    const mockReply = `Mock reply: picker two ${id}`;
    await expect(page.getByRole('log').getByText(mockReply)).toBeVisible();
    await expect.poll(() => assistantRow(mockReply)).toEqual({ model: MOCK_ENGINE.model, provider: MOCK_ENGINE.key });
  });

  test('removing a model from the whitelist removes it from the picker', async ({ page }) => {
    await openNewChat(page);
    let picker = await searchModels(page, 'E2E');
    for (const name of [SEARCH_MODEL.name, PLAIN_MODEL.name, MOCK_ENGINE.modelName]) {
      await expect(picker.getByRole('button', { name })).toBeVisible();
    }

    await openEnginesPage(page);
    const dialog = await openConfigure(page, PICKER_ENGINE.name);
    const plain = dialog.getByRole('checkbox', { name: PLAIN_MODEL.name });
    await expect(plain).toHaveAttribute('aria-checked', 'true');
    await plain.click();
    await expect(plain).toHaveAttribute('aria-checked', 'false');
    await expect(dialog.getByText('1 of 2 enabled')).toBeVisible();
    await dialog.getByRole('button', { name: 'Save Changes' }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByText(`${PICKER_ENGINE.name} updated successfully`)).toBeVisible();
    await expect(engineCard(page, PICKER_ENGINE.name).getByText('1 models', { exact: true })).toBeVisible();
    expect((await engineRow(PICKER_ENGINE.key)).whitelistModels).toEqual([SEARCH_MODEL.id]);

    await openNewChat(page);
    picker = await searchModels(page, 'E2E');
    await expect(picker.getByRole('button', { name: SEARCH_MODEL.name })).toBeVisible();
    await expect(picker.getByRole('button', { name: MOCK_ENGINE.modelName })).toBeVisible();
    await expect(picker.getByRole('button', { name: PLAIN_MODEL.name })).toHaveCount(0);
    await searchModels(page, PLAIN_MODEL.name);
    await expect(picker.getByText('No models match your search')).toBeVisible();
  });

  test("changing an engine's default model is saved and shown on its card", async ({ page }) => {
    await openEnginesPage(page);
    const card = engineCard(page, PICKER_ENGINE.name);
    await expect(card.getByText(SEARCH_MODEL.id, { exact: true })).toBeVisible();
    const dialog = await openConfigure(page, PICKER_ENGINE.name);
    await dialog.getByRole('combobox', { name: 'Default Model for this Engine' }).selectOption(PLAIN_MODEL.id);
    // The default model cannot be taken off the whitelist.
    await expect(dialog.getByRole('checkbox', { name: PLAIN_MODEL.name })).toBeDisabled();
    await dialog.getByRole('button', { name: 'Save Changes' }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByText(`${PICKER_ENGINE.name} updated successfully`)).toBeVisible();
    await expect(card.getByText(PLAIN_MODEL.id, { exact: true })).toBeVisible();
    expect((await engineRow(PICKER_ENGINE.key)).defaultModel).toBe(PLAIN_MODEL.id);
  });

  test('"Set as system default engine" is saved', async ({ page }) => {
    // Known bug: the webapp saves the system default engine by writing
    // `settings.default_engine` / `settings.default_model` into PUT /admin/branding
    // (aiEnginesStore.updateEngine), but the API's branding has no `settings` (BrandingUpdate
    // only takes name/logoUrl/colorPrimary/colorAccent/fontFamily and ignores the rest), and
    // nothing in the API reads a default engine. The dialog reports "<engine> set as system
    // default successfully", yet reopening it shows the box unchecked and new chats never use
    // the engine. Remove `fail` once the default engine is stored and honoured.
    test.fail();
    const token = await ownerToken();
    const brandingBefore = (await api(token, 'GET', '/admin/branding')).json;
    try {
      await openEnginesPage(page);
      let dialog = await openConfigure(page, PICKER_ENGINE.name);
      const systemDefault = dialog.getByRole('checkbox', { name: 'Set as system default engine' });
      await expect(systemDefault).toHaveAttribute('aria-checked', 'false');
      await systemDefault.click();
      await expect(systemDefault).toHaveAttribute('aria-checked', 'true');
      await dialog.getByRole('button', { name: 'Save Changes' }).click();
      await expect(dialog).toBeHidden();
      await expect(page.getByText(`${PICKER_ENGINE.name} set as system default successfully`)).toBeVisible();

      await openEnginesPage(page);
      dialog = await openConfigure(page, PICKER_ENGINE.name);
      await expect(dialog.getByRole('checkbox', { name: 'Set as system default engine' })).toHaveAttribute(
        'aria-checked',
        'true',
        { timeout: 5_000 },
      );
    } finally {
      const after = (await api(token, 'GET', '/admin/branding')).json;
      if (JSON.stringify(after) !== JSON.stringify(brandingBefore)) {
        await api(token, 'PUT', '/admin/branding', brandingBefore);
      }
    }
  });

  test("disabling an engine removes its models from the picker; enabling brings them back", async ({ page }) => {
    try {
      await openEnginesPage(page);
      const toggle = engineCard(page, PICKER_ENGINE.name).getByRole('switch', { name: PICKER_ENGINE.name });
      await expect(toggle).toHaveAttribute('aria-checked', 'true');
      await toggle.click();
      await expect(page.getByText(`${PICKER_ENGINE.name} disabled`)).toBeVisible();
      await expect(toggle).toHaveAttribute('aria-checked', 'false');
      await expect.poll(async () => (await engineRow(PICKER_ENGINE.key)).isEnabled).toBe(false);

      await openNewChat(page);
      let picker = await searchModels(page, 'E2E');
      await expect(picker.getByRole('button', { name: MOCK_ENGINE.modelName })).toBeVisible();
      await expect(picker.getByRole('button', { name: SEARCH_MODEL.name })).toHaveCount(0);
      await expect(picker.getByRole('button', { name: PLAIN_MODEL.name })).toHaveCount(0);

      await openEnginesPage(page);
      await expect(toggle).toHaveAttribute('aria-checked', 'false');
      await toggle.click();
      await expect(page.getByText(`${PICKER_ENGINE.name} enabled`)).toBeVisible();
      await expect(toggle).toHaveAttribute('aria-checked', 'true');

      await openNewChat(page);
      picker = await searchModels(page, 'E2E');
      await expect(picker.getByRole('button', { name: SEARCH_MODEL.name })).toBeVisible();
      await expect(picker.getByRole('button', { name: PLAIN_MODEL.name })).toBeVisible();
    } finally {
      await resetPickerEngine();
    }
  });
});

test.describe('web search toggle', () => {
  test.beforeEach(resetPickerEngine);
  test.afterAll(() => removeEngine(PICKER_ENGINE.key));

  test('is available for a model that supports web search', async ({ page }) => {
    await openNewChat(page);
    await selectModel(page, SEARCH_MODEL.name, SEARCH_MODEL.id);
    const toggle = page.getByRole('button', { name: 'Enable web search' });
    await expect(toggle).toBeEnabled();
    await toggle.click();
    await expect(page.getByRole('button', { name: 'Disable web search' })).toHaveAttribute('aria-pressed', 'true');
  });

  test("is disabled for a model that can't search the web, and switching to one turns it off", async ({ page }) => {
    // Known webapp gap (grengin-docs/webapp-main-changes-0.9.13.md section 3): GET /models sends
    // supports_web_search per model (e2e-chat inherits DeepSeek's false; e2e-plain is false), but
    // webapp main never reads it, so the globe toggle stays enabled for every model and stays on
    // after switching to a model that ignores it. Expected: disabled with the tooltip
    // "This model can't search the web", and shown off. Remove `fail` once implemented.
    test.fail();
    const models = await api(await ownerToken(), 'GET', '/models');
    const flags = Object.fromEntries(
      (models.json.providers as Array<{ models: Array<{ key: string; supports_web_search: boolean }> }>)
        .flatMap((provider) => provider.models)
        .map((model) => [model.key, model.supports_web_search]),
    );
    expect(flags).toMatchObject({ [MOCK_ENGINE.model]: false, [SEARCH_MODEL.id]: true, [PLAIN_MODEL.id]: false });

    await openNewChat(page);
    await selectModel(page, SEARCH_MODEL.name, SEARCH_MODEL.id);
    await page.getByRole('button', { name: 'Enable web search' }).click();
    await expect(page.getByRole('button', { name: 'Disable web search' })).toHaveAttribute('aria-pressed', 'true');

    await selectModel(page, PLAIN_MODEL.name, PLAIN_MODEL.id);
    const toggle = page.getByRole('button', { name: /web search/i });
    await expect(toggle).toHaveAttribute('aria-pressed', 'false', { timeout: 5_000 });
    await expect(toggle).toBeDisabled();
    await expect(toggle).toHaveAttribute('title', "This model can't search the web");

    await selectModel(page, MOCK_ENGINE.modelName, MOCK_ENGINE.model);
    await expect(toggle).toBeDisabled();
  });
});
