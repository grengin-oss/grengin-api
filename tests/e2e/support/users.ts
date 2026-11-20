// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Helpers for the users/roles/departments specs. People come from a second mock IdP
// (mocks/service-users-idp.mjs) that accepts any email, so each test brings people who have
// never signed in before and the shared identities in mocks/users.mjs stay untouched.
import { randomBytes } from 'node:crypto';
import { expect, type Browser, type BrowserContext, type Locator, type Page } from '@playwright/test';
import { USERS } from '../mocks/users.mjs';
import { api, sql, tokenFor } from './api';
import { env } from './env';
import { dismissOnboarding } from './login';

export const PEOPLE_IDP_SLUG = 'e2e-people';
export const PEOPLE_IDP_NAME = 'People Directory';
const peopleIdpUrl = `http://127.0.0.1:${process.env.USERS_IDP_PORT ?? 9470 + Number(process.env.E2E_PORT_OFFSET ?? 0)}`;

export interface Person {
  email: string;
  name: string;
}

export const suffix = () => randomBytes(4).toString('hex');

export function newPerson(label: string): Person {
  const tag = suffix();
  return { email: `${label}-${tag}@people.e2e.grengin.test`, name: `${label[0].toUpperCase()}${label.slice(1)} ${tag}` };
}

export async function ownerToken(): Promise<string> {
  const [owner] = await sql<{ id: string }>(
    'SELECT id FROM users WHERE lower(email) = lower($1) ORDER BY "createdAt" LIMIT 1',
    [USERS.owner.email],
  );
  return tokenFor(owner.id, USERS.owner.email);
}

// Setup: the people directory as a second sign-in provider with JIT provisioning, created
// through the admin API the same way global-setup adds the shared IdP.
export async function ensurePeopleIdp() {
  const token = await ownerToken();
  const providers = await api(token, 'GET', '/admin/sso-providers');
  if (JSON.stringify(providers.json ?? '').includes(`"${PEOPLE_IDP_SLUG}"`)) return;
  const created = await api(token, 'POST', '/admin/sso-providers', {
    provider: PEOPLE_IDP_SLUG,
    name: PEOPLE_IDP_NAME,
    client_id: 'grengin-e2e-people',
    client_secret: 'e2e-people-secret',
    issuer_url: peopleIdpUrl,
    redirect_url: `${env.webUrl}/auth/${PEOPLE_IDP_SLUG}/callback`,
    allowed_domains: [],
    jit_provisioning: true,
  });
  if (created.status >= 300) throw new Error(`creating the people IdP failed: ${created.status} ${JSON.stringify(created.json)}`);
  if (created.json?.is_enabled === false) {
    const validated = await api(token, 'POST', `/admin/sso-providers/${created.json.id}/validate`, {});
    if (validated.status >= 300 || !validated.json?.validation_token) {
      throw new Error(`validating the people IdP failed: ${validated.status} ${JSON.stringify(validated.json)}`);
    }
    const enabled = await api(token, 'PUT', `/admin/sso-providers/${created.json.id}`, {
      is_enabled: true,
      validation_token: validated.json.validation_token,
    });
    if (enabled.status >= 300) throw new Error(`enabling the people IdP failed: ${enabled.status} ${JSON.stringify(enabled.json)}`);
  }
}

export async function personContext(browser: Browser): Promise<{ context: BrowserContext; page: Page }> {
  const context = await browser.newContext({ storageState: { cookies: [], origins: [] }, baseURL: env.webUrl });
  return { context, page: await context.newPage() };
}

// Login page -> people directory -> webapp callback. Like support/login.ts it starts the flow the
// provider button starts when the login page does not list configured providers (known bug).
export async function startSignIn(page: Page, person: Person) {
  await page.goto('/');
  await page.getByRole('heading', { level: 1 }).first().waitFor();
  const button = page.getByRole('button', { name: new RegExp(PEOPLE_IDP_NAME) });
  if (await button.isVisible().catch(() => false)) {
    await button.click();
  } else {
    const redirect = new URL(`/auth/${PEOPLE_IDP_SLUG}/callback`, page.url()).toString();
    await page.evaluate((slug) => sessionStorage.setItem('oauth_provider', slug), PEOPLE_IDP_SLUG);
    await page.goto(`/api/auth/${PEOPLE_IDP_SLUG}?redirect_uri=${encodeURIComponent(redirect)}`);
  }
  await expect(page.getByRole('heading', { name: 'People directory' })).toBeVisible();
  await page.getByLabel('Email').fill(person.email);
  await page.getByLabel('Full name').fill(person.name);
  await page.getByRole('button', { name: 'Continue' }).click();
}

export async function signInPerson(page: Page, person: Person) {
  await startSignIn(page, person);
  await expect(composer(page)).toBeVisible({ timeout: 30_000 });
  await dismissOnboarding(page);
}

// The callback page reports a refused sign-in and shows the API's reason as a toast.
export async function expectSignInRefused(page: Page, reason: RegExp) {
  await expect(page.getByRole('heading', { name: 'Sign in failed' })).toBeVisible({ timeout: 30_000 });
  await expect(page.getByRole('alert').filter({ hasText: reason })).toBeVisible();
}

