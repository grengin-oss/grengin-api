// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test, type Page } from '@playwright/test';
import { USERS } from '../../mocks/users.mjs';
import { env } from '../../support/env';
import { dismissOnboarding, signIn } from '../../support/login';

async function openUserMenu(page: Page) {
  const trigger = page.getByRole('button', { name: 'User menu' });
  await trigger.click();
  await expect(trigger).toHaveAttribute('aria-expanded', 'true');
  return page.getByRole('menu', { name: 'User menu' });
}

test.describe('account', () => {
  test('the user menu shows who is signed in and leads to settings and the Control Hub', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('textbox').first().waitFor();
    await dismissOnboarding(page);
    const trigger = page.getByRole('button', { name: 'User menu' });
    await expect(trigger).toContainText(USERS.owner.name);
    await expect(trigger).toContainText(USERS.owner.email);

    const menu = await openUserMenu(page);
    await expect(menu.getByRole('menuitem', { name: 'Settings' })).toBeVisible();
    await expect(menu.getByRole('menuitem', { name: 'Sign out' })).toBeVisible();
    await menu.getByRole('menuitem', { name: 'Control Hub' }).click();
    await expect(page).toHaveURL(/\/admin\//);
    await expect(page.getByRole('navigation', { name: 'Admin navigation' })).toBeVisible();
  });

  // Signs in again in a separate browser context, so the shared owner session other specs use
  // is not the one being signed out.
  test.describe('sign out', () => {
    test('returns to the sign-in page, and a reload does not bring the session back', async ({ browser }) => {
      const context = await browser.newContext({ storageState: { cookies: [], origins: [] } });
      try {
        const page = await context.newPage();
        await signIn(page, 'owner');
        await page.getByRole('textbox').first().waitFor();
        await dismissOnboarding(page);

        const menu = await openUserMenu(page);
        await menu.getByRole('menuitem', { name: 'Sign out' }).click();
        await expect(page.getByRole('heading', { name: 'Welcome to Grengin', level: 1 })).toBeVisible();
        await expect(page.getByRole('button', { name: 'User menu' })).toHaveCount(0);

        await page.reload();
        await expect(page.getByRole('heading', { name: 'Welcome to Grengin', level: 1 })).toBeVisible();
        await expect(page.getByRole('button', { name: 'User menu' })).toHaveCount(0);
        await page.goto('/admin/audit-logs');
        await expect(page.getByRole('heading', { name: 'Welcome to Grengin', level: 1 })).toBeVisible();
        await expect(page.getByText('Audit Logs', { exact: true })).toHaveCount(0);
        const stored = await page.evaluate(() => [localStorage.getItem('grengin_access_token'), localStorage.getItem('grengin_refresh_token')]);
        expect(stored.filter(Boolean)).toEqual([]);
      } finally {
        await context.close();
      }
    });

    test('ends the session on the server, so its refresh token stops working', async ({ browser }) => {
      // Known bug: Sign out only clears the browser's storage. The webapp calls POST /auth/logout,
      // which grengin-api does not have (the call 404s and the error is swallowed), and refresh
      // tokens are stateless 7-day JWTs, so a copied refresh token keeps minting access tokens
      // after the user signed out. Remove `fail` once sign-out revokes the session server-side.
      test.fail();
      const context = await browser.newContext({ storageState: { cookies: [], origins: [] } });
      try {
        const page = await context.newPage();
        await signIn(page, 'owner');
        await page.getByRole('textbox').first().waitFor();
        await dismissOnboarding(page);
        const refreshToken = await page.evaluate(() => localStorage.getItem('grengin_refresh_token'));
        expect(refreshToken).toBeTruthy();

        const logout = page.waitForResponse((response) => response.url().includes('/auth/logout'));
        const menu = await openUserMenu(page);
        await menu.getByRole('menuitem', { name: 'Sign out' }).click();
        await expect(page.getByRole('heading', { name: 'Welcome to Grengin', level: 1 })).toBeVisible();
        test.info().annotations.push({ type: 'POST /auth/logout', description: String((await logout).status()) });

        const refreshed = await fetch(`${env.apiUrl}/auth/refresh`, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ refresh_token: refreshToken }),
        });
        expect(refreshed.status).toBe(401);
      } finally {
        await context.close();
      }
    });
  });
});
