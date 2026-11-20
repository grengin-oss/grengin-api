// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../support/api';

test.describe('chat', () => {
  test('a message streams a reply from the configured engine and is saved', async ({ page }) => {
    await page.goto('/');
    const composer = page.getByRole('textbox').first();
    await composer.fill('hello from playwright');
    await composer.press('Enter');
    await expect(page.getByText('Mock reply: hello from playwright')).toBeVisible();

    const rows = await sql<{ content: string }>(
      `SELECT "messageContent" AS content FROM messages WHERE role = 'assistant' AND "messageContent" LIKE 'Mock reply: hello from playwright%'`,
    );
    expect(rows.length).toBeGreaterThan(0);
  });

  test('Stop ends a long reply and keeps the text so far', async ({ page }) => {
    await page.goto('/');
    const composer = page.getByRole('textbox').first();
    await composer.fill('count slowly [slow:40]');
    await composer.press('Enter');
    await expect(page.getByText(/part2/)).toBeVisible();
    await page.getByRole('button', { name: /stop/i }).click();
    await expect(page.getByText(/part40/)).toHaveCount(0, { timeout: 5_000 });
    await expect(page.getByText(/part1/)).toBeVisible();
  });
});
