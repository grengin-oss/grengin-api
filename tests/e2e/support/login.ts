// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test, type Page } from '@playwright/test';
import { USERS } from '../mocks/users.mjs';

export const IDP_SLUG = 'e2e-idp';
export const IDP_NAME = 'E2E IdP';

// Signs in through the real flow: login page -> provider button -> mock IdP -> webapp callback
// -> API callback. If the login page does not list the provider (a webapp bug tracked by
// auth.spec.ts), it starts the same flow the button starts, so the rest of the suite still runs.
export async function signIn(page: Page, user: keyof typeof USERS) {
  await page.goto('/');
  const button = page.getByRole('button', { name: new RegExp(IDP_NAME) });
  await page.getByRole('heading', { level: 1 }).first().waitFor();
  if (await button.isVisible().catch(() => false)) {
    await button.click();
  } else {
    test.info().annotations.push({
      type: 'known issue',
      description: 'The login page does not list configured sign-in providers (webapp reads slug/enabled; the API sends provider/is_enabled).',
    });
    // What the provider button's initiateOAuth does: remember the provider for the callback
    // page, then follow the API's login path for it.
    const redirect = new URL(`/auth/${IDP_SLUG}/callback`, page.url()).toString();
    await page.evaluate((slug) => sessionStorage.setItem('oauth_provider', slug), IDP_SLUG);
    await page.goto(`/api/auth/${IDP_SLUG}?redirect_uri=${encodeURIComponent(redirect)}`);
  }
  await expect(page.getByRole('heading', { name: 'E2E identity provider' })).toBeVisible();
  await page.getByTestId(`login-${user}`).click();
  await page.waitForURL((url) => !url.pathname.includes('/callback') && !url.host.endsWith(':9401'), { timeout: 30_000 });
}

// First sign-in shows a guided tour over the chat screen; skip it like a returning user would.
export async function dismissOnboarding(page: Page) {
  for (const name of ['Skip tour', 'Hide setup guide']) {
    const button = page.getByRole('button', { name });
    if (await button.isVisible().catch(() => false)) await button.click();
  }
}
