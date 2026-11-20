// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import { MOCK_ENGINE, openNewChat, selectModel, send } from '../../support/engines';

test('a provider failure shows a clear error and saves no completed reply', async ({ page }) => {
  const id = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
  const prompt = `engines provider failure ${id} [fail]`;
  await openNewChat(page);
  await selectModel(page, MOCK_ENGINE.modelName, MOCK_ENGINE.model);
  await send(page, prompt);

  const log = page.getByRole('log');
  await expect(log.getByText(prompt)).toBeVisible();
  // The API's error names the provider (ChatStreamError::ProviderError description).
  await expect(log.getByText(new RegExp(`Error from ${MOCK_ENGINE.key}`))).toBeVisible();
  await expect(page.getByRole('button', { name: /stop generating/i })).toHaveCount(0);
  await expect(log.getByText(/Mock reply:/)).toHaveCount(0);
  await expect(page.getByRole('textbox', { name: 'Message input' })).toBeEditable();

  const [user] = await sql<{ conversation: string }>(
    `SELECT "conversationId" AS conversation FROM messages WHERE role = 'user' AND "messageContent" = $1`,
    [prompt],
  );
  expect(user).toBeTruthy();
  const replies = () =>
    sql<{ content: string; reason: string | null }>(
      `SELECT "messageContent" AS content, metadata->>'stopReason' AS reason FROM messages
       WHERE role = 'assistant' AND "conversationId" = $1`,
      [user.conversation],
    );
  await expect.poll(replies).toEqual([{ content: '', reason: 'provider_error' }]);
});
