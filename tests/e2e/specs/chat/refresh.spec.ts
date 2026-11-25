// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Refreshing while a reply streams (grengin-api PR #100): the server keeps generating and saves the
// reply complete; GET /chat/{id} marks it `generating: true` until then, and the webapp polls it
// (webapp #113). An edit's reply keeps generating too and is shown as the edited version.
import { expect, test, type Page } from '@playwright/test';
import { sql, tokenFor } from '../../support/api';
import { env } from '../../support/env';
import { assistantMessage, conversation, currentChatId, openNewChat, ownerToken, send, transcript, unique, userMessage } from '../../support/chat';

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

type SseEvent = { id?: string; event?: string; data: string };
// Reads GET /chat/stream/{message_id}/events until the reply ends, the way a reloaded page would.
async function resume(messageId: string, headers: Record<string, string> = {}) {
  const response = await fetch(`${env.apiUrl}/chat/stream/${messageId}/events`, {
    headers: { Authorization: `Bearer ${await ownerToken()}`, Accept: 'text/event-stream', ...headers },
  });
  const events: SseEvent[] = [];
  if (response.status !== 200) return { status: response.status, events };
  const reader = response.body!.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    let end: number;
    while ((end = buffer.indexOf('\n\n')) >= 0) {
      const frame = buffer.slice(0, end);
      buffer = buffer.slice(end + 2);
      const event: SseEvent = { data: '' };
      for (const line of frame.split('\n')) {
        if (line.startsWith('id: ')) event.id = line.slice(4);
        else if (line.startsWith('event: ')) event.event = line.slice(7);
        else if (line.startsWith('data: ')) event.data += line.slice(6);
      }
      if (event.event || event.data) events.push(event);
    }
  }
  return { status: response.status, events };
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

  test('a reloaded page can reconnect to the reply stream, replay it and follow it live', async ({ page }) => {
    const prompt = unique('refresh-resume');
    const chatId = await reloadMidStream(page, prompt);
    const live = (await conversation(chatId)).json.messages.find((m: any) => m.role === 'assistant');
    expect(live?.generating).toBe(true);

    // From the start: the reply's own events with positions as ids, then live until done.
    const full = await resume(live.id);
    expect(full.status).toBe(200);
    expect(full.events.map((e) => e.id)).toEqual(full.events.map((_, i) => String(i + 1)));
    expect(full.events.at(-1)?.event).toContain('done');
    const text = full.events.map((e) => e.data).join(' ');
    for (const part of ['part1', 'part2', `part${SLOW}`]) expect(text).toContain(part);

    // Once the reply has ended there is nothing to reconnect to; the client reads the conversation.
    await expect.poll(async () => (await resume(live.id)).status, { timeout: 10_000 }).toBe(404);
  });

  test('reconnecting with Last-Event-ID sends only the later events', async ({ page }) => {
    const prompt = unique('refresh-resume-after');
    const chatId = await reloadMidStream(page, prompt);
    const live = (await conversation(chatId)).json.messages.find((m: any) => m.role === 'assistant');
    const later = await resume(live.id, { 'Last-Event-ID': '3' });
    expect(later.status).toBe(200);
    expect(later.events[0]?.id).toBe('4');
    expect(later.events.at(-1)?.event).toContain('done');
  });

  test('another user cannot reconnect to the reply', async ({ page }) => {
    const prompt = unique('refresh-resume-private');
    const chatId = await reloadMidStream(page, prompt);
    const live = (await conversation(chatId)).json.messages.find((m: any) => m.role === 'assistant');
    const stranger = await fetch(`${env.apiUrl}/chat/stream/${live.id}/events`, {
      headers: { Authorization: `Bearer ${tokenFor('00000000-0000-4000-8000-000000000000', 'nobody@e2e.grengin.test')}` },
    });
    expect([401, 404]).toContain(stranger.status);
    await expect.poll(async () => (await conversation(chatId)).json.messages.at(-1)?.generating, { timeout: 30_000 }).toBe(false);
  });
});
