// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test } from '@playwright/test';
import { USERS } from '../../mocks/users.mjs';
import {
  GATED_MODEL_NAME,
  addUserRule,
  clearRulesViaApi,
  connectorToggle,
  closeComposerMenus,
  createGatedEngine,
  createMcpServer,
  deleteGatedEngine,
  deleteMcpServer,
  enableConnector,
  grantUserViaApi,
  mcpCalls,
  openAdvanced,
  openChat,
  openConnector,
  releaseHeldCall,
  removeOnlyRule,
  resetMcpCalls,
  selectModel,
  sendMessage,
  setDefaultAccess,
  setDefaultAccessViaApi,
  signedInPage,
  syncTools,
  toolMarker,
  uniqueName,
  userId,
  visibleServerNames,
} from '../../support/mcp';

const MEMBER = USERS.member.email;

test.describe('MCP access control', () => {
  let serverId: string;
  const name = uniqueName('access');

  test.beforeAll(async ({ browser }) => {
    serverId = (await createMcpServer(name, 'all_users')).id;
    await syncTools(serverId);
    // Rules name the member, so make sure the member exists (first sign-in provisions it).
    if (!(await userId('member'))) {
      const { context } = await signedInPage(browser, 'member');
      await context.close();
    }
  });
  test.afterAll(async () => {
    // Deleting the server drops its access rules; the member's account is left as it was.
    await deleteMcpServer(serverId);
  });
  test.beforeEach(async () => {
    await resetMcpCalls();
    await clearRulesViaApi(serverId);
  });

  test('an admin-only server is hidden from members and its tools are never offered to them', async ({ page, browser }) => {
    await openConnector(page, name);
    await openAdvanced(page, 'Access Control');
    await setDefaultAccess(page, 'admin_only');
    expect(await visibleServerNames('member')).not.toContain(name);
    expect(await visibleServerNames('owner')).toContain(name);

    const member = await signedInPage(browser, 'member');
    try {
      await expect(await connectorToggle(member.page, name)).toHaveCount(0);
      await closeComposerMenus(member.page);

      await sendMessage(member.page, `admin only [tool:${toolMarker(serverId, 'echo')}]`);
      await expect(member.page.getByRole('log').getByText(/Mock reply: admin only/).first()).toBeVisible();
      await expect(member.page.getByRole('log').getByText(/Tool said:/)).toHaveCount(0);
      expect((await mcpCalls()).count).toBe(0);
    } finally {
      await member.context.close();
    }
  });

  test('with explicit access, a member can use the server only after an admin grants it', async ({ page, browser }) => {
    await openConnector(page, name);
    await openAdvanced(page, 'Access Control');
    await setDefaultAccess(page, 'explicit_only');
    expect(await visibleServerNames('member')).not.toContain(name);

    const member = await signedInPage(browser, 'member');
    try {
      await expect(await connectorToggle(member.page, name)).toHaveCount(0);
      await closeComposerMenus(member.page);

      await addUserRule(page, MEMBER);
      expect(await visibleServerNames('member')).toContain(name);

      await openChat(member.page);
      await enableConnector(member.page, name);
      await sendMessage(member.page, `granted [tool:${toolMarker(serverId, 'echo')}]`);
      await expect(member.page.getByRole('log').getByText(/Tool said:.*echo: ping/).first()).toBeVisible();
      const received = await mcpCalls();
      expect(received.count).toBe(1);
      expect(received.calls[0]).toMatchObject({ name: 'echo', arguments: { text: 'ping' } });
    } finally {
      await member.context.close();
    }
  });

  test("revoking a member's access stops their next tool request in an already open chat", async ({ page, browser }) => {
    await setDefaultAccessViaApi(serverId, 'explicit_only');
    await grantUserViaApi(serverId, 'member');
    const member = await signedInPage(browser, 'member');
    try {
      await enableConnector(member.page, name);
      await sendMessage(member.page, `first [tool:${toolMarker(serverId, 'echo')}]`);
      await expect(member.page.getByRole('log').getByText(/Tool said:.*echo: ping/).first()).toBeVisible();
      expect((await mcpCalls()).count).toBe(1);

      // The admin revokes while the member's chat stays open (no reload on the member's side).
      await openConnector(page, name);
      await openAdvanced(page, 'Access Control');
      await removeOnlyRule(page, MEMBER);
      expect(await visibleServerNames('member')).not.toContain(name);

      await sendMessage(member.page, `second [tool:${toolMarker(serverId, 'echo')}]`);
      await expect(member.page.getByRole('log').getByText(/Mock reply: second/).first()).toBeVisible();
      await expect(member.page.getByRole('log').getByText(/Tool said:/)).toHaveCount(1);
      expect((await mcpCalls()).count).toBe(1);
    } finally {
      await member.context.close();
    }
  });

  test('revoking access while a tool request is in flight stops its next tool call', async ({ page, browser }) => {
    // Known API bug (runtime audit CONNECTOR-01): the current-access re-check at dispatch
    // (call_with_current_access) only runs for native connector plugins; a remote MCP call uses
    // the permission captured when the request started. Here the model calls `echo` twice in one
    // turn; access is revoked while the first call is held, and the second call still reaches the
    // server. Fixed by grengin-api PR #108 (not merged yet); remove `fail` once it is.
    test.fail();
    test.setTimeout(120_000);
    await setDefaultAccessViaApi(serverId, 'explicit_only');
    await grantUserViaApi(serverId, 'member');
    await createGatedEngine();
    const member = await signedInPage(browser, 'member');
    try {
      await selectModel(member.page, GATED_MODEL_NAME);
      await enableConnector(member.page, name);
      await sendMessage(member.page, `run it twice [twice:${toolMarker(serverId, 'echo')}]`);
      await expect.poll(async () => (await mcpCalls()).held, { timeout: 30_000 }).toBe(1);

      await openConnector(page, name);
      await openAdvanced(page, 'Access Control');
      await removeOnlyRule(page, MEMBER);
      await releaseHeldCall();

      await expect(member.page.getByRole('log').getByText(/Gated done:/).first()).toBeVisible({ timeout: 30_000 });
      const received = await mcpCalls();
      expect(received.calls.map((call) => call.arguments.text)).toEqual(['hold']);
    } finally {
      await releaseHeldCall();
      await member.context.close();
      await deleteGatedEngine();
    }
  });
});
