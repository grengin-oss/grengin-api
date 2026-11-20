// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Uses a second mock model (mocks/service-chat-llm.mjs) that answers with what the backend sent
// it, so these specs check the context the model receives, not only what the webapp shows.
import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import {
  CONTEXT_MODEL,
  CONTEXT_MODEL_NAME,
  assistantMessage,
  conversation,
  currentChatId,
  disableContextEngine,
  enableContextEngine,
  messages,
  openNewChat,
  selectModel,
  send,
  sidebarChat,
  transcript,
  unique,
  userMessage,
} from '../../support/chat';

test.describe('chat context and artifacts', () => {
  test.beforeAll(async () => {
    await enableContextEngine();
  });
  test.afterAll(async () => {
    await disableContextEngine();
  });

  test.beforeEach(async ({ page }) => {
    await openNewChat(page);
    await selectModel(page, CONTEXT_MODEL_NAME, CONTEXT_MODEL);
  });

  test('a multi-turn conversation sends the earlier turns in order, and reopening it shows the whole history', async ({ page }) => {
    const suffix = unique('turns').split('-').at(-1);
    const turns = [1, 2, 3].map((n) => `ctx${n}-${suffix} question ${n}`);
    const token = (n: number) => `ctx${n}-${suffix}`;
    const expected = [
      `Context: ${token(1)} | assistant turns: 0`,
      `Context: ${token(1)} > ${token(2)} | assistant turns: 1`,
      `Context: ${token(1)} > ${token(2)} > ${token(3)} | assistant turns: 2`,
    ];
    for (const [i, turn] of turns.entries()) {
      await send(page, turn);
      await expect(assistantMessage(page, expected[i])).toBeVisible();
    }
    const chatId = await currentChatId(page);
    const transcriptOrder = [turns[0], expected[0], turns[1], expected[1], turns[2], expected[2]];
    await expect(messages(page)).toHaveText(transcriptOrder.map((text) => new RegExp(escape(text))));

    const rows = await sql<{ role: string; content: string }>(
      `SELECT role::text AS role, "messageContent" AS content FROM messages
       WHERE "conversationId" = $1 AND deleted = false ORDER BY "createdAt"`,
      [chatId],
    );
    expect(rows.map((r) => r.content)).toEqual(transcriptOrder);
    expect(rows.map((r) => r.role)).toEqual(['user', 'assistant', 'user', 'assistant', 'user', 'assistant']);

    // Leave for a new chat, then come back through the sidebar.
    await page.getByRole('button', { name: 'New chat' }).first().click();
    await expect(page).not.toHaveURL(/chatId=/);
    await expect(userMessage(page, turns[0])).toHaveCount(0);
    const title = `Title: ${turns[0]}`;
    await sidebarChat(page, title).getByRole('button', { name: title, exact: true }).click();
    await expect(page).toHaveURL(new RegExp(`chatId=${chatId}`));
    await expect(messages(page)).toHaveText(transcriptOrder.map((text) => new RegExp(escape(text))));

    await page.reload();
    await expect(messages(page)).toHaveText(transcriptOrder.map((text) => new RegExp(escape(text))));
    const shown = await conversation(chatId);
    expect(shown.json.messages.map((m: any) => m.parts.text)).toEqual(transcriptOrder);
  });

  test('an attached text file reaches the model with the message', async ({ page }) => {
    const name = `${unique('context-file')}.txt`;
    const firstLine = unique('file-line');
    await page.getByRole('button', { name: 'Add content' }).click();
    const chooser = page.waitForEvent('filechooser');
    await page.getByRole('button', { name: /Add files or photos/ }).click();
    await (await chooser).setFiles({ name, mimeType: 'text/plain', buffer: Buffer.from(`${firstLine}\nmore\n`) });
    await expect(page.getByRole('button', { name: 'Send message' })).toBeVisible();

    const suffix = unique('file').split('-').at(-1);
    await send(page, `ctx1-${suffix} read the file`);
    await expect(assistantMessage(page, `Context: ctx1-${suffix} | assistant turns: 0 | files: ${name}=${firstLine}`)).toBeVisible();
    await expect(userMessage(page, `ctx1-${suffix}`).getByText(name, { exact: true })).toBeVisible();

    // A follow-up turn still carries the file from the earlier turn.
    await send(page, `ctx2-${suffix} and again`);
    await expect(
      assistantMessage(page, `Context: ctx1-${suffix} > ctx2-${suffix} | assistant turns: 1 | files: ${name}=${firstLine}`),
    ).toBeVisible();
  });

  test('an artifact in the reply opens in the side panel, is saved, and reopens after a reload', async ({ page }) => {
    const title = unique('E2E Notes');
    await send(page, `write notes [artifact:${title}]`);

    const card = transcript(page).getByRole('button', { name: new RegExp(`${escape(title)}.*Click to open preview`) });
    await expect(card).toBeVisible();
    await expect(assistantMessage(page, 'Here is the requested note.')).toBeVisible();
    await expect(transcript(page).getByText('<artifact')).toHaveCount(0);
    await expect(transcript(page).getByText(`Heading ${title}`)).toHaveCount(0);

    // The first artifact of a reply opens the preview panel by itself.
    const close = page.getByRole('button', { name: 'Close panel' });
    await expect(close).toBeVisible();
    await expect(page.getByRole('heading', { name: `Heading ${title}` })).toBeVisible();

    const chatId = await currentChatId(page);
    let saved: Array<{ id: string; file: string; type: string; message: string }> = [];
    await expect
      .poll(async () => {
        saved = await sql<{ id: string; file: string; type: string; message: string }>(
          `SELECT a.id, a."fileId" AS file, a."contentType" AS type, a."messageId" AS message
           FROM artifacts a WHERE a."conversationId" = $1 AND a.title = $2`,
          [chatId, title],
        );
        return saved.length;
      })
      .toBe(1);
    expect(saved[0].type).toBe('text/markdown');
    const [file] = await sql<{ size: string }>('SELECT size::text AS size FROM files WHERE id = $1', [saved[0].file]);
    expect(Number(file.size)).toBeGreaterThan(0);
    const shown = await conversation(chatId);
    const answer = shown.json.messages.find((m: any) => m.role === 'assistant');
    expect(answer.id).toBe(saved[0].message);
    expect(answer.parts.artifacts).toEqual([expect.objectContaining({ id: saved[0].id, title, content_type: 'text/markdown' })]);
    expect(answer.parts.text).not.toContain('<artifact');

    await close.click();
    await expect(close).toHaveCount(0);
    await page.reload();
    await expect(card).toBeVisible();
    await expect(close).toHaveCount(0);
    await card.click();
    await expect(page.getByRole('heading', { name: `Heading ${title}` })).toBeVisible();
  });
});

function escape(text: string) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
