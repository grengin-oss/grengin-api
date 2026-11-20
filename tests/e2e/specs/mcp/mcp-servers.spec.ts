// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { sql } from '../../support/api';
import {
  MCP_ENDPOINT,
  createMcpServer,
  deleteMcpServer,
  openAdvanced,
  openConnector,
  serverTools,
  syncTools,
  uniqueName,
} from '../../support/mcp';

test.describe('admin: remote MCP servers', () => {
  test('adds a remote HTTP MCP server from Connectors → Add Server', async ({ page }) => {
    // Known webapp bug: the Add Server dialog's Default Access control (Allow / Deny) sends
    // default_access "allow" or "deny", but POST /admin/mcp-servers only accepts all_users,
    // admin_only or explicit_only, so every create is rejected with a deserialization error.
    // MCPServers.svelte openCreateModal()/handleSubmit() vs McpDefaultAccess in the API.
    // Remove `fail` once the dialog maps its choice to a valid value.
    test.fail();
    const name = uniqueName('added in UI');
    try {
      await page.goto('/admin/mcp-servers');
      await page.getByRole('button', { name: 'Add Server' }).click();
      const dialog = page.getByRole('dialog', { name: 'Add MCP Server' });
      await dialog.getByLabel('Name', { exact: true }).fill(name);
      await dialog.getByLabel('Description').fill('Mock remote MCP server added through the dialog');
      await expect(dialog.getByRole('button', { name: /Transport/ })).toContainText('HTTP');
      await dialog.getByLabel('URL', { exact: true }).fill(MCP_ENDPOINT);
      await dialog.getByRole('button', { name: 'Create Server' }).click();

      await expect(page.getByText('MCP server created').first()).toBeVisible({ timeout: 10_000 });
      await expect(dialog).toBeHidden();
      await expect(page.getByText(name).first()).toBeVisible();
      const rows = await sql<{ url: string; transportType: string; enabled: boolean }>(
        'SELECT url, "transportType", enabled FROM mcp_servers WHERE name = $1',
        [name],
      );
      expect(rows).toEqual([{ url: MCP_ENDPOINT, transportType: 'http', enabled: true }]);
    } finally {
      const [row] = await sql<{ id: string }>('SELECT id FROM mcp_servers WHERE name = $1', [name]);
      await deleteMcpServer(row?.id);
    }
  });

  test.describe('an added server', () => {
    let serverId: string;
    const name = uniqueName('tools');

    test.beforeAll(async () => {
      serverId = (await createMcpServer(name)).id;
    });
    test.afterAll(async () => {
      await deleteMcpServer(serverId);
    });

    test('offers a way to sync its tools from the admin UI', async ({ page }) => {
      // Known webapp gap: webapp main has no Sync Tools (or Test Connection) action for a
      // custom remote server. syncMcpServerTools() in api/admin/mcpServers.ts is never called and
      // the "Sync Tools" label is unused, while the Tool Access tab tells the admin to "Sync tools
      // from the server list". Remote servers only get tools through POST .../sync-tools.
      // Remove `fail` once the connector page offers the sync.
      test.fail();
      await openConnector(page, name);
      const syncOnOverview = page.getByRole('button', { name: /Sync Tools/i });
      if (!(await syncOnOverview.isVisible())) await openAdvanced(page, 'Tool Access');
      await page.getByRole('button', { name: /Sync Tools/i }).click({ timeout: 5_000 });
      await expect(page.getByText('Tools synced successfully').first()).toBeVisible();
      expect((await serverTools(serverId)).map((tool) => tool.name)).toEqual(['add', 'echo']);
    });

    test('shows the synced tools with their descriptions', async ({ page }) => {
      const result = await syncTools(serverId);
      expect(result.total_tools).toBe(2);

      await openConnector(page, name);
      const toolSection = page.locator('section').filter({ has: page.getByRole('heading', { name: 'Tools', exact: true }) });
      await expect(toolSection.getByText('echo', { exact: true })).toHaveAttribute('title', 'Echo the given text back');
      await expect(toolSection.getByText('add', { exact: true })).toHaveAttribute('title', 'Add two numbers');

      await openAdvanced(page, 'Tool Access');
      await expect(page.getByText('Server Tools (2 total)')).toBeVisible();
      await expect(page.getByText('echo', { exact: true })).toBeVisible();
      await expect(page.getByText('add', { exact: true })).toBeVisible();
    });

    test('lets an admin switch a single tool off and on again', async ({ page }) => {
      // Known webapp gap: the per-tool switches ("What Grengin can do with it") are disabled in
      // ConnectorOverview.svelte with `{#if false}`, and they act on topic groups, which a custom
      // remote server does not have. The API supports it (PUT /admin/mcp-servers/{id}/tools/{tool}).
      // Remove `fail` once the connector page has a per-tool switch.
      test.fail();
      await syncTools(serverId);
      await openConnector(page, name);
      const addSwitch = page.getByRole('switch', { name: /add/i });
      await addSwitch.click({ timeout: 5_000 });
      await expect(addSwitch).toHaveAttribute('aria-checked', 'false');
      await expect.poll(async () => (await serverTools(serverId)).find((tool) => tool.name === 'add')?.enabled).toBe(false);
      await addSwitch.click();
      await expect(addSwitch).toHaveAttribute('aria-checked', 'true');
      await expect.poll(async () => (await serverTools(serverId)).find((tool) => tool.name === 'add')?.enabled).toBe(true);
    });
  });
});
