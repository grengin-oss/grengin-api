// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test, type Page } from '@playwright/test';
import { api, sql } from '../../support/api';
import {
  MOCK_ENGINE,
  PICKER_ENGINE,
  PICKER_LLM_URL,
  engineCard,
  engineRow,
  mockManifest,
  openConfigure,
  openEnginesPage,
  ownerToken,
  removeEngine,
  resetPickerEngine,
} from '../../support/engines';

// The harness is offline: the catalog is served by the backend from GRENGIN_METADATA_BASE_URL.
// Requests from the browser to the public meta host are blocked and recorded.
async function blockPublicMetaHost(page: Page): Promise<string[]> {
  const attempted: string[] = [];
  await page.route(/^https:\/\/meta\.grengin\.com\//, (route) => {
    attempted.push(route.request().url());
    return route.abort();
  });
  return attempted;
}

// Providers the Browse dialog should offer: the catalog's AI providers that are not connected.
async function unconnectedCatalogProviders() {
  const token = await ownerToken();
  const discovery = await api(token, 'GET', '/discovery/ai-providers');
  const engines = await api(token, 'GET', '/admin/ai-engines');
  const connected = new Set(
    (engines.json as Array<{ engine_key: string; api_key_configured: boolean }>)
      .filter((engine) => engine.api_key_configured)
      .map((engine) => engine.engine_key),
  );
  const providers = (discovery.json.providers as Array<{ id: string; name: string; category?: string }>).filter(
    (provider) => !connected.has(provider.id),
  );
  const categories = (discovery.json.categories ?? []) as Array<{ id: string; label: string }>;
  return { providers, categories };
}

const GROQ = { key: 'groq', name: 'Groq', fakeKey: 'gsk_e2e_fake_key_that_is_not_valid' };

async function openBrowse(page: Page) {
  await page.getByRole('button', { name: 'Browse Providers' }).click();
  const dialog = page.getByRole('dialog', { name: 'Browse Providers' });
  await expect(dialog).toBeVisible();
  return dialog;
}

