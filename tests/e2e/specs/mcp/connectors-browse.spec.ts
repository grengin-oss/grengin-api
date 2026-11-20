// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { readFile } from 'node:fs/promises';
import path from 'node:path';
import { expect, test } from '@playwright/test';
import { api } from '../../support/api';
import { env } from '../../support/env';
import { tokenOf } from '../../support/mcp';

// Native connector suites (one sign-in, several apps) published in the 0.9 connector catalog.
const SUITES = [
  { id: 'google-basic', search: 'Google Basic', label: /^Google Basic Apps$/ },
  { id: 'zoho', search: 'Zoho', label: /^Zoho( \((US|India)\))?$/ },
];

test.describe('connectors browse', () => {
  test('the discovery API lists the native connector suites', async () => {
    const { status, json } = await api(await tokenOf('owner'), 'GET', '/discovery/connector-plugins');
    expect(status).toBe(200);
    const providers: { id: string; name: string; members: string[] }[] = json.providers;
    for (const suite of SUITES) {
      const entry = providers.find((provider) => provider.id === suite.id);
      expect(entry, `${suite.id} in /discovery/connector-plugins`).toBeTruthy();
      expect(entry!.name).toMatch(suite.label);
      expect(entry!.members.length).toBeGreaterThan(1);
    }
  });

  test('Browse connectors offers the native connector suites', async ({ page }) => {
    // Known webapp gap: webapp main browses the legacy catalog (META_BASE/connectors.json in
    // api/admin/connectors.ts fetchCatalog) instead of GET /discovery/connector-plugins, so none
    // of the suites (Google Basic Apps, Zoho, Microsoft 365, …) is offered. See
    // grengin-docs/connector-suites-frontend-integration.md. Remove `fail` once Browse uses discovery.
    test.fail();
    // The webapp reads the public catalog host directly; serve it from the local catalog checkout.
    await page.route('https://meta.grengin.com/**', async (route) => {
      const file = path.join(env.catalogDir, new URL(route.request().url()).pathname);
      try {
        const body = await readFile(file);
        await route.fulfill({ body, headers: { 'access-control-allow-origin': '*' }, contentType: file.endsWith('.json') ? 'application/json' : undefined });
      } catch {
        await route.fulfill({ status: 404, body: '' });
      }
    });

    await page.goto('/admin/mcp-servers');
    await page
      .getByRole('button', { name: 'Browse connectors' })
      .or(page.getByRole('button', { name: 'Skip and browse all tools' }))
      .first()
      .click();
    const dialog = page.getByRole('dialog', { name: 'Browse connectors' });
    await expect(dialog).toBeVisible();
    const search = dialog.getByRole('textbox', { name: 'Search connectors' });
    // The catalog does load (its single-app connectors are offered), so the suites are simply missing.
    await search.fill('Gmail');
    await expect(dialog.getByText('Gmail', { exact: true }).first()).toBeVisible();
    for (const suite of SUITES) {
      await search.fill(suite.search);
      await expect(dialog.getByText(suite.label).first(), `${suite.id} offered in Browse`).toBeVisible({ timeout: 5_000 });
    }
  });
});
