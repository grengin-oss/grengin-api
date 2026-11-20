// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// The stack runs grengin-api with RAG_ENABLED=false and no embedding configuration, so project
// sources can be stored but never indexed or retrieved.
import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import { createProject, openProject, registerTextSource, unique } from '../../support/projects';

const NOTES = 'Release checklist\n- freeze the branch\n- tag the build\n';

test.describe('project sources', () => {
  test('a text file uploaded on the Sources tab is listed as a project source', async ({ page }) => {
    // Known bug: uploadProjectSource posts multipart FormData, but request() keeps its default
    // `Content-Type: application/json` header and POST /projects/{id}/sources only accepts a JSON
    // AddSourceRequest (file_name, file_type, file_size, file_id). The API rejects the upload and
    // the page shows "Failed to upload file".
    test.fail();
    const name = unique('E2E Project sources');
    const projectId = await createProject(page, name);
    await openProject(page, projectId, name);
    await page.getByRole('tab', { name: /^Sources/ }).click();

    const fileName = `${unique('release-notes')}.txt`;
    await page
      .locator('label', { hasText: 'Browse Files' })
      .locator('input[type="file"]')
      .setInputFiles({ name: fileName, mimeType: 'text/plain', buffer: Buffer.from(NOTES) });
    await expect(page.getByText('1 file uploaded')).toBeVisible();
    await expect(page.getByRole('main').getByText(fileName)).toBeVisible();
    expect(await sql('SELECT 1 FROM project_sources WHERE "projectId" = $1 AND "fileName" = $2', [projectId, fileName])).toHaveLength(1);
  });

  test('with RAG off, a text source settles as failed instead of ready or pending', async ({ page }) => {
    const name = unique('E2E Project rag-off');
    const projectId = await createProject(page, name);
    const fileName = `${unique('rag-off')}.txt`;
    const source = await registerTextSource(projectId, fileName, NOTES);

    // Nothing can be embedded, so the source must not claim to be ready, nor stay in flight.
    await expect
      .poll(async () => {
        const [row] = await sql<{ status: string; error: string | null }>(
          'SELECT "processingStatus" AS status, "processingError" AS error FROM project_sources WHERE id = $1',
          [source.id],
        );
        return row?.status;
      })
      .toBe('error');
    const [row] = await sql<{ error: string | null }>('SELECT "processingError" AS error FROM project_sources WHERE id = $1', [source.id]);
    expect(row.error ?? '').not.toBe('');
    expect(await sql('SELECT 1 FROM project_source_chunks WHERE "projectSourceId" = $1', [source.id])).toHaveLength(0);

    await openProject(page, projectId, name);
    await page.getByRole('tab', { name: /^Sources/ }).click();
    await expect(page.getByRole('main').getByText(fileName)).toBeVisible();
  });

  test('the Sources tab tells the user when a source could not be indexed', async ({ page }) => {
    // Known gap: the webapp's ProjectSource type has no processing status, so a source the API
    // marked `error` (never indexed, never used in chats) shows as a normal "Uploaded" file,
    // under a tab that promises its files are used "as context in your chats".
    test.fail();
    const name = unique('E2E Project status');
    const projectId = await createProject(page, name);
    const fileName = `${unique('status-check')}.txt`;
    await registerTextSource(projectId, fileName, NOTES);

    await openProject(page, projectId, name);
    await page.getByRole('tab', { name: /^Sources/ }).click();
    const card = page
      .getByRole('main')
      .locator('div')
      .filter({ hasText: fileName })
      .filter({ has: page.getByRole('button', { name: 'Remove file' }) })
      .last();
    await expect(card).toBeVisible();
    await expect(card).toContainText(/not indexed|could not be (indexed|processed)|failed|error/i);
  });
});