test.describe('AI Engines page', () => {
  test.beforeAll(resetPickerEngine);
  test.afterAll(() => removeEngine(PICKER_ENGINE.key));

  test('lists the configured mock engine with its default model and enabled switch', async ({ page }) => {
    await openEnginesPage(page);
    const card = engineCard(page, MOCK_ENGINE.name);
    await expect(card).toBeVisible();
    await expect(card.getByText(MOCK_ENGINE.model, { exact: true })).toBeVisible();
    await expect(card.getByRole('switch', { name: MOCK_ENGINE.name })).toHaveAttribute('aria-checked', 'true');
  });

  test('validating a working plugin engine reports the key valid and marks it Connected', async ({ page }) => {
    await openEnginesPage(page);
    const dialog = await openConfigure(page, PICKER_ENGINE.name);
    await dialog.getByRole('button', { name: 'Validate' }).click();
    await expect(dialog.getByRole('status').filter({ hasText: 'Key is valid and connected.' })).toBeVisible();
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(engineCard(page, PICKER_ENGINE.name).getByText('Connected', { exact: true })).toBeVisible();
    expect((await engineRow(PICKER_ENGINE.key)).apiKeyStatus).toBe('valid');
  });

  test.describe('Browse Providers', () => {
    test('offers every catalog provider that is not connected yet', async ({ page }) => {
      await blockPublicMetaHost(page);
      const { providers } = await unconnectedCatalogProviders();
      expect(providers.length).toBeGreaterThan(5);
      await openEnginesPage(page);
      const dialog = await openBrowse(page);
      await expect(dialog.getByText(`${providers.length} providers available to connect`)).toBeVisible();
      await expect(dialog.getByText(`Showing ${providers.length} of ${providers.length} providers`)).toBeVisible();
      // Groups show three cards until expanded.
      const more = dialog.getByRole('button', { name: /^\+ Show \d+ more$/ });
      for (let remaining = await more.count(); remaining > 0; remaining--) {
        await more.first().click();
        await expect(more).toHaveCount(remaining - 1);
      }
      for (const provider of providers) {
        await expect(dialog.getByRole('button', { name: `Connect ${provider.name}`, exact: true })).toBeVisible();
      }
      // Engines that are already connected are not offered again.
      await expect(dialog.getByRole('button', { name: `Connect ${MOCK_ENGINE.name}` })).toHaveCount(0);
    });

    test('groups providers by the catalog categories the API returns, without calling the public meta host', async ({ page }) => {
      // Known webapp bug (grengin-docs/webapp-main-changes-0.9.13.md section 2): webapp main loads
      // the grouping from https://meta.grengin.com/distributions/grengin-api/<version>/categories.json
      // in the browser (providerCategories.ts fetchProviderCategories) instead of the `categories`
      // and per-provider `category` that GET /discovery/ai-providers already returns from the
      // installation's own catalog. Offline, air-gapped or own-catalog installs (like this harness)
      // therefore show every provider under a single "Other Providers" group. Remove `fail` once fixed.
      test.fail();
      const attempted = await blockPublicMetaHost(page);
      const { providers, categories } = await unconnectedCatalogProviders();
      expect(categories.map((category) => category.label)).toEqual(
        expect.arrayContaining(['Frontier Labs', 'Cloud Platforms', 'Aggregators', 'Fast Inference', 'Independent Labs']),
      );
      const expectedTabs = categories
        .filter((category) => providers.some((provider) => provider.category === category.id))
        .map((category) => {
          const count = providers.filter((provider) => provider.category === category.id).length;
          return new RegExp(`^${category.label}\\s*${count}$`);
        });

      await openEnginesPage(page);
      const dialog = await openBrowse(page);
      const tabs = dialog.getByRole('group', { name: 'Filter providers by category' }).getByRole('button');
      await expect(tabs).toHaveText([new RegExp(`^All\\s*${providers.length}$`), ...expectedTabs]);

      await tabs.filter({ hasText: 'Frontier Labs' }).click();
      await expect(dialog.getByRole('button', { name: 'Connect OpenAI', exact: true })).toBeVisible();
      await expect(dialog.getByRole('button', { name: 'Connect DeepSeek', exact: true })).toHaveCount(0);
      expect(attempted.filter((url) => url.includes('/categories.json'))).toEqual([]);
    });
  });

  test('refuses a custom engine on a plain-HTTP private address with a clear error', async ({ page }) => {
    // The backend only calls plain-HTTP or private-network endpoints when the plugin config carries
    // the administrator's explicit allowInsecureHttp / allowPrivateNetwork approval; the dialog has
    // no such option, so the mock LLM cannot be added from here and nothing must be created.
    const key = 'e2e-custom-plain-http';
    const manifest = await mockManifest(key, 'E2E Custom Plain HTTP', `${PICKER_LLM_URL}/v1/`);
    try {
      await openEnginesPage(page);
      await page.getByRole('button', { name: 'Add Custom Engine' }).click();
      const dialog = page.getByRole('dialog', { name: 'Add Custom Engine' });
      await dialog.getByLabel('Display Name').fill('E2E Custom Plain HTTP');
      await dialog.getByLabel('Base URL Override').fill(`${PICKER_LLM_URL}/v1/`);
      await dialog.getByLabel('Plugin JSON').fill(JSON.stringify(manifest, null, 2));
      await dialog.getByRole('textbox', { name: 'API Key' }).fill('e2e-custom-key');
      await dialog.getByRole('button', { name: 'Add Engine' }).click();
      await expect(dialog.getByRole('alert')).toContainText('plugin_config');
      await expect(dialog).toBeVisible();
      expect(await engineRow(key)).toBeUndefined();
      await dialog.getByRole('button', { name: 'Cancel' }).click();
      await expect(engineCard(page, 'E2E Custom Plain HTTP')).toHaveCount(0);
    } finally {
      await removeEngine(key);
    }
  });

  test('a catalog engine connected with a fake key validates as invalid, and removing the key disconnects it', async ({ page }) => {
    // Validation lists models at the provider's real endpoint (api.groq.com), which answers 401 to
    // a made-up key. The run needs outbound internet for this; offline the key would stay
    // "not_validated" ("could not be validated right now").
    test.skip((await engineRow(GROQ.key)) !== undefined, 'Groq is already installed in this database');
    try {
      await openEnginesPage(page);
      const browse = await openBrowse(page);
      await browse.getByRole('textbox', { name: 'Search providers…' }).fill(GROQ.name);
      await browse.getByRole('button', { name: `Connect ${GROQ.name}`, exact: true }).click();

      const connect = page.getByRole('dialog', { name: `Connect ${GROQ.name}` });
      await connect.getByRole('textbox', { name: 'API Key' }).fill(GROQ.fakeKey);
      await connect.getByRole('button', { name: 'Connect and Save' }).click();
      await expect(connect).toBeHidden();
      await expect(page.getByText(`${GROQ.name} connected successfully`)).toBeVisible();
      const card = engineCard(page, GROQ.name);
      await expect(card.getByText('Not Validated', { exact: true })).toBeVisible();
      expect(await engineRow(GROQ.key)).toMatchObject({ isEnabled: true, apiKeyStatus: 'not_validated' });

      const configure = await openConfigure(page, GROQ.name);
      const validate = configure.getByRole('button', { name: 'Validate' });
      await validate.click();
      await expect.poll(async () => (await engineRow(GROQ.key)).apiKeyStatus).toBe('invalid');
      await expect(validate).toBeEnabled();
      await expect(configure.getByRole('status').filter({ hasText: 'Key is valid and connected.' })).toHaveCount(0);
      await configure.getByRole('button', { name: 'Cancel' }).click();
      await expect(card.getByText('Connected', { exact: true })).toHaveCount(0);

      const again = await openConfigure(page, GROQ.name);
      await again.getByRole('button', { name: 'Delete' }).click();
      await again.getByRole('button', { name: 'Confirm Delete' }).click();
      await expect(page.getByText('API key removed successfully')).toBeVisible();
      await again.getByRole('button', { name: 'Cancel' }).click();
      await expect(card).toHaveCount(0);
      const [row] = await sql<{ apiKey: string | null; isEnabled: boolean }>(
        'SELECT "apiKey", "isEnabled" FROM ai_engines WHERE "engineKey" = $1',
        [GROQ.key],
      );
      expect(row).toMatchObject({ apiKey: null, isEnabled: false });
      const browseAgain = await openBrowse(page);
      await browseAgain.getByRole('textbox', { name: 'Search providers…' }).fill(GROQ.name);
      await expect(browseAgain.getByRole('button', { name: `Connect ${GROQ.name}`, exact: true })).toBeVisible();
    } finally {
      await removeEngine(GROQ.key);
    }
  });

  test('an API key the provider rejects is shown as invalid in the dialog and on the card', async ({ page }) => {
    // Known bug: the API reports a rejected key as api_key_status "invalid" (ApiKeyStatus::Invalid,
    // serde snake_case), but webapp main only recognises "in_valid" (admin/types.ts,
    // aiEnginesStore.getStatusMessage, AIEngines.getEngineStatus). After Validate, the dialog
    // replaces the result with "API key is not configured. Please add an API key." (also the error
    // toast) and the card says "Not Configured" instead of "Invalid Key". Remove `fail` once the two
    // sides agree on the value.
    test.fail();
    test.skip((await engineRow(GROQ.key)) !== undefined, 'Groq is already installed in this database');
    const token = await ownerToken();
    try {
      const created = await api(token, 'POST', '/admin/ai-engines', { engine_key: GROQ.key, api_key: GROQ.fakeKey });
      expect(created.status).toBeLessThan(300);
      await openEnginesPage(page);
      const configure = await openConfigure(page, GROQ.name);
      await configure.getByRole('button', { name: 'Validate' }).click();
      await expect.poll(async () => (await engineRow(GROQ.key)).apiKeyStatus).toBe('invalid');
      await expect(configure.getByRole('status')).toHaveText('Error: The provided API key is invalid. Please check and try again.', { timeout: 5_000 });
      await configure.getByRole('button', { name: 'Cancel' }).click();
      await expect(engineCard(page, GROQ.name).getByText('Invalid Key', { exact: true })).toBeVisible({ timeout: 5_000 });
    } finally {
      await removeEngine(GROQ.key);
    }
  });
});
