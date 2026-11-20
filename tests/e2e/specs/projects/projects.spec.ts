// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import { env } from '../../support/env';
import { dismissOnboarding, signIn } from '../../support/login';
import {
  chooseEchoModel,
  composer,
  conversationIdFromUrl,
  createProject,
  echoReply,
  ensureEchoEngine,
  escapeRegExp,
  linkChatToProject,
  marker,
  openProject,
  removeEchoEngine,
  saveInstructions,
  send,
  unique,
} from '../../support/projects';

// Starts a chat from the project's empty Chats tab and answers the first question with the echo
// model, which reports the markers it finds in the system prompt.
async function startEchoChatFromProject(page: import('@playwright/test').Page, question: string) {
  await page.getByRole('tab', { name: /^Chats/ }).click();
  await page.getByRole('button', { name: 'Start Chat' }).click();
  await page.waitForURL(/[?&]projectId=/);
  await dismissOnboarding(page);
  await chooseEchoModel(page);
  await send(page, question);
  await expect(echoReply(page, question)).toBeVisible();
  return conversationIdFromUrl(page);
}

test.describe('projects', () => {
  test.beforeAll(async () => {
    await ensureEchoEngine();
  });

  test.afterAll(async () => {
    await removeEchoEngine();
  });

  test('a project is created on the Projects page, opens, and keeps its instructions', async ({ page }) => {
    const name = unique('E2E Project');
    const instructions = `Answer as the release manager. ${marker()}`;
    await createProject(page, name, 'Created by the projects spec');

    await page.getByRole('main').getByRole('button', { name: new RegExp(escapeRegExp(name)) }).first().click();
    await expect(page).toHaveURL(/\/projects\/[0-9a-f-]{36}$/);
    await expect(page.getByRole('heading', { level: 1, name })).toBeVisible();
    await expect(page.getByText('Created by the projects spec')).toBeVisible();

    await saveInstructions(page, instructions);
    const [row] = await sql<{ instructions: string | null; visibility: string }>(
      'SELECT instructions, visibility FROM projects WHERE name = $1',
      [name],
    );
    expect(row).toEqual({ instructions, visibility: 'private' });

    await page.reload();
    await page.getByRole('tab', { name: 'Instructions' }).click();
    await expect(page.getByPlaceholder(/Always respond in a formal tone/)).toHaveValue(instructions);
  });

  test('a chat started from a project is filed under that project', async ({ page }) => {
    // Known bug: Chat.svelte links the new conversation with POST /conversations/{id}/projects
    // and body {"projectId": …}, while the API's LinkProjectRequest requires `project_id`; the
    // API answers 422 and the webapp only logs it, so the chat never appears in the project.
    test.fail();
    const name = unique('E2E Project chats');
    const projectId = await createProject(page, name);
    await openProject(page, projectId, name);
    const question = 'which release is next';
    const conversationId = await startEchoChatFromProject(page, question);

    await expect
      .poll(async () => (await sql('SELECT 1 FROM conversation_projects WHERE "conversationId" = $1 AND "projectId" = $2', [conversationId, projectId])).length)
      .toBe(1);
    await openProject(page, projectId, name);
    await expect(page.getByRole('button', { name: new RegExp(escapeRegExp(question)) })).toBeVisible();
  });

  test("the project's instructions reach the model from the first message of a project chat", async ({ page }) => {
    // Known bug: the chat is never linked (see the test above). Even with the link fixed, the
    // webapp links only after the API has sent the `conversation` event, which happens after the
    // system prompt for the first reply was assembled, so the first reply misses the instructions.
    test.fail();
    const name = unique('E2E Project first');
    const instructionMarker = marker();
    const projectId = await createProject(page, name);
    await openProject(page, projectId, name);
    await saveInstructions(page, `Mention ${instructionMarker} in every answer.`);

    const question = 'first question in project';
    await startEchoChatFromProject(page, question);
    await expect(echoReply(page, question)).toContainText(instructionMarker);
  });

  test("the project's instructions reach the model in a chat filed under the project", async ({ page }) => {
    const name = unique('E2E Project linked');
    const instructionMarker = marker();
    const projectId = await createProject(page, name);
    await openProject(page, projectId, name);
    await saveInstructions(page, `Mention ${instructionMarker} in every answer.`);

    const conversationId = await startEchoChatFromProject(page, 'opening question');
    // The webapp's own link call fails (see above); file the chat the way the API expects.
    await linkChatToProject(conversationId, projectId);

    const question = 'follow-up question';
    await send(page, question);
    await expect(echoReply(page, question)).toContainText(instructionMarker);
  });

  test("a message typed in the project page's composer is answered by an available model", async ({ page }) => {
    // Known bug: ProjectDetailPage starts with selectedModel "gpt-5.2" / provider "openai"
    // hardcoded instead of the installation's models, and sends them in the ?model=&provider=
    // of the chat it opens, so the message goes to a model this installation does not offer.
    test.fail();
    const name = unique('E2E Project composer');
    const projectId = await createProject(page, name);
    await openProject(page, projectId, name);

    const question = 'hello from the project page';
    await send(page, question);
    await page.waitForURL(/[?&]projectId=/);
    await expect(page.getByRole('log').getByText(new RegExp(`(Mock reply:|Echo) ${escapeRegExp(question)}`))).toBeVisible();
  });

  test('renaming a project from its page updates the page and the list', async ({ page }) => {
    const name = unique('E2E Project rename');
    const renamed = `${name} renamed`;
    const projectId = await createProject(page, name);
    await openProject(page, projectId, name);

    await page.getByRole('button', { name: 'Edit', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Edit Project' });
    await dialog.getByLabel('Project name').fill(renamed);
    await dialog.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(page.getByText(`${renamed} updated successfully`)).toBeVisible();
    await expect(page.getByRole('heading', { level: 1, name: renamed })).toBeVisible();

    const [row] = await sql<{ name: string }>('SELECT name FROM projects WHERE id = $1', [projectId]);
    expect(row.name).toBe(renamed);
    await page.goto('/projects');
    await expect(page.getByRole('main').getByText(renamed, { exact: true }).first()).toBeVisible();
    await expect(page.getByRole('main').getByText(name, { exact: true })).toHaveCount(0);
  });

  test('deleting a project removes it and leaves its chats as ordinary chats', async ({ page }) => {
    const name = unique('E2E Project delete');
    const projectId = await createProject(page, name);
    await openProject(page, projectId, name);
    const question = 'keep me after delete';
    const conversationId = await startEchoChatFromProject(page, question);
    await linkChatToProject(conversationId, projectId);

    await page.goto('/projects');
    await page.getByRole('button', { name: 'List', exact: true }).click();
    const row = page.getByRole('listitem').filter({ hasText: name });
    await row.getByRole('button', { name: 'Delete', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Delete Project' });
    await dialog.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByText(`${name} deleted successfully`)).toBeVisible();
    await expect(row).toHaveCount(0);

    expect(await sql('SELECT 1 FROM projects WHERE id = $1', [projectId])).toHaveLength(0);
    expect(await sql('SELECT 1 FROM conversation_projects WHERE "projectId" = $1', [projectId])).toHaveLength(0);
    expect(await sql('SELECT 1 FROM conversations WHERE id = $1', [conversationId])).toHaveLength(1);

    await page.goto(`/?chatId=${conversationId}`);
    await expect(echoReply(page, question)).toBeVisible();
    await expect(composer(page)).toBeEnabled();
  });

  test('a private project shared with a member is visible to them; unshared ones are not', async ({ page, browser }) => {
    test.setTimeout(120_000);
    const memberContext = await browser.newContext({ baseURL: env.webUrl, storageState: { cookies: [], origins: [] } });
    const memberPage = await memberContext.newPage();
    // Signing in provisions the member, so the owner can find them.
    await signIn(memberPage, 'member');
    await dismissOnboarding(memberPage);

    const shared = unique('E2E Project shared');
    const hidden = unique('E2E Project hidden');
    const sharedId = await createProject(page, shared);
    await createProject(page, hidden);

    await openProject(page, sharedId, shared);
    await page.getByRole('button', { name: 'Members', exact: true }).click();
    const picker = page.getByRole('dialog', { name: 'Add members' });
    await picker.getByRole('textbox', { name: /Search by name, email, or department/ }).fill('member@e2e');
    await picker.getByRole('checkbox', { name: 'E2E Member (member@e2e.grengin.test)' }).click();
    await picker.getByRole('button', { name: 'Add 1 Member' }).click();
    await expect(page.getByText(`E2E Member added to "${shared}"`)).toBeVisible();
    await page.getByRole('tab', { name: /^Members/ }).click();
    await expect(page.getByRole('main').getByText('member@e2e.grengin.test')).toBeVisible();

    await memberPage.goto('/projects');
    const memberMain = memberPage.getByRole('main');
    await expect(memberMain.getByText(shared, { exact: true }).first()).toBeVisible();
    await expect(memberMain.getByText(hidden, { exact: true })).toHaveCount(0);
    await memberMain.getByRole('button', { name: new RegExp(escapeRegExp(shared)) }).first().click();
    await expect(memberPage.getByRole('heading', { level: 1, name: shared })).toBeVisible();
    await memberContext.close();
  });
});