export const composer = (page: Page) => page.getByRole('textbox', { name: 'Message input' });

export async function userByEmail(email: string) {
  const [row] = await sql<{ id: string; status: string; department_id: string | null }>(
    'SELECT id, status, "departmentId" AS department_id FROM users WHERE lower(email) = lower($1) AND status <> \'deleted\'',
    [email],
  );
  return row;
}

export async function rolesOf(userId: string): Promise<string[]> {
  const rows = await sql<{ name: string }>(
    'SELECT r.name FROM user_role_assignments a JOIN roles r ON r.id = a."roleId" WHERE a."userId" = $1 ORDER BY r.name',
    [userId],
  );
  return rows.map((row) => row.name);
}

// Setup for specs that are not about onboarding: a person signs in once (JIT, pending), is
// approved through the admin API, and signs in again. onboarding.spec.ts covers the UI approval.
export async function provisionPerson(browser: Browser, label: string) {
  await ensurePeopleIdp();
  const person = newPerson(label);
  const { context, page } = await personContext(browser);
  await startSignIn(page, person);
  await expectSignInRefused(page, /pending administrator approval/i);
  const user = await userByEmail(person.email);
  const approved = await api(await ownerToken(), 'PATCH', `/admin/users/${user.id}/status`, { status: 'active' });
  expect(approved.status, JSON.stringify(approved.json)).toBeLessThan(300);
  await signInPerson(page, person);
  return { person, context, page, id: user.id };
}

// ---------- admin UI (owner) ----------

// The legacy /admin/users route redirects to Organization > Users.
export async function openUsersAdmin(page: Page) {
  await page.goto('/admin/users');
  await page.waitForURL(/\/admin\/departments\?tab=users/);
  await expect(page.getByRole('tab', { name: 'Users' })).toHaveAttribute('aria-selected', 'true');
}

export async function findUserRow(page: Page, email: string): Promise<Locator> {
  await page.getByRole('textbox', { name: 'Search by name, email, or department...' }).fill(email);
  const row = page.getByRole('row').filter({ hasText: email });
  await expect(row).toHaveCount(1);
  return row;
}

export const statusCell = (row: Locator) => row.getByRole('cell').last();

export async function openUserMenu(page: Page) {
  const trigger = page.getByRole('button', { name: 'User menu' });
  if ((await trigger.getAttribute('aria-expanded')) !== 'true') await trigger.click();
  await expect(page.getByRole('menuitem', { name: 'Sign out' })).toBeVisible();
}

export async function openOrganization(page: Page) {
  await page.goto('/admin/departments');
  await expect(page.getByText('Organization', { exact: true }).first()).toBeVisible();
}

export async function createDepartment(page: Page, name: string) {
  await openOrganization(page);
  await page.getByRole('button', { name: 'Create Department' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Create Department' });
  await dialog.getByLabel(/^Name/).fill(name);
  await dialog.getByRole('button', { name: 'Create', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: 'Department created successfully' })).toBeVisible();
}

export function departmentTreeItem(page: Page, name: string) {
  return page.getByRole('treeitem', { name: new RegExp(`^${name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')},`) });
}

export async function selectDepartment(page: Page, name: string) {
  await page.getByRole('textbox', { name: 'Find a department...' }).fill(name);
  await departmentTreeItem(page, name).click();
  await expect(page.getByRole('region', { name: 'Details' }).getByText(name, { exact: true }).first()).toBeVisible();
}

export async function addDepartmentMember(page: Page, person: Person) {
  await page.getByRole('tab', { name: /^Members/ }).click();
  await page.getByRole('button', { name: 'Add Members' }).first().click();
  const dialog = page.getByRole('dialog', { name: 'Add Members' });
  await dialog.getByRole('textbox', { name: /Search by name, email, or department/ }).fill(person.email);
  await dialog.getByRole('checkbox', { name: `${person.name} (${person.email})` }).click();
  await dialog.getByRole('button', { name: 'Add 1 Member' }).click();
  await expect(dialog).toHaveCount(0);
  await expect(page.getByRole('cell', { name: person.email })).toBeVisible();
}

export async function setDepartmentBudget(page: Page, amount: string, action: 'Warn' | 'Block') {
  await page.getByRole('tab', { name: 'Budget' }).click();
  await page.getByRole('button', { name: 'Edit Budget' }).click();
  const dialog = page.getByRole('dialog', { name: 'Edit Budget' });
  await dialog.getByLabel('Budget Amount in USD').fill(amount);
  await dialog.getByLabel('Period').selectOption('monthly');
  await dialog.getByLabel('Action on Exceed').selectOption(action.toLowerCase());
  await dialog.getByRole('button', { name: 'Save Budget' }).click();
  await dialog.getByRole('button', { name: 'Confirm & Save' }).click();
  await expect(page.getByRole('status').filter({ hasText: 'Budget updated successfully' })).toBeVisible();
  await expect(dialog).toHaveCount(0);
}

export async function sendChat(page: Page, text: string) {
  await page.goto('/');
  await composer(page).waitFor();
  await dismissOnboarding(page);
  await composer(page).fill(text);
  await composer(page).press('Enter');
}
