// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Refreshing while a reply streams (grengin-api PR #100): the server keeps generating and saves the
// reply complete; GET /chat/{id} marks it `generating: true` until then. Webapp `main` does not yet
// follow such a reply after a reload (no polling, no Stop) — see
// grengin-docs/chat-reply-after-refresh-frontend.md — so those UI checks are expected failures.
import { expect, test, type Page } from '@playwright/test';
import { sql } from '../../support/api';
import { assistantMessage, conversation, currentChatId, openNewChat, send, transcript, unique } from '../../support/chat';

const SLOW = 30;

type Reply = { id: string; content: string; stop: string | null };
async function reply(chatId: string): Promise<Reply | undefined> {
  const [row] = await sql<Reply>(
    `SELECT id, "messageContent" AS content, metadata->>'stopReason' AS stop FROM messages
     WHERE "conversationId" = $1 AND role::text = 'assistant' ORDER BY "createdAt" DESC LIMIT 1`,
    [chatId],
  );
  return row;
}

// Starts a slow reply, waits until it is visibly streaming, then reloads the page mid-stream.
async function reloadMidStream(page: Page, prompt: string) {
  await send(page, `${prompt} [slow:${SLOW}]`);
  await expect(transcript(page).getByText(/part2 /)).toBeVisible();
  const chatId = await currentChatId(page);
  await page.reload();
  return chatId;
}

const lastPart = new RegExp(`part${SLOW}\\b`);

test.describe('refreshing while a reply is generating', () => {
  test.beforeEach(async ({ page }) => {
    await openNewChat(page);
  });

  test('the server finishes the reply after the page is reloaded, and reopening shows it complete', async ({ page }) => {
    const prompt = unique('refresh-keeps');
    const chatId = await reloadMidStream(page, prompt);

    // Right after the reload the reply is still being generated on the server.
    const during = await conversation(chatId);
    const live = during.json.messages.find((m: any) => m.role === 'assistant');
    expect(live?.generating).toBe(true);
    expect(live?.parts.text ?? '').not.toMatch(lastPart);

    await expect.poll(async () => (await reply(chatId))?.content ?? '', { timeout: 30_000 }).toMatch(lastPart);
    const done = await reply(chatId);
    expect(done?.content.trim()).toBe(Array.from({ length: SLOW }, (_, i) => `part${i + 1}`).join(' '));
    expect(done?.stop).toBeNull();
    await expect
      .poll(async () => (await conversation(chatId)).json.messages.find((m: any) => m.role === 'assistant')?.generating, {
        timeout: 10_000,
      })
      .toBe(false);
    const final = (await conversation(chatId)).json.messages.find((m: any) => m.role === 'assistant');
    expect(final.stop_reason).toBeUndefined();

    await page.reload();
    await expect(assistantMessage(page, lastPart)).toBeVisible();
    await expect(page.getByText('The response was interrupted.')).toHaveCount(0);
  });

  test('after a reload the reply keeps growing on screen until it is complete', async ({ page }) => {
    test.fail(
      true,
      'Webapp main renders the saved partial text once and never re-fetches while `generating` is true ' +
        '(grengin-docs/chat-reply-after-refresh-frontend.md, "What the frontend should do" 1-3).',
    );
    const prompt = unique('refresh-follows');
    const chatId = await reloadMidStream(page, prompt);
    await expect(assistantMessage(page, /part1 /)).toBeVisible();
    await expect.poll(async () => (await reply(chatId))?.content ?? '', { timeout: 30_000 }).toMatch(lastPart);
    // The server has finished; the open page should have followed it without another reload.
    await expect(assistantMessage(page, lastPart)).toBeVisible({ timeout: 5_000 });
  });

  test('after a reload Stop is offered and cancels the reply on the server', async ({ page }) => {
    test.fail(
      true,
      'Webapp main offers Stop only for a stream open in the same tab; after a reload it does not call ' +
        'POST /chat/stream/{message_id}/cancel (grengin-docs/chat-reply-after-refresh-frontend.md, step 5).',
    );
    const prompt = unique('refresh-stop');
    const chatId = await reloadMidStream(page, prompt);
    const stop = page.getByRole('button', { name: 'Stop generating' });
    try {
      await expect(stop).toBeVisible({ timeout: 5_000 });
      await stop.click();
      await expect.poll(async () => (await reply(chatId))?.stop ?? null).toBe('user_cancelled');
      await expect(assistantMessage(page, /part1 /)).toBeVisible();
      await expect(assistantMessage(page, lastPart)).toHaveCount(0);
      await expect(page.getByText('The response was interrupted.')).toBeVisible();
    } finally {
      // Let the generation the UI could not stop end before the next test.
      await expect.poll(async () => (await conversation(chatId)).json.messages.at(-1)?.generating, { timeout: 30_000 }).toBe(false);
    }
  });
});
