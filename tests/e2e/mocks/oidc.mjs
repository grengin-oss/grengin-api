// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// A minimal OpenID Connect provider for end-to-end tests: discovery, JWKS, an HTML login
// page with one button per test user, authorization code + PKCE (S256), id_token, userinfo.
import crypto from 'node:crypto';
import http from 'node:http';
import { USERS } from './users.mjs';

const port = Number(process.env.OIDC_PORT ?? 9401);
const issuer = `http://127.0.0.1:${port}`;
const { publicKey, privateKey } = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'e2e', use: 'sig', alg: 'RS256' };
const codes = new Map();
const accessTokens = new Map();

const b64url = (value) => Buffer.from(value).toString('base64url');
function sign(claims) {
  const head = b64url(JSON.stringify({ alg: 'RS256', typ: 'JWT', kid: 'e2e' }));
  const body = b64url(JSON.stringify(claims));
  const signature = crypto.sign('sha256', Buffer.from(`${head}.${body}`), privateKey);
  return `${head}.${body}.${signature.toString('base64url')}`;
}

const escape = (value) => String(value).replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
function json(res, status, body) {
  res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  res.end(JSON.stringify(body));
}
async function readBody(req) {
  let raw = '';
  for await (const chunk of req) raw += chunk;
  return new URLSearchParams(raw);
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, issuer);
  try {
    if (url.pathname === '/.well-known/openid-configuration') {
      return json(res, 200, {
        issuer,
        authorization_endpoint: `${issuer}/authorize`,
        token_endpoint: `${issuer}/token`,
        userinfo_endpoint: `${issuer}/userinfo`,
        jwks_uri: `${issuer}/jwks`,
        response_types_supported: ['code'],
        subject_types_supported: ['public'],
        id_token_signing_alg_values_supported: ['RS256'],
        scopes_supported: ['openid', 'email', 'profile'],
        token_endpoint_auth_methods_supported: ['client_secret_basic', 'client_secret_post'],
        code_challenge_methods_supported: ['S256'],
        claims_supported: ['sub', 'email', 'email_verified', 'name', 'picture'],
      });
    }
    if (url.pathname === '/jwks') return json(res, 200, { keys: [jwk] });
    if (url.pathname === '/authorize' && req.method === 'GET') {
      const params = url.searchParams;
      const hidden = [...params].map(([k, v]) => `<input type="hidden" name="${escape(k)}" value="${escape(v)}">`).join('');
      const buttons = Object.entries(USERS)
        .map(([key, user]) => `<button type="submit" name="user" value="${escape(key)}" data-testid="login-${escape(key)}">Sign in as ${escape(user.email)}</button>`)
        .join('<br>');
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      return res.end(`<!doctype html><title>E2E identity provider</title><h1>E2E identity provider</h1><form method="post" action="/authorize">${hidden}${buttons}</form>`);
    }
    if (url.pathname === '/authorize' && req.method === 'POST') {
      const form = await readBody(req);
      const user = USERS[form.get('user')];
      if (!user || form.get('response_type') !== 'code') return json(res, 400, { error: 'invalid_request' });
      const code = crypto.randomBytes(24).toString('base64url');
      codes.set(code, {
        user,
        clientId: form.get('client_id'),
        redirectUri: form.get('redirect_uri'),
        nonce: form.get('nonce'),
        challenge: form.get('code_challenge'),
        expires: Date.now() + 60_000,
      });
      const target = new URL(form.get('redirect_uri'));
      target.searchParams.set('code', code);
      if (form.get('state')) target.searchParams.set('state', form.get('state'));
      res.writeHead(302, { location: target.toString() });
      return res.end();
    }
    if (url.pathname === '/token' && req.method === 'POST') {
      const form = await readBody(req);
      const grant = codes.get(form.get('code'));
      codes.delete(form.get('code'));
      let clientId = form.get('client_id');
      const basic = req.headers.authorization?.match(/^Basic (.+)$/i);
      if (basic) clientId = decodeURIComponent(Buffer.from(basic[1], 'base64').toString().split(':')[0]);
      if (!grant || grant.expires < Date.now() || grant.clientId !== clientId || grant.redirectUri !== form.get('redirect_uri')) {
        return json(res, 400, { error: 'invalid_grant' });
      }
      if (grant.challenge) {
        const verifier = form.get('code_verifier') ?? '';
        const derived = crypto.createHash('sha256').update(verifier).digest('base64url');
        if (derived !== grant.challenge) return json(res, 400, { error: 'invalid_grant' });
      }
      const now = Math.floor(Date.now() / 1000);
      const claims = { sub: grant.user.sub, email: grant.user.email, email_verified: true, name: grant.user.name };
      const accessToken = crypto.randomBytes(24).toString('base64url');
      accessTokens.set(accessToken, claims);
      return json(res, 200, {
        access_token: accessToken,
        token_type: 'Bearer',
        expires_in: 3600,
        id_token: sign({ iss: issuer, aud: clientId, iat: now, exp: now + 3600, nonce: grant.nonce ?? undefined, ...claims }),
      });
    }
    if (url.pathname === '/userinfo') {
      const token = req.headers.authorization?.replace(/^Bearer /i, '');
      const claims = accessTokens.get(token);
      return claims ? json(res, 200, claims) : json(res, 401, { error: 'invalid_token' });
    }
    json(res, 404, { error: 'not_found' });
  } catch (error) {
    json(res, 500, { error: 'server_error', message: String(error) });
  }
});

server.listen(port, '127.0.0.1', () => console.log(`mock OIDC provider on ${issuer}`));
