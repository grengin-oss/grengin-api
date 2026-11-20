// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import {
  createMcpServer,
  deleteMcpServer,
  enableConnector,
  mcpCalls,
  openChat,
  resetMcpCalls,
  sendMessage,
  syncTools,
  toolMarker,
  uniqueName,
} from '../../support/mcp';

test.describe('chat with an MCP connector', () => {
  let serverId: string;
  const name = uniqueName('chat');

  test.beforeAll(async () => {
    serverId = (await createMcpServer(name, 'all_users')).id;
    await syncTools(serverId);
  });
  test.afterAll(async () => {
    await deleteMcpServer(serverId);
  });
  test.beforeEach(async () => {
    await resetMcpCalls();
  });

  test('the model calls the selected connector tool and answers with its result', async ({ page }) => {
    await openChat(page);
    await enableConnector(page, name);
    await sendMessage(page, `please [tool:${toolMarker(serverId, 'echo')}]`);

    const messages = page.getByRole('log');
    await expect(messages.getByText(/Tool said:.*echo: ping/).first()).toBeVisible();
    // The tool call shows in the conversation as its own step.
    await expect(messages.getByRole('button', { name: /echo/i }).first()).toBeVisible();

    const received = await mcpCalls();
    expect(received.count).toBe(1);
    expect(received.calls[0]).toMatchObject({ name: 'echo', arguments: { text: 'ping' } });

    const executions = await sql<{ toolName: string; isError: boolean }>(
      'SELECT "toolName", "isError" FROM mcp_executions WHERE "serverId" = $1',
      [serverId],
    );
    expect(executions).toEqual([{ toolName: 'echo', isError: false }]);
  });

  test('a connector switched off in the composer is not offered to the model', async ({ page }) => {
    await openChat(page);
    await enableConnector(page, name);
    const toggle = page.getByRole('button', { name, exact: true });
    await page.getByRole('button', { name: 'Select connectors' }).click();
    await toggle.click();
    await expect(toggle).toHaveAttribute('aria-pressed', 'false');
    await page.keyboard.press('Escape');

    await sendMessage(page, `again [tool:${toolMarker(serverId, 'echo')}]`);
    await expect(page.getByRole('log').getByText(/Mock reply: again/).first()).toBeVisible();
    await expect(page.getByRole('log').getByText(/Tool said:/)).toHaveCount(0);
    expect((await mcpCalls()).count).toBe(0);
  });
});
