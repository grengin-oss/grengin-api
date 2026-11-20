// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import { dismissOnboarding } from '../../support/login';
import { createDepartment, deleteDepartment, openDepartment, ownerId, setDepartmentBudget, unique } from '../../support/admin';

test.describe('notifications', () => {
  const cleanup: string[] = [];

  test.afterEach(async () => {
    // Children first: a department with sub-departments cannot be deleted.
    while (cleanup.length) await deleteDepartment(cleanup.pop()!);
  });

  test('lowering a department budget below its sub-allocations alerts the owner, who reads it', async ({ page }) => {
    // A parent with $1000 and a child holding $90 of it. Lowering the parent to $100 in the UI
    // leaves $10 (10%) available, under the 20% threshold, which raises "Budget low".
    const parentName = unique('E2E Budget Parent');
    const parent = await createDepartment(parentName);
    cleanup.push(parent.id);
    await setDepartmentBudget(parent.id, 1000);
    const child = await createDepartment(unique('E2E Budget Child'), parent.id);
    cleanup.push(child.id);
    await setDepartmentBudget(child.id, 90);
    const title = `Budget low for ${parentName}`;
    const owner = await ownerId();

    const details = await openDepartment(page, parentName);
    await details.getByRole('tab', { name: 'Budget' }).click();
    await details.getByRole('button', { name: 'Edit Budget' }).click();
    const dialog = page.getByRole('dialog', { name: 'Edit Budget' });
    await dialog.getByLabel('Budget Amount in USD').fill('100');
    await dialog.getByRole('button', { name: 'Save Budget' }).click();
    await dialog.getByRole('button', { name: 'Confirm & Save' }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Budget updated successfully' })).toBeVisible();

    await expect
      .poll(async () => (await sql(`SELECT 1 FROM notifications WHERE "userId" = $1 AND title = $2`, [owner, title])).length)
      .toBe(1);

    // The bell in the chat sidebar carries an unread badge and lists the alert.
    await page.goto('/');
    await page.getByRole('textbox').first().waitFor();
    await dismissOnboarding(page);
    const bell = page.getByRole('button', { name: 'Open alerts' }).first();
    await expect(bell).toContainText(/\d+/);
    await bell.click();
    const popover = page.getByRole('dialog', { name: 'Notifications' });
    const item = popover.getByRole('button', { name: new RegExp(`^${title}: Department budget has fallen below 20%`) });
    await expect(item).toBeVisible();

    // Opening it lists the alert on the Alerts page, which marks what it shows as read.
    await item.click();
    await expect(page.getByRole('heading', { name: 'Alerts', level: 1 })).toBeVisible();
    const cardOnPage = page.getByLabel(new RegExp(`^${title}:`));
    await expect(cardOnPage).toBeVisible();
    await expect
      .poll(async () => {
        const [row] = await sql<{ readAt: Date | null }>(`SELECT "readAt" FROM notifications WHERE "userId" = $1 AND title = $2`, [owner, title]);
        return row?.readAt != null;
      })
      .toBe(true);

    // The open page keeps the lists it loaded; after a reload the read alert is no longer unread.
    await page.reload();
    await expect(page.getByLabel(new RegExp(`^${title}:`))).toBeVisible();
    await page.getByRole('button', { name: 'Unread', exact: true }).click();
    await expect(page.getByRole('button', { name: 'Unread', exact: true })).toHaveAttribute('aria-pressed', 'true');
    await expect(page.getByLabel(new RegExp(`^${title}:`))).toHaveCount(0);
    await page.getByRole('button', { name: /^All/ }).click();
    await expect(page.getByLabel(new RegExp(`^${title}:`))).toBeVisible();
  });
});
