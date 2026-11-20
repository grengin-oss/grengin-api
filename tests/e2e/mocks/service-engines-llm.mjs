// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// A second OpenAI-compatible provider for the AI engines specs. It lists two models and names the
// model in every reply ("Picker reply from <model>: <text>"), so a spec can see which model the
// chat actually called. `[fail]` in the latest user message answers 500.
import http from 'node:http';

const port = 9410 + Number(process.env.E2E_PORT_OFFSET ?? 0);
const MODELS = ['e2e-search', 'e2e-plain'];

const textOf = (content) =>
  typeof content === 'string' ? content : (content ?? []).map((part) => part?.text ?? '').join(' ');

function reply(body) {
  const messages = body.messages ?? [];
  const user = [...messages].reverse().find((m) => m.role === 'user');
  const text = textOf(user?.content);
  const system = messages.filter((m) => m.role === 'system' || m.role === 'developer').map((m) => textOf(m.content)).join(' ');
  if (/conversation title/i.test(system)) return `Title: ${text.replace(/\[[^\]]*\]/g, '').trim().slice(0, 40) || 'E2E chat'}`;
  if (/\[fail\]/.test(text)) return null;
  return `Picker reply from ${body.model}: ${text.replace(/\s+/g, ' ').slice(0, 60)}`;
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${port}`);
  if (url.pathname.endsWith('/models')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ object: 'list', data: MODELS.map((id) => ({ id, object: 'model' })) }));
  }
  if (!url.pathname.endsWith('/chat/completions') || req.method !== 'POST') {
    res.writeHead(404);
    return res.end();
  }
  let raw = '';
  for await (const chunk of req) raw += chunk;
  const body = JSON.parse(raw || '{}');
  const content = reply(body);
  if (content === null) {
    res.writeHead(500, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ error: { message: 'mock failure', type: 'server_error' } }));
  }
  const id = `chatcmpl-${Date.now()}`;
  const usage = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
  if (!body.stream) {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ id, object: 'chat.completion', model: body.model, choices: [{ index: 0, message: { role: 'assistant', content }, finish_reason: 'stop' }], usage }));
  }
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
  const send = (payload) => res.write(`data: ${JSON.stringify({ id, object: 'chat.completion.chunk', model: body.model, ...payload })}\n\n`);
  send({ choices: [{ index: 0, delta: { role: 'assistant', content }, finish_reason: null }] });
  send({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] });
  send({ choices: [], usage });
  res.write('data: [DONE]\n\n');
  res.end();
});

server.listen(port, '127.0.0.1', () => console.log(`engines mock LLM on http://127.0.0.1:${port}`));
