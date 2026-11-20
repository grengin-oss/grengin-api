// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// An OpenAI-compatible chat completions server with deterministic behaviour, steered by
// markers in the latest user message:
//   [slow:N]      stream N chunks, 400 ms apart (Stop and refresh-while-generating tests)
//   [tool:NAME]   call tool NAME once with {"text":"ping"}, then answer with the tool result
//   [fail]        respond 500
// Anything else streams "Mock reply: <first words>". Usage is always reported.
import http from 'node:http';

const port = Number(process.env.LLM_PORT ?? 9402);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
export const calls = [];

function lastUser(messages) {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === 'user') {
      const content = messages[i].content;
      return typeof content === 'string' ? content : (content ?? []).map((part) => part.text ?? '').join(' ');
    }
  }
  return '';
}

function plan(body) {
  const messages = body.messages ?? [];
  const text = lastUser(messages);
  const toolResult = messages.at(-1)?.role === 'tool' ? String(messages.at(-1).content) : null;
  if (/\[fail\]/.test(text)) return { fail: true };
  if (toolResult !== null) return { chunks: [`Tool said: ${toolResult}`] };
  const tool = text.match(/\[tool:([\w.-]+)\]/)?.[1];
  const offered = (body.tools ?? []).map((t) => t.function?.name);
  if (tool) {
    const name = offered.find((n) => n === tool || n?.endsWith(tool));
    if (name) return { tool: name };
  }
  const slow = Number(text.match(/\[slow:(\d+)\]/)?.[1] ?? 0);
  if (slow) return { chunks: Array.from({ length: slow }, (_, i) => `part${i + 1} `), delay: 400 };
  return { chunks: [`Mock reply: ${text.replace(/\s+/g, ' ').slice(0, 60)}`] };
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, `http://127.0.0.1:${port}`);
  if (url.pathname.endsWith('/models')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ object: 'list', data: [{ id: 'e2e-chat', object: 'model' }] }));
  }
  if (!url.pathname.endsWith('/chat/completions') || req.method !== 'POST') {
    res.writeHead(404);
    return res.end();
  }
  let raw = '';
  for await (const chunk of req) raw += chunk;
  const body = JSON.parse(raw || '{}');
  const step = plan(body);
  calls.push({ at: Date.now(), model: body.model, step });
  if (step.fail) {
    res.writeHead(500, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ error: { message: 'mock failure', type: 'server_error' } }));
  }
  const id = `chatcmpl-${Date.now()}`;
  const usage = { prompt_tokens: 12, completion_tokens: 7, total_tokens: 19 };
  if (!body.stream) {
    res.writeHead(200, { 'content-type': 'application/json' });
    const message = step.tool
      ? { role: 'assistant', content: null, tool_calls: [{ id: 'call_1', type: 'function', function: { name: step.tool, arguments: '{"text":"ping"}' } }] }
      : { role: 'assistant', content: step.chunks.join('') };
    return res.end(JSON.stringify({ id, object: 'chat.completion', model: body.model, choices: [{ index: 0, message, finish_reason: step.tool ? 'tool_calls' : 'stop' }], usage }));
  }
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
  const send = (payload) => res.write(`data: ${JSON.stringify({ id, object: 'chat.completion.chunk', model: body.model, ...payload })}\n\n`);
  let closed = false;
  req.on('close', () => { closed = true; });
  if (step.tool) {
    send({ choices: [{ index: 0, delta: { role: 'assistant', tool_calls: [{ index: 0, id: 'call_1', type: 'function', function: { name: step.tool, arguments: '{"text":"ping"}' } }] }, finish_reason: null }] });
    send({ choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] });
  } else {
    for (const piece of step.chunks) {
      if (closed) return;
      send({ choices: [{ index: 0, delta: { content: piece }, finish_reason: null }] });
      if (step.delay) await sleep(step.delay);
    }
    send({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] });
  }
  send({ choices: [], usage });
  res.write('data: [DONE]\n\n');
  res.end();
});

server.listen(port, '127.0.0.1', () => console.log(`mock LLM on http://127.0.0.1:${port}`));
