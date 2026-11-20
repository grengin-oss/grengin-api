// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// A second OpenID Connect provider for the users/roles/departments specs. Unlike mocks/oidc.mjs
// it has no fixed list of people: its login page asks for an email and a name, so every test can
// bring a person who has never signed in before (JIT provisioning) without touching the shared
// identities. The subject is derived from the email, so the same email is the same identity.
import crypto from 'node:crypto';
import http from 'node:http';

const port = Number(process.env.USERS_IDP_PORT ?? 9470 + Number(process.env.E2E_PORT_OFFSET ?? 0));
const issuer = `http://127.0.0.1:${port}`;
const { publicKey, privateKey } = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'e2e-people', use: 'sig', alg: 'RS256' };
const codes = new Map();
const accessTokens = new Map();

const b64url = (value) => Buffer.from(value).toString('base64url');
function sign(claims) {
  const head = b64url(JSON.stringify({ alg: 'RS256', typ: 'JWT', kid: 'e2e-people' }));
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
        claims_supported: ['sub', 'email', 'email_verified', 'name'],
      });
    }
    if (url.pathname === '/jwks') return json(res, 200, { keys: [jwk] });
    if (url.pathname === '/authorize' && req.method === 'GET') {
      const hidden = [...url.searchParams].map(([k, v]) => `<input type="hidden" name="${escape(k)}" value="${escape(v)}">`).join('');
      res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' });
      return res.end(
        `<!doctype html><title>People directory</title><h1>People directory</h1>` +
          `<form method="post" action="/authorize">${hidden}` +
          `<label>Email <input type="email" name="email" required></label><br>` +
          `<label>Full name <input type="text" name="name"></label><br>` +
          `<button type="submit">Continue</button></form>`,
      );
    }
    if (url.pathname === '/authorize' && req.method === 'POST') {
      const form = await readBody(req);
      const email = (form.get('email') ?? '').trim().toLowerCase();
      if (!email || form.get('response_type') !== 'code') return json(res, 400, { error: 'invalid_request' });
      const user = { sub: `people-${email}`, email, name: (form.get('name') ?? '').trim() || email.split('@')[0] };
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
        const derived = crypto.createHash('sha256').update(form.get('code_verifier') ?? '').digest('base64url');
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
      const claims = accessTokens.get(req.headers.authorization?.replace(/^Bearer /i, ''));
      return claims ? json(res, 200, claims) : json(res, 401, { error: 'invalid_token' });
    }
    json(res, 404, { error: 'not_found' });
  } catch (error) {
    json(res, 500, { error: 'server_error', message: String(error) });
  }
});

server.listen(port, '127.0.0.1', () => console.log(`people directory (OIDC) on ${issuer}`));
