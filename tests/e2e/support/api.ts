// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Setup-only helpers. Tests exercise the product through the UI; these prepare state the UI
// cannot reach yet (the very first sign-in provider) and read state back for assertions.
import { createHmac } from 'node:crypto';
import pg from 'pg';
import { env } from './env';

export async function sql<T = Record<string, unknown>>(text: string, params: unknown[] = []): Promise<T[]> {
  const client = new pg.Client({ connectionString: env.databaseUrl() });
  await client.connect();
  try {
    return (await client.query(text, params)).rows as T[];
  } finally {
    await client.end();
  }
}

// The same short-lived access token the security suites mint for an existing user.
export function tokenFor(userId: string, email: string): string {
  const claims = { sub: email, user_id: userId, name: 'E2E setup', exp: Math.floor(Date.now() / 1000) + 900, token_use: 'access' };
  const b64 = (value: object) => Buffer.from(JSON.stringify(value)).toString('base64url');
  const body = `${b64({ alg: 'HS256', typ: 'JWT' })}.${b64(claims)}`;
  return `${body}.${createHmac('sha256', env.jwtSecret()).update(body).digest('base64url')}`;
}

export async function api(token: string, method: string, path: string, body?: unknown) {
  const response = await fetch(`${env.apiUrl}${path}`, {
    method,
    headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await response.text();
  let json: unknown = text;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    // keep text
  }
  return { status: response.status, json: json as any };
}

export async function waitFor(url: string, timeoutMs = 120_000) {
  const end = Date.now() + timeoutMs;
  let last = '';
  while (Date.now() < end) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
      last = `HTTP ${response.status}`;
    } catch (error) {
      last = String(error);
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(`${url} did not become ready: ${last}`);
}
