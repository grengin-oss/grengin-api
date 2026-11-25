// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Refreshing while a reply streams (grengin-api PR #100): the server keeps generating and saves the
// reply complete; GET /chat/{id} marks it `generating: true` until then, and the webapp polls it
// (webapp #113). An edit's reply keeps generating too and is shown as the edited version.
import { expect, test, type Page } from '@playwright/test';
import { sql } from '../../support/api';
import { assistantMessage, conversation, currentChatId, openNewChat, send, transcript, unique, userMessage } from '../../support/chat';

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
    const prompt = unique('refresh-follows');
    const chatId = await reloadMidStream(page, prompt);
    await expect(assistantMessage(page, /part1 /)).toBeVisible();
    await expect.poll(async () => (await reply(chatId))?.content ?? '', { timeout: 30_000 }).toMatch(lastPart);
    // The server has finished; the open page should have followed it without another reload.
    await expect(assistantMessage(page, lastPart)).toBeVisible({ timeout: 5_000 });
  });

  test('after a reload Stop is offered and cancels the reply on the server', async ({ page }) => {
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

  test('an edited message keeps replying after a reload and becomes the shown version', async ({ page }) => {
    const original = unique('refresh-edit-original');
    const edited = `${unique('refresh-edit-new')} [slow:${SLOW}]`;
    await send(page, original);
    await expect(assistantMessage(page, `Mock reply: ${original}`)).toBeVisible();
    const chatId = await currentChatId(page);

    await userMessage(page, original).hover();
    await userMessage(page, original).getByRole('button', { name: 'Edit message' }).click();
    const editor = transcript(page).getByRole('textbox', { name: 'Edit message' });
    await editor.fill(edited);
    await transcript(page).getByRole('button', { name: 'Save', exact: true }).click();
    await expect(transcript(page).getByText(/part2 /)).toBeVisible();
    await page.reload();

    // Right after the reload the API shows the edit, still generating, in place of the original.
    const during = (await conversation(chatId)).json.messages;
    expect(during.find((m: any) => m.role === 'user')?.parts.text).toBe(edited);
    expect(during.find((m: any) => m.role === 'assistant')?.generating).toBe(true);

    // The reloaded page follows the edited reply until it is complete, without another reload.
    await expect(userMessage(page, edited)).toBeVisible();
    await expect(assistantMessage(page, lastPart)).toBeVisible({ timeout: 30_000 });
    await expect(userMessage(page, original)).toHaveCount(0);

    await expect
      .poll(async () => (await conversation(chatId)).json.messages.at(-1)?.generating, { timeout: 10_000 })
      .toBe(false);
    const final = (await conversation(chatId)).json.messages;
    const user = final.find((m: any) => m.role === 'user');
    expect(user.parts.text).toBe(edited);
    expect(user.versions).toMatchObject({ current: 2, total: 2 });
    expect(final.find((m: any) => m.role === 'assistant')?.parts.text.trim()).toBe(
      Array.from({ length: SLOW }, (_, i) => `part${i + 1}`).join(' '),
    );
  });
});
