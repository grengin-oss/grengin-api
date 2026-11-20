// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test, type Page } from '@playwright/test';
import { USERS } from '../../mocks/users.mjs';
import { sql } from '../../support/api';
import { createDepartment, deleteDepartment, ownerId, unique } from '../../support/admin';
import { dismissOnboarding } from '../../support/login';

// The mock LLM reports 12 prompt + 7 completion tokens per call; the e2e engine is priced from
// the catalog's DeepSeek manifest ($0.15 input / $0.60 output per million tokens).
const PROMPT_TOKENS = 12;
const COMPLETION_TOKENS = 7;
const COST_PER_CALL = (PROMPT_TOKENS * 0.15 + COMPLETION_TOKENS * 0.6) / 1_000_000;

// The page's default range is "Last 7 days": today-7 .. today (UTC dates, both ends whole days).
const WINDOW = `m.deleted = false AND m."createdAt" >= (CURRENT_DATE - 7)::timestamptz AND m."createdAt" < (CURRENT_DATE + 1)::timestamptz`;

const compact = (n: number) => {
  if (n >= 1_000_000) return `${(n / 1_000_000) % 1 === 0 ? n / 1_000_000 : (n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000) % 1 === 0 ? n / 1_000 : (n / 1_000).toFixed(1)}K`;
  return String(Math.round(n));
};
const grouped = (n: number) => new Intl.NumberFormat('en-US').format(n);
const dollars = (n: number) => `$${n.toFixed(2)}`;

type Usage = { requests: number; tokens: number; cost: number };

async function usage(userId?: string): Promise<Usage> {
  const [row] = await sql<{ requests: string; tokens: string; cost: string }>(
    `SELECT count(*) FILTER (WHERE m.role = 'user') AS requests, coalesce(sum(m."totalTokens"), 0) AS tokens, coalesce(sum(m.cost), 0) AS cost
     FROM messages m JOIN conversations c ON c.id = m."conversationId"
     WHERE ${WINDOW} ${userId ? 'AND c."userId" = $1' : ''}`,
    userId ? [userId] : [],
  );
  return { requests: Number(row.requests), tokens: Number(row.tokens), cost: Number(row.cost) };
}

async function chat(page: Page, text: string) {
  await page.goto('/');
  const composer = page.getByRole('textbox').first();
  await composer.waitFor();
  await dismissOnboarding(page);
  await composer.fill(text);
  await composer.press('Enter');
  await expect(page.getByRole('log').getByText(`Mock reply: ${text}`)).toBeVisible();
}

function statCard(page: Page, title: string) {
  return page.locator('.stat-card').filter({ has: page.getByText(title, { exact: true }) }).locator('.stat-value');
}

test.describe('usage analytics', () => {
  let department: { id: string; name: string } | null = null;

  test.afterEach(async () => {
    if (department) await deleteDepartment(department.id);
    department = null;
  });

  test('a chat with the mock engine is priced from the manifest and counted for the owner', async ({ page }) => {
    const owner = await ownerId();
    const text = unique('analytics probe');
    await chat(page, text);

    // The reply is stored with the mock's token usage and the manifest price.
    let stored: { request: number; response: number; total: number; cost: string } | undefined;
    await expect
      .poll(async () => {
        [stored] = await sql<{ request: number; response: number; total: number; cost: string }>(
          `SELECT "requestTokens" AS request, "responseTokens" AS response, "totalTokens" AS total, cost::text AS cost
           FROM messages WHERE role = 'assistant' AND "messageContent" = $1`,
          [`Mock reply: ${text}`],
        );
        return stored?.total ?? 0;
      })
      .toBe(PROMPT_TOKENS + COMPLETION_TOKENS);
    expect(stored).toMatchObject({ request: PROMPT_TOKENS, response: COMPLETION_TOKENS });
    expect(Number(stored!.cost)).toBeCloseTo(COST_PER_CALL, 12);

    await page.goto('/admin/analytics');
    await expect(page.getByRole('tab', { name: 'Overview usage metrics and trends' })).toHaveAttribute('aria-selected', 'true');
    const all = await usage();
    expect(all.requests).toBeGreaterThan(0);
    await expect(statCard(page, 'Total Requests')).toHaveText(compact(all.requests));
    await expect(statCard(page, 'Total Tokens')).toHaveText(compact(all.tokens));
    await expect(statCard(page, 'Total Cost')).toHaveText(dollars(all.cost));

    // By Users: the owner's row matches the owner's stored usage.
    await page.getByRole('tab', { name: 'Usage breakdown by user' }).click();
    await page.getByPlaceholder('Search by name, email, or department...').fill(USERS.owner.email);
    const table = page.getByRole('table', { name: 'User usage for the selected date range' });
    const row = table.getByRole('row').filter({ hasText: USERS.owner.email });
    await expect(row).toHaveCount(1);
    const mine = await usage(owner);
    expect(mine.requests).toBeGreaterThan(0);
    const cells = row.getByRole('cell');
    await expect(cells.nth(0)).toHaveText(USERS.owner.name);
    await expect(cells.nth(1)).toHaveText(USERS.owner.email);
    await expect(cells.nth(3)).toHaveText(grouped(mine.requests));
    await expect(cells.nth(6)).toHaveText(compact(mine.tokens));
    await expect(cells.nth(7)).toHaveText(dollars(mine.cost));

    // By Model: the e2e model is listed with requests in the range.
    await page.getByRole('tab', { name: 'Usage breakdown by model' }).click();
    const models = page.getByRole('table', { name: 'Model usage for the selected date range' });
    await expect(models.getByRole('row').filter({ hasText: 'e2e-chat' })).toHaveCount(1);
  });

  test('departments are listed with their usage on the By Department tab', async ({ page }) => {
    department = await createDepartment(unique('E2E Analytics Dept'));
    await page.goto('/admin/analytics');
    await page.getByRole('tab', { name: 'Usage breakdown by department' }).click();
    await page.getByPlaceholder('Search by department name...').fill(department.name);
    const table = page.getByRole('table', { name: 'Department usage for the selected date range' });
    const row = table.getByRole('row').filter({ hasText: department.name });
    await expect(row).toHaveCount(1);
    const cells = row.getByRole('cell');
    // A new department has no members and no usage yet.
    await expect(cells.nth(1)).toHaveText('0');
    await expect(cells.nth(2)).toHaveText('0');
    await expect(cells.nth(5)).toHaveText('0');
    await expect(cells.nth(6)).toHaveText('$0.00');
  });
});
