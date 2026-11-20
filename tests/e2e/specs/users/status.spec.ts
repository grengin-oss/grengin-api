// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import {
  composer,
  expectSignInRefused,
  findUserRow,
  openUsersAdmin,
  provisionPerson,
  signInPerson,
  startSignIn,
  statusCell,
  userByEmail,
} from '../../support/users';

test.describe('account status', () => {
  test('deactivating a person ends their session and blocks sign-in until they are reactivated', async ({ page, browser }) => {
    test.setTimeout(120_000);
    const { person, context, page: member } = await provisionPerson(browser, 'leaver');
    await expect(composer(member)).toBeVisible();

    await openUsersAdmin(page);
    const row = await findUserRow(page, person.email);
    await row.getByRole('button', { name: 'Disable user' }).click();
    await expect(page.getByRole('status').filter({ hasText: 'User deactivated successfully' })).toBeVisible();
    await expect(statusCell(row)).toHaveText('Deactivated');
    await expect.poll(async () => (await userByEmail(person.email))?.status).toBe('deactivated');

    try {
      // The open session no longer works: the next page load drops back to the sign-in page.
      await member.reload();
      await expect(member.getByRole('heading', { level: 1 }).first()).toBeVisible();
      await expect(composer(member)).toHaveCount(0);

      // A fresh sign-in through the IdP is refused with the reason.
      await startSignIn(member, person);
      await expectSignInRefused(member, /account has been deactivated/i);
    } finally {
      // Reactivate even if the checks above failed, so nothing is left switched off.
      await row.getByRole('button', { name: 'Enable user' }).click();
      await expect(page.getByRole('status').filter({ hasText: 'User activated successfully' })).toBeVisible();
    }
    await expect(statusCell(row)).toHaveText('Active');

    await signInPerson(member, person);
    await expect(composer(member)).toBeVisible();
    await context.close();
  });
});
