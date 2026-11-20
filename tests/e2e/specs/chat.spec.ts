// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../support/api';
import { dismissOnboarding } from '../support/login';

test.describe('chat', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    await page.getByRole('textbox').first().waitFor();
    await dismissOnboarding(page);
  });

  test('a message streams a reply from the configured engine and is saved', async ({ page }) => {
    const composer = page.getByRole('textbox').first();
    await composer.fill('hello from playwright');
    await composer.press('Enter');
    const messages = page.getByRole('log');
    await expect(messages.getByText('Mock reply: hello from playwright')).toBeVisible();
    // The generated title shows in the sidebar.
    await expect(page.getByText('Title: hello from playwright').first()).toBeVisible();

    const rows = await sql<{ content: string }>(
      `SELECT "messageContent" AS content FROM messages WHERE role = 'assistant' AND "messageContent" LIKE 'Mock reply: hello from playwright%'`,
    );
    expect(rows.length).toBeGreaterThan(0);
  });

  test('Stop ends a long reply, keeps the text so far and records why', async ({ page }) => {
    const composer = page.getByRole('textbox').first();
    await composer.fill('count slowly [slow:40]');
    await composer.press('Enter');
    const messages = page.getByRole('log');
    await expect(messages.getByText(/part2/)).toBeVisible();
    await page.getByRole('button', { name: /stop generating/i }).click();
    await expect(page.getByRole('button', { name: /stop generating/i })).toHaveCount(0);
    await expect(messages.getByText(/part1/)).toBeVisible();
    await expect(messages.getByText(/part40/)).toHaveCount(0);

    await expect
      .poll(async () => {
        const [row] = await sql<{ reason: string | null }>(
          `SELECT metadata->>'stopReason' AS reason FROM messages
           WHERE role = 'assistant' AND "messageContent" LIKE 'part1%' ORDER BY "createdAt" DESC LIMIT 1`,
        );
        return row?.reason ?? null;
      })
      .toBe('user_cancelled');
  });
});
