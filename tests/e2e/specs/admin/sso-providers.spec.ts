// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

import { expect, test, type Page } from '@playwright/test';
import { sql } from '../../support/api';
import { env } from '../../support/env';
import {
  createOidcProvider,
  removeProvider,
  ensureE2eIdpEnabled,
  IDP_NAME,
  IDP_SLUG,
  providers,
} from '../../support/admin';

const LOCKOUT = 'At least one SSO provider must remain enabled';

function card(page: Page, name: string) {
  return page.locator('article').filter({ has: page.getByText(name, { exact: true }) });
}

async function isEnabled(id: string): Promise<boolean> {
  const [row] = await sql<{ isEnabled: boolean }>('SELECT "isEnabled" FROM sso_providers WHERE id = $1', [id]);
  return Boolean(row?.isEnabled);
}

test.describe('sign-in providers (Control Hub → Authentication)', () => {
  test.afterEach(async () => {
    await ensureE2eIdpEnabled();
  });

  test('lists the suite identity provider as active and enabled', async ({ page }) => {
    await page.goto('/admin/settings');
    await expect(page.getByRole('heading', { name: 'Authentication' })).toBeVisible();
    const idp = card(page, IDP_NAME);
    await expect(idp).toBeVisible();
    await expect(idp.getByText('Active', { exact: true })).toBeVisible();
    await expect(idp.getByText(`Sign in with ${IDP_NAME}`)).toBeVisible();
    await expect(idp.getByRole('checkbox', { name: `Toggle ${IDP_NAME} provider status` })).toBeChecked();
    await expect(idp.getByText('Enabled', { exact: true })).toBeVisible();
  });

  test('a second OIDC provider is validated and enabled from Configure, then disabled and deleted', async ({ page }) => {
    const slug = `e2e-sso-${Date.now().toString(36)}`;
    const name = `E2E SSO ${slug.slice(8)}`;
    // The Authentication page has no "add provider" flow; the provider starts disabled, as
    // POST /admin/sso-providers creates it.
    const id = await createOidcProvider(slug, name);
    try {
      await page.goto('/admin/settings');
      const row = card(page, name);
      await expect(row).toBeVisible();
      await expect(row.getByText('Inactive', { exact: true })).toBeVisible();
      await expect(row.getByText(`Sign in with ${name}`)).toBeVisible();

      // Configure: entering the secret makes the dialog validate the credentials against the
      // IdP before it saves, which is what an enable needs.
      await row.getByRole('button', { name: `Configure ${name}` }).click();
      const dialog = page.getByRole('dialog', { name: `Edit ${name}` });
      await expect(dialog.getByLabel('Client ID', { exact: true })).toHaveValue('grengin-e2e');
      await dialog.getByLabel('Client secret', { exact: true }).fill('e2e-client-secret');
      const status = dialog.locator('.switch-field').filter({ hasText: 'Status' }).getByRole('checkbox');
      await expect(status).not.toBeChecked();
      await status.check({ force: true });
      await expect(dialog.getByRole('button', { name: 'Save & Enable' })).toBeVisible();
      await dialog.getByRole('button', { name: 'Save & Enable' }).click();
      await expect(dialog.getByText(`${name} connected and enabled`)).toBeVisible();
      await dialog.getByRole('button', { name: 'Close', exact: true }).click();
      await expect(dialog).toBeHidden();

      await expect(row.getByText('Active', { exact: true })).toBeVisible();
      await expect(row.getByRole('checkbox', { name: `Toggle ${name} provider status` })).toBeChecked();
      expect(await isEnabled(id)).toBe(true);
      // Its sign-in entry is offered to users now.
      const offered = await (await fetch(`${env.apiUrl}/auth/providers`)).json();
      expect(JSON.stringify(offered)).toContain(`"${slug}"`);

      // Disable: allowed, because the suite IdP stays enabled.
      await row.getByRole('checkbox', { name: `Toggle ${name} provider status` }).uncheck({ force: true });
      await expect(page.getByRole('status').filter({ hasText: `SSO provider ${name} Disabled.` })).toBeVisible();
      await expect(row.getByText('Inactive', { exact: true })).toBeVisible();
      expect(await isEnabled(id)).toBe(false);

      // Delete through the confirmation dialog.
      await row.getByRole('button', { name: `Delete ${name}` }).click();
      const confirm = page.getByRole('dialog', { name: 'Confirm deletion' });
      await expect(confirm.getByText(`Are you sure you want to delete ${name}?`)).toBeVisible();
      await confirm.getByRole('button', { name: 'Delete', exact: true }).click();
      await expect(page.getByRole('status').filter({ hasText: 'SSO provider deleted' })).toBeVisible();
      // Deleting resets the provider: its credentials are cleared and it is offered for setup again.
      await expect(row.getByText('Inactive', { exact: true })).toBeVisible();
      await expect(row.getByRole('button', { name: 'Configure' })).toBeVisible();
      const [reset] = await sql<{ clientId: string; isEnabled: boolean }>('SELECT "clientId", "isEnabled" FROM sso_providers WHERE id = $1', [id]);
      expect(reset).toEqual({ clientId: '<empty>', isEnabled: false });
    } finally {
      await removeProvider(id);
    }
  });

  test('deleting a custom provider removes it from the list', async ({ page }) => {
    // Known bug: DELETE /admin/sso-providers/:id never deletes the row; it blanks the
    // credentials and disables it (right for the built-in Google/Azure entries, which come back
    // from the environment anyway). A custom OIDC provider therefore stays on the Authentication
    // page as an unconfigured "Configure" card after the admin deleted it, with no way to remove
    // it from the UI. Remove `fail` once a deleted custom provider is gone.
    test.fail();
    const slug = `e2e-sso-${Date.now().toString(36)}`;
    const name = `E2E SSO ${slug.slice(8)}`;
    const id = await createOidcProvider(slug, name);
    try {
      await page.goto('/admin/settings');
      const row = card(page, name);
      await row.getByRole('button', { name: `Delete ${name}` }).click();
      await page.getByRole('dialog', { name: 'Confirm deletion' }).getByRole('button', { name: 'Delete', exact: true }).click();
      await expect(page.getByRole('status').filter({ hasText: 'SSO provider deleted' })).toBeVisible();
      await expect(row).toHaveCount(0, { timeout: 5_000 });
    } finally {
      await removeProvider(id);
    }
  });

  test('the status switch on a provider card enables a configured provider', async ({ page }) => {
    // Known bug: the card's switch sends PUT /admin/sso-providers/:id with only `is_enabled: true`.
    // The API requires a validation token to enable a provider, answers 401 InvalidToken, and the
    // page shows "The access token is invalid or expired." while the provider stays disabled.
    // Enabling only works through Configure with a re-entered secret. Remove `fail` once fixed.
    test.fail();
    const slug = `e2e-sso-${Date.now().toString(36)}`;
    const name = `E2E SSO ${slug.slice(8)}`;
    const id = await createOidcProvider(slug, name);
    try {
      await page.goto('/admin/settings');
      const row = card(page, name);
      const put = page.waitForResponse((r) => r.request().method() === 'PUT' && r.url().includes(`/admin/sso-providers/${id}`));
      await row.getByRole('checkbox', { name: `Toggle ${name} provider status` }).check({ force: true });
      const response = await put;
      test.info().annotations.push({ type: 'PUT /admin/sso-providers/:id', description: `${response.status()} ${await response.text()}` });
      expect(response.status()).toBe(200);
      await expect(row.getByText('Active', { exact: true })).toBeVisible();
      expect(await isEnabled(id)).toBe(true);
    } finally {
      await removeProvider(id);
    }
  });

  test('the last enabled provider can be neither disabled nor deleted', async ({ page }) => {
    const enabled = (await providers()).filter((p) => p.isEnabled).map((p) => p.provider);
    expect(enabled, 'only the suite IdP is enabled when this runs').toEqual([IDP_SLUG]);
    const [idp] = (await providers()).filter((p) => p.provider === IDP_SLUG);

    await page.goto('/admin/settings');
    const row = card(page, IDP_NAME);
    const toggle = row.getByRole('checkbox', { name: `Toggle ${IDP_NAME} provider status` });
    await expect(toggle).toBeChecked();

    await toggle.click({ force: true });
    await expect(page.getByRole('alert').filter({ hasText: LOCKOUT })).toBeVisible();
    await expect(toggle).toBeChecked();
    await expect(row.getByText('Active', { exact: true })).toBeVisible();
    expect(await isEnabled(idp.id)).toBe(true);

    await row.getByRole('button', { name: `Delete ${IDP_NAME}` }).click();
    const confirm = page.getByRole('dialog', { name: 'Confirm deletion' });
    await confirm.getByRole('button', { name: 'Delete', exact: true }).click();
    // The open dialog is aria-modal, so the toast behind it is reached by its text.
    await expect(page.getByText(LOCKOUT).first()).toBeVisible();
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    await expect(row).toBeVisible();
    expect(await isEnabled(idp.id)).toBe(true);
  });
});
