// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import {
  assistantMessage,
  conversation,
  currentChatId,
  openChatOptions,
  openNewChat,
  send,
  sidebarChat,
  unique,
  userMessage,
  word,
} from '../../support/chat';

test.describe('chat sidebar', () => {
  test.beforeEach(async ({ page }) => {
    await openNewChat(page);
  });

  test('a chat is renamed from the sidebar and keeps the new title after a reload', async ({ page }) => {
    const prompt = unique('rename-source');
    await send(page, prompt);
    await expect(assistantMessage(page, `Mock reply: ${prompt}`)).toBeVisible();
    const chatId = await currentChatId(page);
    const title = `Title: ${prompt}`;
    await expect(sidebarChat(page, title)).toBeVisible();

    const menu = await openChatOptions(page, title);
    await menu.getByRole('button', { name: 'Rename' }).click();
    const renamed = unique('Renamed chat');
    const input = page.getByRole('textbox', { name: 'Enter a chat title' });
    await expect(input).toHaveValue(title);
    await input.fill(renamed);
    await input.press('Enter');

    await expect(sidebarChat(page, renamed)).toBeVisible();
    await expect(sidebarChat(page, title)).toHaveCount(0);
    await expect
      .poll(async () => (await sql<{ title: string }>('SELECT title FROM conversations WHERE id = $1', [chatId]))[0]?.title)
      .toBe(renamed);

    await page.reload();
    await expect(sidebarChat(page, renamed)).toBeVisible();
    await expect(userMessage(page, prompt)).toBeVisible();
  });

  test('a deleted chat leaves the sidebar, is gone after a reload and its messages are removed', async ({ page }) => {
    const prompt = unique('delete-me');
    await send(page, prompt);
    await expect(assistantMessage(page, `Mock reply: ${prompt}`)).toBeVisible();
    const chatId = await currentChatId(page);
    const title = `Title: ${prompt}`;
    await expect(sidebarChat(page, title)).toBeVisible();

    const menu = await openChatOptions(page, title);
    await menu.getByRole('button', { name: 'Delete' }).click();
    const dialog = page.getByRole('dialog', { name: 'Delete Chat' });
    await expect(dialog).toContainText(title);
    await dialog.getByRole('button', { name: 'Delete', exact: true }).click();

    await expect(dialog).toHaveCount(0);
    await expect(sidebarChat(page, title)).toHaveCount(0);
    // The open chat was the one deleted, so the webapp leaves it.
    await expect(page).not.toHaveURL(new RegExp(chatId));
    await expect(userMessage(page, prompt)).toHaveCount(0);

    await page.reload();
    await expect(page.getByRole('textbox', { name: 'Message input' })).toBeVisible();
    await expect(sidebarChat(page, title)).toHaveCount(0);
    expect((await conversation(chatId)).status).toBe(404);
    expect(await sql('SELECT id FROM conversations WHERE id = $1', [chatId])).toHaveLength(0);
    expect(await sql('SELECT id FROM messages WHERE "conversationId" = $1', [chatId])).toHaveLength(0);
  });

  test('cancelling the delete dialog keeps the chat', async ({ page }) => {
    const prompt = unique('keep-me');
    await send(page, prompt);
    await expect(assistantMessage(page, `Mock reply: ${prompt}`)).toBeVisible();
    const chatId = await currentChatId(page);
    const title = `Title: ${prompt}`;
    await expect(sidebarChat(page, title)).toBeVisible();

    const menu = await openChatOptions(page, title);
    await menu.getByRole('button', { name: 'Delete' }).click();
    const dialog = page.getByRole('dialog', { name: 'Delete Chat' });
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).toHaveCount(0);
    await expect(sidebarChat(page, title)).toBeVisible();
    expect((await conversation(chatId)).status).toBe(200);
  });

  test('the search palette finds a chat by a word from its messages and by its title, and opens it', async ({ page }) => {
    const first = unique('search-source');
    const needle = word('needle');
    await send(page, first);
    await expect(assistantMessage(page, `Mock reply: ${first}`)).toBeVisible();
    await send(page, `the hidden word is ${needle} today`);
    await expect(assistantMessage(page, needle)).toBeVisible();
    const chatId = await currentChatId(page);
    await expect(sidebarChat(page, `Title: ${first}`)).toBeVisible();
    // A title word that occurs in no message, so finding it proves the title is searched.
    const title = `Found by ${word('title')}`;
    const menu = await openChatOptions(page, `Title: ${first}`);
    await menu.getByRole('button', { name: 'Rename' }).click();
    await page.getByRole('textbox', { name: 'Enter a chat title' }).fill(title);
    await page.getByRole('textbox', { name: 'Enter a chat title' }).press('Enter');
    await expect(sidebarChat(page, title)).toBeVisible();

    await openNewChat(page);
    await page.getByRole('button', { name: 'Search through your chat conversations' }).first().click();
    const palette = page.getByRole('dialog', { name: 'Search chats and projects' });
    const query = palette.getByRole('combobox', { name: 'Search chats and projects' });
    await expect(query).toBeFocused();
    const result = palette.getByRole('option').filter({ hasText: title });

    await query.fill(needle);
    await expect(result).toHaveCount(1);

    const missing = word('nomatch');
    await query.fill(missing);
    await expect(palette.getByText(`No matches for “${missing}”`)).toBeVisible();
    await expect(result).toHaveCount(0);

    await query.fill(title.split(' ').at(-1)!);
    await expect(result).toHaveCount(1);
    await result.click();

    await expect(palette).toHaveCount(0);
    await expect(page).toHaveURL(new RegExp(`chatId=${chatId}`));
    await expect(userMessage(page, first)).toBeVisible();
    await expect(userMessage(page, needle)).toBeVisible();
  });
});
