// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import {
  composer,
  ensurePeopleIdp,
  expectSignInRefused,
  findUserRow,
  newPerson,
  openUsersAdmin,
  personContext,
  rolesOf,
  signInPerson,
  startSignIn,
  statusCell,
  userByEmail,
} from '../../support/users';

test.describe('first sign-in (JIT provisioning)', () => {
  test.beforeAll(ensurePeopleIdp);

  test('a newcomer waits for approval until an admin activates them, then gets in', async ({ page, browser }) => {
    const person = newPerson('newcomer');
    const { context, page: newcomer } = await personContext(browser);

    // First sign-in creates the account but does not let the person in.
    await startSignIn(newcomer, person);
    await expectSignInRefused(newcomer, /pending administrator approval/i);
    const pending = await userByEmail(person.email);
    expect(pending?.status).toBe('pending');
    expect(await rolesOf(pending.id)).toEqual(['User']);

    // Signing in again before approval is still refused.
    await startSignIn(newcomer, person);
    await expectSignInRefused(newcomer, /pending administrator approval/i);

    // The owner finds the newcomer in Organization > Users and enables the account.
    await openUsersAdmin(page);
    const row = await findUserRow(page, person.email);
    await expect(row).toContainText(person.name);
    await expect(row.getByTitle('Edit user')).toHaveText('User');
    await row.getByRole('button', { name: 'Enable user' }).click();
    await expect(page.getByRole('status').filter({ hasText: 'User activated successfully' })).toBeVisible();
    await expect(statusCell(row)).toHaveText('Active');
    await expect(row.getByRole('button', { name: 'Disable user' })).toHaveAttribute('aria-pressed', 'true');
    await expect.poll(async () => (await userByEmail(person.email))?.status).toBe('active');

    // Now the same identity signs in and lands in the chat workspace.
    await signInPerson(newcomer, person);
    await expect(composer(newcomer)).toBeVisible();
    await expect(newcomer.getByRole('button', { name: 'User menu' })).toContainText(person.email);
    await context.close();
  });

  test('the Users list tells a sign-up waiting for approval apart from a deactivated account', async ({ page, browser }) => {
    // Known webapp bug: UserRow.svelte only knows "active" and labels every other status
    // "Deactivated" (and the status filter offers only Active/Deactivated), although
    // GET /admin/users returns status "pending" for JIT sign-ups awaiting approval. An admin
    // cannot see who is waiting for approval. Remove `fail` once pending accounts are labelled.
    test.fail();
    const person = newPerson('waiting');
    const { context, page: newcomer } = await personContext(browser);
    await startSignIn(newcomer, person);
    await expectSignInRefused(newcomer, /pending administrator approval/i);
    await context.close();
    expect((await userByEmail(person.email))?.status).toBe('pending');

    await openUsersAdmin(page);
    const row = await findUserRow(page, person.email);
    await expect(statusCell(row)).toHaveText(/pending/i, { timeout: 5_000 });
  });
});
