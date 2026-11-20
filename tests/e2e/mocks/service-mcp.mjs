// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// A remote MCP server (Streamable HTTP, JSON responses) on 127.0.0.1:$MCP_PORT, modelled on the
// mock in grengin-api's own src/services/mcp_client.rs tests:
//   POST /mcp            initialize (echoes protocolVersion), notifications/* -> 202,
//                        tools/list (echo, add), tools/call
//   GET  /calls          {count, held, calls[]} of every tools/call that reached the server
//   POST /calls/reset    forget recorded calls and release any held call
//   POST /hold/release   let a held call answer: echo with text "hold" waits for this
// It also serves a small OpenAI-compatible model at /v1 for the in-flight revocation test: on a
// user message containing [twice:NAME] it calls the offered tool ending with NAME twice in one
// turn ({"text":"hold"}, then {"text":"again"}), then answers "Gated done: <last tool result>".
import http from 'node:http';

const port = Number(process.env.MCP_PORT ?? 9404);

const TOOLS = [
  {
    name: 'echo',
    description: 'Echo the given text back',
    inputSchema: { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] },
  },
  {
    name: 'add',
    description: 'Add two numbers',
    inputSchema: { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } }, required: ['a', 'b'] },
  },
];

let calls = [];
let held = 0;
let releases = [];
const release = () => {
  for (const resolve of releases) resolve();
  releases = [];
};

async function readJson(req) {
  let raw = '';
  for await (const chunk of req) raw += chunk;
  return raw ? JSON.parse(raw) : {};
}

function json(res, status, body) {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(body));
}

async function callTool(name, args) {
  if (name === 'echo') {
    if (args?.text === 'hold') {
      held += 1;
      await new Promise((resolve) => {
        releases.push(resolve);
        setTimeout(resolve, 45_000);
      });
      held -= 1;
    }
    return { content: [{ type: 'text', text: `echo: ${args?.text ?? ''}` }], isError: false };
  }
  if (name === 'add') {
    return { content: [{ type: 'text', text: String(Number(args?.a ?? 0) + Number(args?.b ?? 0)) }], isError: false };
  }
  return null;
}

async function mcp(req, res) {
  if (req.method === 'GET') {
    res.writeHead(405, { allow: 'POST, DELETE' });
    return res.end();
  }
  if (req.method === 'DELETE') {
    res.writeHead(200);
    return res.end();
  }
  const message = await readJson(req);
  if (message.id === undefined || message.id === null) {
    res.writeHead(202);
    return res.end();
  }
  const reply = (result) => json(res, 200, { jsonrpc: '2.0', id: message.id, result });
  const fail = (code, text) => json(res, 200, { jsonrpc: '2.0', id: message.id, error: { code, message: text } });
  switch (message.method) {
    case 'initialize':
      return reply({
        protocolVersion: message.params?.protocolVersion ?? '2025-06-18',
        capabilities: { tools: {} },
        serverInfo: { name: 'e2e-mcp', version: '1' },
      });
    case 'ping':
      return reply({});
    case 'tools/list':
      return reply({ tools: TOOLS });
    case 'tools/call': {
      const { name, arguments: args } = message.params ?? {};
      calls.push({ at: Date.now(), name, arguments: args ?? {}, authorization: req.headers.authorization ?? null });
      const result = await callTool(name, args);
      return result ? reply(result) : fail(-32602, `unknown tool ${name}`);
    }
    default:
      return fail(-32601, `method not found: ${message.method}`);
  }
}

const textOf = (content) =>
  typeof content === 'string' ? content : (content ?? []).map((part) => part?.text ?? '').join(' ');

function gatedPlan(body) {
  const messages = body.messages ?? [];
  const system = messages.filter((m) => m.role === 'system' || m.role === 'developer').map((m) => textOf(m.content)).join(' ');
  const lastUser = messages.findLastIndex((m) => m.role === 'user');
  const text = lastUser >= 0 ? textOf(messages[lastUser].content) : '';
  if (/conversation title/i.test(system)) return { text: 'Title: gated MCP turn' };
  const results = messages.slice(lastUser + 1).filter((m) => m.role === 'tool');
  const wanted = text.match(/\[twice:([\w.-]+)\]/)?.[1];
  const name = wanted && (body.tools ?? []).map((t) => t.function?.name).find((n) => n === wanted || n?.endsWith(wanted));
  if (!name) return { text: 'Gated: no tool offered' };
  if (results.length >= 2) return { text: `Gated done: ${textOf(results.at(-1).content)}` };
  return { tool: name, id: `call_${results.length + 1}`, args: results.length === 0 ? '{"text":"hold"}' : '{"text":"again"}' };
}

async function llm(req, res, path) {
  if (path.endsWith('/models')) {
    return json(res, 200, { object: 'list', data: [{ id: 'e2e-gated', object: 'model' }] });
  }
  if (!path.endsWith('/chat/completions') || req.method !== 'POST') {
    res.writeHead(404);
    return res.end();
  }
  const body = await readJson(req);
  const step = gatedPlan(body);
  const id = `chatcmpl-gated-${Date.now()}`;
  const usage = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 };
  const toolCall = step.tool && { id: step.id, type: 'function', function: { name: step.tool, arguments: step.args } };
  if (!body.stream) {
    const message = toolCall ? { role: 'assistant', content: null, tool_calls: [toolCall] } : { role: 'assistant', content: step.text };
    return json(res, 200, { id, object: 'chat.completion', model: body.model, choices: [{ index: 0, message, finish_reason: toolCall ? 'tool_calls' : 'stop' }], usage });
  }
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache', connection: 'keep-alive' });
  const send = (payload) => res.write(`data: ${JSON.stringify({ id, object: 'chat.completion.chunk', model: body.model, ...payload })}\n\n`);
  if (toolCall) {
    send({ choices: [{ index: 0, delta: { role: 'assistant', tool_calls: [{ index: 0, ...toolCall }] }, finish_reason: null }] });
    send({ choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] });
  } else {
    send({ choices: [{ index: 0, delta: { role: 'assistant', content: step.text }, finish_reason: null }] });
    send({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] });
  }
  send({ choices: [], usage });
  res.write('data: [DONE]\n\n');
  res.end();
}

const server = http.createServer(async (req, res) => {
  try {
    const path = new URL(req.url, `http://127.0.0.1:${port}`).pathname;
    if (path === '/mcp') return await mcp(req, res);
    if (path === '/calls' && req.method === 'GET') return json(res, 200, { count: calls.length, held, calls });
    if (path === '/calls/reset' && req.method === 'POST') {
      calls = [];
      release();
      return json(res, 200, { count: 0 });
    }
    if (path === '/hold/release' && req.method === 'POST') {
      release();
      return json(res, 200, { released: true });
    }
    if (path.startsWith('/v1/')) return await llm(req, res, path);
    res.writeHead(404);
    res.end();
  } catch (error) {
    console.error(error);
    if (!res.headersSent) json(res, 500, { error: String(error) });
  }
});

server.listen(port, '127.0.0.1', () => console.log(`mock MCP server on http://127.0.0.1:${port}/mcp`));
