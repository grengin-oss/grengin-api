// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// OpenAI-compatible chat completions server for the projects/skills specs. It reports which
// markers (E2EP + uppercase letters/digits) occur in the system prompt, so a spec can see whether
// project instructions or skill instructions/knowledge reached the model:
//   "Echo <first 40 chars of the last user message>: markers=E2EPABC,E2EPDEF"   or   "...: markers=none"
// Title requests answer "Title: ..." like the shared mock.
import http from 'node:http';

const port = 9450 + Number(process.env.E2E_PORT_OFFSET ?? 0);
export const MODEL_ID = 'e2e-projects-echo-chat';

const textOf = (content) =>
  typeof content === 'string' ? content : (content ?? []).map((part) => part?.text ?? '').join(' ');

function reply(body) {
  const messages = body.messages ?? [];
  const system = messages
    .filter((m) => m.role === 'system' || m.role === 'developer')
    .map((m) => textOf(m.content))
    .join('\n');
  const lastUser = [...messages].reverse().find((m) => m.role === 'user');
  const text = textOf(lastUser?.content).replace(/\s+/g, ' ').trim();
  if (/conversation title/i.test(system)) return `Title: ${text.slice(0, 40) || 'E2E chat'}`;
  const markers = [...new Set(system.match(/\bE2EP[A-Z0-9]+\b/g) ?? [])].sort();
  return `Echo ${text.slice(0, 40)}: markers=${markers.length ? markers.join(',') : 'none'}`;
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${port}`);
  if (url.pathname.endsWith('/models')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ object: 'list', data: [{ id: MODEL_ID, object: 'model' }] }));
  }
  if (!url.pathname.endsWith('/chat/completions') || req.method !== 'POST') {
    res.writeHead(404);
    return res.end();
  }
  let raw = '';
  for await (const chunk of req) raw += chunk;
  const body = JSON.parse(raw || '{}');
  const content = reply(body);
  const id = `chatcmpl-${Date.now()}`;
  const usage = { prompt_tokens: 12, completion_tokens: 7, total_tokens: 19 };
  if (!body.stream) {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(
      JSON.stringify({
        id,
        object: 'chat.completion',
        model: body.model,
        choices: [{ index: 0, message: { role: 'assistant', content }, finish_reason: 'stop' }],
        usage,
      }),
    );
  }
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
  const send = (payload) =>
    res.write(`data: ${JSON.stringify({ id, object: 'chat.completion.chunk', model: body.model, ...payload })}\n\n`);
  send({ choices: [{ index: 0, delta: { role: 'assistant', content }, finish_reason: null }] });
  send({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] });
  send({ choices: [], usage });
  res.write('data: [DONE]\n\n');
  res.end();
});

server.listen(port, '127.0.0.1', () => console.log(`projects echo LLM on http://127.0.0.1:${port}`));
