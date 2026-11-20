// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import {
  assistantMessage,
  conversation,
  currentChatId,
  openNewChat,
  send,
  transcript,
  unique,
  userMessage,
  versions,
} from '../../support/chat';

type Row = { id: string; role: string; content: string; deleted: boolean; inactive: boolean; parent: string | null };
const rowsOf = (chatId: string) =>
  sql<Row>(
    `SELECT id, role::text AS role, "messageContent" AS content, deleted, "inactiveBranch" AS inactive,
            "previousMessageId" AS parent
     FROM messages WHERE "conversationId" = $1 ORDER BY "createdAt"`,
    [chatId],
  );

test.describe('chat message versions', () => {
  test.beforeEach(async ({ page }) => {
    await openNewChat(page);
  });

  test('editing a sent message adds a version; the switcher shows both and the choice survives a reload', async ({ page }) => {
    const original = unique('edit-original');
    const edited = unique('edit-new');
    await send(page, original);
    await expect(assistantMessage(page, `Mock reply: ${original}`)).toBeVisible();
    const chatId = await currentChatId(page);

    await userMessage(page, original).hover();
    await userMessage(page, original).getByRole('button', { name: 'Edit message' }).click();
    const editor = transcript(page).getByRole('textbox', { name: 'Edit message' });
    await expect(editor).toHaveValue(original);
    await editor.fill(edited);
    await transcript(page).getByRole('button', { name: 'Save', exact: true }).click();

    await expect(assistantMessage(page, `Mock reply: ${edited}`)).toBeVisible();
    await expect(userMessage(page, edited)).toBeVisible();
    await expect(userMessage(page, original)).toHaveCount(0);
    await expect(assistantMessage(page, `Mock reply: ${original}`)).toHaveCount(0);
    await expect(versions(page)).toHaveCount(1);
    await expect(versions(page)).toContainText('2 / 2');

    const shown = await conversation(chatId);
    expect(shown.status).toBe(200);
    const shownUser = shown.json.messages.find((m: any) => m.role === 'user');
    expect(shownUser.parts.text).toBe(edited);
    expect(shownUser.versions).toMatchObject({ current: 2, total: 2 });
    expect(shownUser.versions.message_ids).toHaveLength(2);
    expect(shownUser.versions.message_ids[1]).toBe(shownUser.id);

    // Both versions are kept: the original turn is hidden as another version, not deleted.
    const rows = await rowsOf(chatId);
    const users = rows.filter((r) => r.role === 'user');
    expect(users.map((r) => r.content)).toEqual([original, edited]);
    expect(users[0]).toMatchObject({ deleted: true, inactive: true });
    expect(users[1]).toMatchObject({ deleted: false });
    expect(users[0].parent).toBe(users[1].parent);

    await versions(page).getByRole('button', { name: 'Previous version' }).click();
    await expect(versions(page)).toContainText('1 / 2');
    await expect(userMessage(page, original)).toBeVisible();
    await expect(assistantMessage(page, `Mock reply: ${original}`)).toBeVisible();
    await expect(assistantMessage(page, `Mock reply: ${edited}`)).toHaveCount(0);
    await expect(versions(page).getByRole('button', { name: 'Previous version' })).toBeDisabled();

    await page.reload();
    await expect(userMessage(page, original)).toBeVisible();
    await expect(assistantMessage(page, `Mock reply: ${original}`)).toBeVisible();
    await expect(versions(page)).toContainText('1 / 2');
    const afterReload = await conversation(chatId);
    expect(afterReload.json.messages.find((m: any) => m.role === 'user').versions).toMatchObject({ current: 1, total: 2 });

    await versions(page).getByRole('button', { name: 'Next version' }).click();
    await expect(versions(page)).toContainText('2 / 2');
    await expect(userMessage(page, edited)).toBeVisible();
    await expect(assistantMessage(page, `Mock reply: ${edited}`)).toBeVisible();
  });

  test('Regenerate asks the model again and keeps the earlier answer as another version', async ({ page }) => {
    const prompt = unique('regenerate');
    await send(page, prompt);
    const answer = assistantMessage(page, `Mock reply: ${prompt}`);
    await expect(answer).toBeVisible();
    const chatId = await currentChatId(page);
    const [first] = (await rowsOf(chatId)).filter((r) => r.role === 'assistant');

    await answer.getByRole('button', { name: 'Regenerate' }).click();
    await expect(versions(page)).toContainText('2 / 2');
    await expect(assistantMessage(page, `Mock reply: ${prompt}`)).toHaveCount(1);

    let rows: Row[] = [];
    await expect
      .poll(async () => {
        rows = await rowsOf(chatId);
        return rows.filter((r) => r.role === 'assistant').length;
      })
      .toBe(2);
    const answers = rows.filter((r) => r.role === 'assistant');
    expect(answers[0].id).toBe(first.id);
    expect(answers[0]).toMatchObject({ deleted: true, inactive: true });
    expect(answers[1]).toMatchObject({ deleted: false, content: `Mock reply: ${prompt}` });

    const shown = await conversation(chatId);
    const shownAnswer = shown.json.messages.find((m: any) => m.role === 'assistant');
    expect(shownAnswer.id).toBe(answers[1].id);

    await versions(page).getByRole('button', { name: 'Previous version' }).click();
    await expect(versions(page)).toContainText('1 / 2');
    await expect
      .poll(async () => (await conversation(chatId)).json.messages.find((m: any) => m.role === 'assistant')?.id)
      .toBe(first.id);
  });
});
