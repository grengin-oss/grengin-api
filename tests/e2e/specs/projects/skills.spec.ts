// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import { dismissOnboarding } from '../../support/login';
import {
  chooseEchoModel,
  conversationIdFromUrl,
  echoReply,
  ensureEchoEngine,
  marker,
  openSkillsSettings,
  removeEchoEngine,
  send,
  setComposerSkill,
  unique,
  writeSkill,
} from '../../support/projects';

async function newEchoChat(page: import('@playwright/test').Page) {
  await page.goto('/');
  await dismissOnboarding(page);
  await chooseEchoModel(page);
}

test.describe('skills', () => {
  test.beforeAll(async () => {
    await ensureEchoEngine();
  });

  test.afterAll(async () => {
    await removeEchoEngine();
  });

  test('a skill turned on in an ongoing chat shapes the next reply', async ({ page }) => {
    const skillMarker = marker();
    const name = unique('e2e-skill-live');
    await writeSkill(page, { name, description: 'Adds a marker to answers', instructions: `Always mention ${skillMarker}.` });
    const [skill] = await sql<{ id: string; instructions: string }>('SELECT id, instructions FROM skills WHERE name = $1', [name]);
    expect(skill.instructions).toBe(`Always mention ${skillMarker}.`);

    await newEchoChat(page);
    await send(page, 'before the skill');
    await expect(echoReply(page, 'before the skill')).toContainText('markers=none');
    const conversationId = await conversationIdFromUrl(page);

    await setComposerSkill(page, name, true);
    await expect
      .poll(async () => (await sql('SELECT 1 FROM conversation_skills WHERE "conversationId" = $1 AND "skillId" = $2', [conversationId, skill.id])).length)
      .toBe(1);
    await send(page, 'with the skill');
    await expect(echoReply(page, 'with the skill')).toContainText(skillMarker);

    await setComposerSkill(page, name, false);
    await send(page, 'skill turned off');
    await expect(echoReply(page, 'skill turned off')).toContainText('markers=none');
  });

  test('a skill picked before the first message shapes the first reply', async ({ page }) => {
    // Known bug: in a new chat the composer only stages the skill and links it after the API
    // sends the `conversation` event; by then the API has already assembled the system prompt
    // for the first reply, and the webapp does not send the staged ids as `selected_skills`.
    test.fail();
    const skillMarker = marker();
    const name = unique('e2e-skill-first');
    await writeSkill(page, { name, description: 'Adds a marker to answers', instructions: `Always mention ${skillMarker}.` });

    await newEchoChat(page);
    await setComposerSkill(page, name, true);
    await send(page, 'first message with skill');
    await expect(echoReply(page, 'first message with skill')).toBeVisible();
    await expect(echoReply(page, 'first message with skill')).toContainText(skillMarker);
  });

  test("an imported .md skill brings its file to the model as knowledge", async ({ page }) => {
    const knowledgeMarker = marker();
    const slug = unique('e2e-skill-import');
    await openSkillsSettings(page);
    await page.getByRole('button', { name: 'New Skill' }).click();
    await page.getByRole('menuitem', { name: /Import file/ }).click();
    const dialog = page.getByRole('dialog', { name: 'Import skill' });
    await dialog.locator('input[type="file"]').setInputFiles({
      name: `${slug}.md`,
      mimeType: 'text/markdown',
      buffer: Buffer.from(`# Release glossary\n\nThe code word for a green build is ${knowledgeMarker}.\n`),
    });
    await expect(dialog.getByText('Will be imported as')).toContainText(slug);
    await dialog.getByRole('button', { name: 'Import', exact: true }).click();
    await expect(page.getByText('Skill imported')).toBeVisible();
    const card = page.getByRole('article').filter({ hasText: slug });
    await expect(card).toContainText('1 files');

    const [skill] = await sql<{ id: string }>('SELECT id FROM skills WHERE name = $1', [slug]);
    const knowledge = await sql<{ content: string }>('SELECT content FROM skill_knowledge WHERE "skillId" = $1', [skill.id]);
    expect(knowledge.map((k) => k.content).join('\n')).toContain(knowledgeMarker);

    await newEchoChat(page);
    await send(page, 'before the import');
    await expect(echoReply(page, 'before the import')).toContainText('markers=none');
    await setComposerSkill(page, slug, true);
    await send(page, 'what is the code word');
    await expect(echoReply(page, 'what is the code word')).toContainText(knowledgeMarker);
  });

  test('a deleted skill disappears from Settings and from the composer', async ({ page }) => {
    const kept = unique('e2e-skill-kept');
    const name = unique('e2e-skill-delete');
    await writeSkill(page, { name: kept, description: 'Stays', instructions: 'Say hello.' });
    await writeSkill(page, { name, description: 'Short-lived', instructions: 'Say goodbye.' });

    const card = page.getByRole('article').filter({ hasText: name });
    await card.getByRole('button', { name: 'Delete', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Delete skill' });
    await dialog.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByText('Skill deleted')).toBeVisible();
    await expect(card).toHaveCount(0);
    await expect(page.getByRole('article').filter({ hasText: kept })).toBeVisible();
    expect(await sql('SELECT 1 FROM skills WHERE name = $1', [name])).toHaveLength(0);

    await page.goto('/');
    await dismissOnboarding(page);
    await page.getByRole('button', { name: 'Select connectors' }).click();
    await expect(page.getByRole('button', { name: kept, exact: true })).toBeVisible();
    await expect(page.getByRole('button', { name, exact: true })).toHaveCount(0);
  });
});
