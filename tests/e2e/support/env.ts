// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

const required = (name: string) => {
  const value = process.env[name];
  if (!value) throw new Error(`${name} is not set (stack/inside.sh exports it)`);
  return value;
};

export const env = {
  apiUrl: process.env.E2E_API_URL ?? 'http://127.0.0.1:18200',
  webUrl: process.env.E2E_WEB_URL ?? 'http://127.0.0.1:4173',
  oidcUrl: `http://127.0.0.1:${process.env.OIDC_PORT ?? 9401}`,
  llmUrl: `http://127.0.0.1:${process.env.LLM_PORT ?? 9402}`,
  mcpUrl: `http://127.0.0.1:${process.env.MCP_PORT ?? 9404}`,
  catalogDir: process.env.CATALOG_DIR ?? '',
  databaseUrl: () => required('DATABASE_URL'),
  jwtSecret: () => required('JWT_SECRET'),
};
