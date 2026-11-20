// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { randomUUID } from 'node:crypto';
import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import {
  addDepartmentMember,
  createDepartment,
  departmentTreeItem,
  findUserRow,
  openOrganization,
  openUserMenu,
  openUsersAdmin,
  provisionPerson,
  selectDepartment,
  sendChat,
  setDepartmentBudget,
  suffix,
  userByEmail,
} from '../../support/users';

const messagesContaining = async (text: string) =>
  (await sql<{ n: string }>('SELECT count(*) AS n FROM messages WHERE "messageContent" LIKE $1', [`%${text}%`]))[0].n;
const budgetOf = async (name: string) =>
  (
    await sql<{ allocated: string; action: string; period: string }>(
      'SELECT "budgetAllocated"::text AS allocated, "actionOnExceed" AS action, "budgetPeriod" AS period FROM departments WHERE name = $1',
      [name],
    )
  )[0];
const usageRows = async (userId: string) =>
  Number((await sql<{ n: string }>('SELECT count(*) AS n FROM usage_logs WHERE "userId" = $1', [userId]))[0].n);

test.describe('departments and budgets', () => {
  test('a member is refused chat once the department budget is used up, and nothing reaches the model', async ({ page, browser }) => {
    test.setTimeout(180_000);
    const { person, context, page: member, id } = await provisionPerson(browser, 'spender');
    const department = `Budget team ${suffix()}`;

    // The owner creates the department, adds the person and gives it a blocking monthly budget.
    await createDepartment(page, department);
    await selectDepartment(page, department);
    await addDepartmentMember(page, person);
    await expect.poll(async () => (await userByEmail(person.email))?.department_id).toBeTruthy();
    await setDepartmentBudget(page, '1', 'Block');
    await expect.poll(() => budgetOf(department)).toEqual({ allocated: '1.00', action: 'block', period: 'monthly' });
    await openUsersAdmin(page);
    await expect((await findUserRow(page, person.email)).getByRole('cell').nth(3)).toHaveText(department);

    // Within budget the member chats normally.
    const allowed = `within budget ${suffix()}`;
    await sendChat(member, allowed);
    await expect(member.getByRole('log').getByText(`Mock reply: ${allowed}`)).toBeVisible();

    // The month's spend reaches the allocation. A dollar of mock replies is ~170,000 messages,
    // so the spend is recorded directly in the usage ledger the budget is computed from.
    await sql(
      `INSERT INTO usage_logs (id, identifier, "userId", "modelProvider", "modelName", "requestTokens",
         "responseTokens", "totalTokens", "costUsd", "latencyMs", timestamp, status)
       VALUES ($1, $2, $3, 'e2e-mock', 'e2e-chat', 1000, 1000, 2000, 1.00, 10, now(), 'success')`,
      [randomUUID(), `e2e-users-${suffix()}`, id],
    );
    const usageBefore = await usageRows(id);

    const refused = `over budget ${suffix()}`;
    await sendChat(member, refused);
    await expect(member.getByRole('alert').filter({ hasText: /department budget is exhausted/i })).toBeVisible();
    await expect(member.getByText(`Mock reply: ${refused}`)).toHaveCount(0);
    // Refused before anything was stored or sent: no message rows, no model usage.
    expect(await messagesContaining(refused)).toBe('0');
    expect(await usageRows(id)).toBe(usageBefore);

    // Raising the budget lets the member chat again.
    await openOrganization(page);
    await selectDepartment(page, department);
    await setDepartmentBudget(page, '5', 'Block');
    await expect.poll(() => budgetOf(department)).toEqual({ allocated: '5.00', action: 'block', period: 'monthly' });
    const restored = `budget raised ${suffix()}`;
    await sendChat(member, restored);
    await expect(member.getByRole('log').getByText(`Mock reply: ${restored}`)).toBeVisible();
    await context.close();
  });

  test('the budget panel shows the exceed action and keeps it when the budget is edited again', async ({ page }) => {
    // Known bug (API/webapp contract): the Department returned by GET /me/administered-departments
    // and PUT /admin/departments/{id} (src/dto/admin_department.rs `Department`) has no
    // action_on_exceed, while BudgetManagement.svelte reads department.action_on_exceed for the
    // "Action on Exceed" badge and to prefill the Edit Budget dialog (`|| 'warn'`). The badge never
    // shows, and re-opening Edit Budget on a Block department preselects Warn, so saving any other
    // change (e.g. a new amount) silently turns blocking off. The database keeps 'block' until then.
    // Remove `fail` once the department payload carries action_on_exceed.
    test.fail();
    const department = `Exceed action ${suffix()}`;
    await createDepartment(page, department);
    await selectDepartment(page, department);
    await setDepartmentBudget(page, '2', 'Block');
    await expect.poll(() => budgetOf(department)).toEqual({ allocated: '2.00', action: 'block', period: 'monthly' });
    await page.getByRole('button', { name: 'Edit Budget' }).click();
    const dialog = page.getByRole('dialog', { name: 'Edit Budget' });
    await expect(dialog.getByLabel('Action on Exceed')).toHaveValue('block', { timeout: 5_000 });
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(page.getByText('Action on Exceed: Block')).toBeVisible({ timeout: 5_000 });
  });

  test('a department admin sees only their own department in the Control Hub', async ({ page, browser }) => {
    test.setTimeout(240_000);
    const admin = await provisionPerson(browser, 'teamlead');
    const insider = await provisionPerson(browser, 'insider');
    const outsider = await provisionPerson(browser, 'outsider');
    const own = `Lead team ${suffix()}`;
    const other = `Other team ${suffix()}`;

    // The owner sets up two departments and makes the team lead admin of the first one.
    await createDepartment(page, own);
    await selectDepartment(page, own);
    await addDepartmentMember(page, insider.person);
    await page.getByRole('tab', { name: 'Overview' }).click();
    await page.getByRole('button', { name: 'Add Admin' }).click();
    await page.getByPlaceholder('Search users to add as admin...').fill(admin.person.email);
    await page.getByRole('button', { name: new RegExp(admin.person.email.replace(/[.+]/g, '\\$&')) }).click();
    await expect(page.getByRole('status').filter({ hasText: 'Admin added successfully' })).toBeVisible();
    await createDepartment(page, other);
    await selectDepartment(page, other);
    await addDepartmentMember(page, outsider.person);

    // The team lead now has a Control Hub scoped to their department.
    const lead = admin.page;
    await lead.goto('/');
    await openUserMenu(lead);
    await lead.getByRole('menuitem', { name: 'Control Hub' }).click();
    await lead.waitForURL(/\/admin\//);

    await openOrganization(lead);
    await lead.getByRole('tab', { name: 'Teams' }).click();
    await expect(departmentTreeItem(lead, own)).toBeVisible();
    await expect(departmentTreeItem(lead, other)).toHaveCount(0);
    await lead.getByRole('textbox', { name: 'Find a department...' }).fill(other);
    await expect(lead.getByText('No departments match your search')).toBeVisible();

    await lead.getByRole('tab', { name: 'Users' }).click();
    const search = lead.getByRole('textbox', { name: 'Search by name, email, or department...' });
    await search.fill(insider.person.email);
    await expect(lead.getByRole('row').filter({ hasText: insider.person.email })).toHaveCount(1);
    await search.fill(outsider.person.email);
    await expect(lead.getByText('No users found')).toBeVisible();
    await expect(lead.getByRole('row').filter({ hasText: outsider.person.email })).toHaveCount(0);

    // Usage analytics is limited to the department as well.
    await lead.goto('/admin/analytics');
    await expect(lead.getByRole('tab', { name: 'Overview' })).toHaveCount(0);
    await lead.getByRole('tab', { name: 'Usage breakdown by department' }).click();
    await expect(lead.getByRole('row').filter({ hasText: own })).toHaveCount(1);
    await expect(lead.getByRole('row').filter({ hasText: other })).toHaveCount(0);
    await lead.getByRole('tab', { name: 'Usage breakdown by user' }).click();
    await expect(lead.getByRole('row').filter({ hasText: insider.person.email })).toHaveCount(1);
    await expect(lead.getByRole('row').filter({ hasText: outsider.person.email })).toHaveCount(0);

    for (const person of [admin, insider, outsider]) await person.context.close();
  });
});
