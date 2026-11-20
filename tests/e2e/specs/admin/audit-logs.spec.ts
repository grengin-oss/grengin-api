// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { readFile } from 'node:fs/promises';
import { expect, test, type Page } from '@playwright/test';
import { sql } from '../../support/api';
import { createDepartment, deleteDepartment, IDP_SLUG, openDepartment, ownerId, unique } from '../../support/admin';
import { signIn } from '../../support/login';

const SHOW_DETAILS = 'Show event details';

async function renameDepartmentInUi(page: Page, from: string, to: string) {
  const details = await openDepartment(page, from);
  await details.getByRole('button', { name: 'Edit', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit Department' });
  await dialog.getByLabel('Name').fill(to);
  await dialog.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('treeitem', { name: new RegExp(`^${to},`) })).toBeVisible();
}

// Audit rows are written after the response, so the page may need a refresh to show them.
async function refreshUntilVisible(page: Page, row: ReturnType<Page['locator']>) {
  await expect(async () => {
    await page.getByRole('button', { name: 'Refresh' }).click();
    await expect(row).toBeVisible({ timeout: 2_000 });
  }).toPass({ timeout: 20_000 });
}

test.describe('audit logs', () => {
  let department: { id: string; name: string } | null = null;
  let owner = '';

  test.beforeEach(async () => {
    owner = await ownerId();
  });

  test.afterEach(async () => {
    if (department) await deleteDepartment(department.id);
    department = null;
  });

  test('an admin rename is listed with its actor, type and resource, and the filters narrow the list', async ({ page }) => {
    const before = unique('E2E Audit Dept');
    department = await createDepartment(before);
    const after = `${before} renamed`;
    await renameDepartmentInUi(page, before, after);

    await page.goto('/admin/audit-logs');
    await expect(page.getByText('Audit Logs', { exact: true }).first()).toBeVisible();
    await page.getByLabel('Action', { exact: true }).selectOption({ label: 'Department Updated' });
    await expect(page).toHaveURL(/action=admin_department_updated/);

    const row = page.getByRole('button', { name: SHOW_DETAILS }).filter({ hasText: department.id.slice(0, 8) });
    await refreshUntilVisible(page, row);
    // Type and actor on the row itself.
    await expect(row.getByText('Department Updated')).toBeVisible();
    await expect(row.getByRole('button', { name: `Filter by user ${owner}` })).toHaveText(owner.slice(0, 8));
    await expect(row).toContainText('department');
    // Every listed row is of the selected type.
    const rows = page.getByRole('button', { name: SHOW_DETAILS });
    const count = await rows.count();
    for (let i = 0; i < count; i++) await expect(rows.nth(i)).toContainText('Department Updated');

    // The payload drawer carries the full identities and the change.
    await row.click();
    await expect(page.getByRole('button', { name: `Copy ${owner}` })).toBeVisible();
    await expect(page.getByRole('button', { name: `Copy ${department.id}` })).toBeVisible();
    await page.getByRole('button', { name: 'JSON view' }).click();
    const payload = page.locator('pre').filter({ hasText: '"/admin/departments/{department_id}"' });
    await expect(payload).toContainText('"method": "PUT"');
    await expect(payload).toContainText('"status_code": 200');
    await expect(payload).toContainText(`"name": "${before}"`);
    await expect(payload).toContainText(`"name": "${after}"`);

    // A start date after today leaves nothing; clearing the filters brings the rows back.
    const tomorrow = new Date(Date.now() + 86_400_000).toISOString().slice(0, 10);
    await page.getByLabel('Start Date', { exact: true }).fill(tomorrow);
    await expect(page.getByText('No audit logs yet')).toBeVisible();
    await expect(page.getByRole('status').filter({ hasText: 'events matched' })).toHaveText('0 events matched');
    await page.getByRole('button', { name: 'Clear all' }).click();
    await expect(page).not.toHaveURL(/action=/);
    await expect(page.getByLabel('Action', { exact: true })).toHaveValue('');
    await expect(page.getByRole('button', { name: SHOW_DETAILS }).first()).toBeVisible();

    // The actor link narrows the list to that user.
    await page
      .getByRole('button', { name: `Filter by user ${owner}` })
      .first()
      .click();
    await expect(page.getByLabel('Search Event', { exact: true })).toHaveValue(owner);
    await expect(page).toHaveURL(new RegExp(`user=${owner}`));
    const userRows = page.getByRole('button', { name: SHOW_DETAILS });
    await expect(userRows.first()).toBeVisible();
    const users = page.locator('button[aria-label^="Filter by user "]');
    const shown = await users.count();
    for (let i = 0; i < shown; i++) await expect(users.nth(i)).toHaveAttribute('title', owner);
  });

  test('Search Event finds a user by the short id the table shows', async ({ page }) => {
    // Known bug: the field's placeholder ("e.g. 6bc782b8…") and the table both use the
    // 8-character id, but the value is sent as `userId`, which GET /admin/audit-logs parses as a
    // full UUID, so a short id fails the whole request (400) and the page shows an error toast
    // instead of the user's events. Remove `fail` once the search accepts what the table shows.
    test.fail();
    await page.goto('/admin/audit-logs');
    await expect(page.getByRole('button', { name: SHOW_DETAILS }).first()).toBeVisible();
    await page.getByLabel('Search Event', { exact: true }).fill(owner.slice(0, 8));
    await expect(page.getByRole('alert')).toHaveCount(0, { timeout: 3_000 });
    await expect(page.locator(`button[aria-label="Filter by user ${owner}"]`).first()).toBeVisible();
  });

  test('CSV and JSON exports download the filtered events', async ({ page }) => {
    const name = unique('E2E Export Dept');
    department = await createDepartment(name);
    await renameDepartmentInUi(page, name, `${name} v2`);

    await page.goto('/admin/audit-logs?action=admin_department_updated');
    const row = page.getByRole('button', { name: SHOW_DETAILS }).filter({ hasText: department.id.slice(0, 8) });
    await refreshUntilVisible(page, row);

    const csvDownload = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Export CSV' }).click();
    const csv = await csvDownload;
    expect(csv.suggestedFilename()).toMatch(/^audit-logs-\d{4}-\d{2}-\d{2}\.csv$/);
    const csvText = await readFile((await csv.path())!, 'utf8');
    const [header, ...lines] = csvText.trim().split('\n');
    expect(header).toBe('id,user_id,action,resource_type,resource_id,ip_address,user_agent,created_at,details');
    expect(lines.some((line) => line.includes(department!.id) && line.includes(owner))).toBe(true);
    for (const line of lines.filter((l) => /^[0-9a-f-]{36},/.test(l))) expect(line).toContain(',admin_department_updated,');
    await expect(page.getByRole('status').filter({ hasText: 'Audit logs exported successfully' })).toBeVisible();

    const jsonDownload = page.waitForEvent('download');
    await page.getByRole('button', { name: 'Export JSON' }).click();
    const json = await jsonDownload;
    expect(json.suggestedFilename()).toMatch(/^audit-logs-\d{4}-\d{2}-\d{2}\.json$/);
    const entries = JSON.parse(await readFile((await json.path())!, 'utf8')) as Array<Record<string, any>>;
    expect(entries.length).toBeGreaterThan(0);
    expect(entries.every((entry) => entry.action === 'admin_department_updated')).toBe(true);
    const mine = entries.find((entry) => entry.resourceId === department!.id);
    expect(mine).toMatchObject({ userId: owner, action: 'admin_department_updated', resourceType: 'department' });
  });

  test('a sign-in is recorded as a login by the user who signed in', async ({ browser }) => {
    const context = await browser.newContext({ storageState: { cookies: [], origins: [] } });
    try {
      const page = await context.newPage();
      const since = new Date();
      await signIn(page, 'owner');
      await page.getByRole('textbox').first().waitFor();

      await expect
        .poll(async () => (await sql(`SELECT 1 FROM audit_logs WHERE action = 'login' AND "createdAt" >= $1`, [since])).length)
        .toBeGreaterThan(0);

      await page.goto('/admin/audit-logs?action=login');
      const newest = page.getByRole('button', { name: SHOW_DETAILS }).first();
      await expect(newest).toBeVisible();
      await expect(newest.getByText('Login', { exact: true })).toBeVisible();
      await expect(newest).toContainText(IDP_SLUG);

      // Known bug: the audit middleware takes the actor from the request's bearer token, and
      // GET /auth/{provider}/callback has none, so every login row is stored with userId NULL
      // and the table shows "—" instead of who signed in. Remove `fail` once logins carry the user.
      test.fail();
      await expect(newest.getByRole('button', { name: `Filter by user ${await ownerId()}` })).toBeVisible({ timeout: 5_000 });
    } finally {
      await context.close();
    }
  });
});
