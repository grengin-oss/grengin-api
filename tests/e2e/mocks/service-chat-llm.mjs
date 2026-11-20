// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// A second OpenAI-compatible model for the chat specs (specs/chat/*), registered as its own AI
// engine by those specs only while they run. It answers with what it received, so specs can check
// what the backend sent to the model:
//   - every reply starts "Context: <ctxN-… tokens of all user turns, in order> | assistant turns: N"
//   - "| files: name=first line" for each file part (data: URL) the request carried
//   - [artifact:TITLE] answers with a Markdown artifact titled TITLE, streamed in small chunks
// Title requests are answered "Title: <text>" like mocks/llm.mjs.
import http from 'node:http';

const port = 9420 + Number(process.env.E2E_PORT_OFFSET ?? 0);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const partsOf = (content) => (typeof content === 'string' ? [{ type: 'text', text: content }] : (content ?? []));
const textOf = (content) => partsOf(content).map((part) => part?.text ?? '').join(' ');

function filesOf(messages) {
  const files = [];
  for (const message of messages) {
    for (const part of partsOf(message.content)) {
      const data = part?.file?.file_data;
      const match = typeof data === 'string' ? data.match(/^data:[^;]*;base64,(.*)$/s) : null;
      if (!match) continue;
      const firstLine = Buffer.from(match[1], 'base64').toString('utf8').split('\n')[0].trim();
      files.push(`${part.file.filename ?? 'file'}=${firstLine}`);
    }
  }
  return files;
}

function reply(body) {
  const messages = body.messages ?? [];
  const users = messages.filter((m) => m.role === 'user').map((m) => textOf(m.content));
  const text = users.at(-1) ?? '';
  const system = messages.filter((m) => m.role === 'system' || m.role === 'developer').map((m) => textOf(m.content)).join(' ');
  if (/conversation title/i.test(system)) {
    return [`Title: ${text.replace(/\[[^\]]*\]/g, '').trim().slice(0, 40) || 'E2E chat'}`];
  }
  const artifact = text.match(/\[artifact:([^\]]+)\]/)?.[1];
  if (artifact) {
    const full = `Here is the requested note.\n<artifact type="text/markdown" title="${artifact}">\n# Heading ${artifact}\n\nArtifact body line.\n</artifact>\nDone.`;
    // Small chunks, so the backend's artifact tag parser sees tags split across chunks.
    return full.match(/[\s\S]{1,7}/g);
  }
  const tokens = users.flatMap((u) => u.match(/ctx\d-[a-z0-9]+/g) ?? []);
  const assistants = messages.filter((m) => m.role === 'assistant').length;
  const files = filesOf(messages);
  return [`Context: ${tokens.join(' > ') || 'none'} | assistant turns: ${assistants}${files.length ? ` | files: ${files.join(', ')}` : ''}`];
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${port}`);
  if (url.pathname.endsWith('/models')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ object: 'list', data: [{ id: 'e2e-context', object: 'model' }] }));
  }
  if (!url.pathname.endsWith('/chat/completions') || req.method !== 'POST') {
    res.writeHead(404);
    return res.end();
  }
  let raw = '';
  for await (const chunk of req) raw += chunk;
  const body = JSON.parse(raw || '{}');
  const chunks = reply(body);
  const id = `chatcmpl-ctx-${Date.now()}`;
  const usage = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
  if (!body.stream) {
    res.writeHead(200, { 'content-type': 'application/json' });
    const message = { role: 'assistant', content: chunks.join('') };
    return res.end(JSON.stringify({ id, object: 'chat.completion', model: body.model, choices: [{ index: 0, message, finish_reason: 'stop' }], usage }));
  }
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
  const send = (payload) => res.write(`data: ${JSON.stringify({ id, object: 'chat.completion.chunk', model: body.model, ...payload })}\n\n`);
  for (const piece of chunks) {
    send({ choices: [{ index: 0, delta: { content: piece }, finish_reason: null }] });
    if (chunks.length > 1) await sleep(20);
  }
  send({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] });
  send({ choices: [], usage });
  res.write('data: [DONE]\n\n');
  res.end();
});

server.listen(port, '127.0.0.1', () => console.log(`chat context mock LLM on http://127.0.0.1:${port}`));
