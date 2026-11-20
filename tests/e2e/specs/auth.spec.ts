// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { IDP_NAME } from '../support/login';

test.describe('sign-in page', () => {
  test.use({ storageState: { cookies: [], origins: [] } });

  test('lists the sign-in providers configured on the installation', async ({ page }) => {
    // Known webapp bug: Login.svelte keeps providers with `enabled` and reads `slug`, while
    // GET /auth/providers returns `is_enabled` and `provider`, so configured providers never
    // show and the hardcoded Google/Azure fallback is displayed instead. Remove `fail` once fixed.
    test.fail();
    await page.goto('/');
    await expect(page.getByRole('button', { name: new RegExp(IDP_NAME) })).toBeVisible({ timeout: 10_000 });
  });

  test('does not offer providers that are not configured', async ({ page }) => {
    // Same bug: the fallback offers Google and Azure although neither is configured here.
    test.fail();
    await page.goto('/');
    await page.getByRole('heading', { level: 1 }).first().waitFor();
    await expect(page.getByRole('button', { name: /Sign in with Google/ })).toHaveCount(0);
  });
});
