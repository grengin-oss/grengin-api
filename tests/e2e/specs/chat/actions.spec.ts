// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import { assistantMessage, composer, conversation, currentChatId, openNewChat, send, unique, userMessage } from '../../support/chat';

test.describe('chat message actions', () => {
  test.beforeEach(async ({ page }) => {
    await openNewChat(page);
  });

  test('Copy content copies the answer and the prompt to the clipboard', async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    const prompt = unique('copy-me');
    await send(page, prompt);
    const answer = assistantMessage(page, `Mock reply: ${prompt}`);
    await expect(answer).toBeVisible();

    await answer.getByRole('button', { name: 'Copy content' }).click();
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(`Mock reply: ${prompt}`);

    const question = userMessage(page, prompt);
    await question.hover();
    await question.getByRole('button', { name: 'Copy content' }).click();
    await expect.poll(() => page.evaluate(() => navigator.clipboard.readText())).toBe(prompt);
  });

  test('a text file attached to a message is uploaded, sent with it and shown on the message', async ({ page }) => {
    const name = `${unique('notes')}.txt`;
    const body = `${unique('file-body')}\nsecond line\n`;
    await page.getByRole('button', { name: 'Add content' }).click();
    const chooser = page.waitForEvent('filechooser');
    await page.getByRole('button', { name: /Add files or photos/ }).click();
    await (await chooser).setFiles({ name, mimeType: 'text/plain', buffer: Buffer.from(body) });

    // The pending attachment uploads at once (POST /files) and the composer waits for it.
    await expect(page.getByRole('button', { name: `Preview file: ${name}` })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Send message' })).toBeVisible();
    const [stored] = await sql<{ id: string; size: string; type: string }>(
      'SELECT id, size::text AS size, "contentType" AS type FROM files WHERE name = $1',
      [name],
    );
    expect(stored).toMatchObject({ size: String(Buffer.byteLength(body)), type: 'text/plain' });

    const prompt = unique('with-file');
    await send(page, prompt);
    await expect(assistantMessage(page, `Mock reply: ${prompt}`)).toBeVisible();
    const question = userMessage(page, prompt);
    await expect(question.getByText(name, { exact: true })).toBeVisible();
    await expect(question.getByRole('button', { name: 'Download file' })).toBeVisible();
    await expect(composer(page)).toHaveValue('');
    await expect(page.getByRole('button', { name: `Preview file: ${name}` })).toHaveCount(0);

    const chatId = await currentChatId(page);
    const [row] = await sql<{ files: Array<{ id: string; name: string }> }>(
      `SELECT metadata->'files' AS files FROM messages WHERE "conversationId" = $1 AND role::text = 'user'`,
      [chatId],
    );
    expect(row.files).toEqual([expect.objectContaining({ id: stored.id, name })]);
    const shown = await conversation(chatId);
    const userTurn = shown.json.messages.find((m: any) => m.role === 'user');
    expect(userTurn.parts.files).toEqual([expect.objectContaining({ id: stored.id, name })]);

    await page.reload();
    await expect(userMessage(page, prompt).getByText(name, { exact: true })).toBeVisible();
  });
});
