// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { findUserRow, openUserMenu, openUsersAdmin, provisionPerson, rolesOf } from '../../support/users';

test.describe('roles', () => {
  test('granting a role opens the Control Hub to the person and removing it closes it again', async ({ page, browser }) => {
    test.setTimeout(120_000);
    const { person, context, page: member, id } = await provisionPerson(browser, 'rolee');

    // A plain User has no admin entry and is turned away from admin pages.
    await openUserMenu(member);
    await expect(member.getByRole('menuitem', { name: 'Control Hub' })).toHaveCount(0);
    await member.goto('/admin/departments');
    await expect(member.getByRole('heading', { name: 'Access denied' })).toBeVisible();

    // The owner grants HR Admin (users, departments, roles) from the user's row.
    await openUsersAdmin(page);
    const row = await findUserRow(page, person.email);
    await row.getByTitle('Edit user').click();
    const dialog = page.getByRole('dialog', { name: 'Edit User' });
    await expect(dialog.getByText('User', { exact: true })).toBeVisible();
    await dialog.getByRole('button', { name: 'Add role' }).click();
    await dialog.getByRole('textbox', { name: 'Search roles...' }).fill('HR Admin');
    await dialog.getByRole('button', { name: 'Add role HR Admin globally' }).click();
    await expect(dialog.getByRole('button', { name: 'Remove role HR Admin' })).toBeVisible();
    await expect.poll(() => rolesOf(id)).toEqual(['HR Admin', 'User']);
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).toHaveCount(0);

    // After a reload the person has the Control Hub and can manage people.
    await member.goto('/');
    await openUserMenu(member);
    await member.getByRole('menuitem', { name: 'Control Hub' }).click();
    await member.waitForURL(/\/admin\/departments/);
    await expect(member.getByRole('heading', { name: 'Access denied' })).toHaveCount(0);
    await expect(member.getByRole('tab', { name: 'Users' })).toBeVisible();
    await expect(member.getByRole('button', { name: 'Create User' })).toBeVisible();
    // HR Admin carries no AI-platform permission, so that page stays closed.
    await member.goto('/admin/ai-engines');
    await expect(member.getByRole('heading', { name: 'Access denied' })).toBeVisible();

    // The owner removes the role again.
    await page.reload();
    const again = await findUserRow(page, person.email);
    await again.getByTitle('Edit user').click();
    await dialog.getByRole('button', { name: 'Remove role HR Admin' }).click();
    const confirm = page.getByRole('dialog', { name: 'Remove role?' });
    await confirm.getByRole('button', { name: 'Delete' }).click();
    await expect(dialog.getByRole('button', { name: 'Remove role HR Admin' })).toHaveCount(0);
    await expect.poll(() => rolesOf(id)).toEqual(['User']);
    await dialog.getByRole('button', { name: 'Cancel' }).click();

    // The person loses the Control Hub and the admin pages.
    await member.goto('/admin/departments');
    await expect(member.getByRole('heading', { name: 'Access denied' })).toBeVisible();
    await member.goto('/');
    await openUserMenu(member);
    await expect(member.getByRole('menuitem', { name: 'Control Hub' })).toHaveCount(0);
    await context.close();
  });
});
