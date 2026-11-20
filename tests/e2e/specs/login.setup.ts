// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test as setup } from '@playwright/test';
import { dismissOnboarding, signIn } from '../support/login';

setup('the bootstrapped owner signs in through the identity provider', async ({ page }) => {
  await signIn(page, 'owner');
  await page.getByRole('textbox').first().waitFor();
  await dismissOnboarding(page);
  // Signed in: the chat composer is available and the login page is gone.
  await expect(page.getByRole('textbox').first()).toBeVisible();
  await expect(page.getByRole('button', { name: /E2E IdP/ })).toHaveCount(0);
  await page.context().storageState({ path: 'state/owner.json' });
});
