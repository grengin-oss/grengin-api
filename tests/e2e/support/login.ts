// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, type Page } from '@playwright/test';
import { USERS } from '../mocks/users.mjs';

export const IDP_NAME = 'E2E IdP';

// Signs in through the real flow: login page -> provider button -> mock IdP -> callback.
export async function signIn(page: Page, user: keyof typeof USERS) {
  await page.goto('/');
  await page.getByRole('button', { name: new RegExp(IDP_NAME) }).click();
  await expect(page.getByRole('heading', { name: 'E2E identity provider' })).toBeVisible();
  await page.getByTestId(`login-${user}`).click();
  await page.waitForURL((url) => !url.pathname.includes('/callback') && !url.host.includes(':9401'), { timeout: 30_000 });
}
