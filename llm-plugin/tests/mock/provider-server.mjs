#!/usr/bin/env node
// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// A dependency-free mock LLM provider, speaking both the OpenAI-compatible and Anthropic wire
// formats, for exercising the declarative provider plugin without spending real tokens.
//
//   node provider-server.mjs [--port N]
//
// Prints `{"port":N}` on the first stdout line, then serves. Scenarios are chosen from the last
// user message: say "weather" for a tool call, "search" for web search, anything else for plain
// text. Requests are recorded and readable from `GET /__requests`.
//
// The frames below deliberately reproduce real provider quirks that have broken this runtime
// before: `content: null` beside tool calls, a tool id repeated in every fragment, CRLF frames,
// comment keepalives, JSON split mid-token across writes, and `[DONE]` after a finish reason.

import { createServer } from 'node:http';

const portArgument = process.argv.indexOf('--port');
const port = portArgument === -1 ? 0 : Number(process.argv[portArgument + 1]);

/** Every request the plugin sent, so tests can assert on the outgoing payload. */
const received = [];

const MODEL = 'mock-model';

/** Writes SSE frames with a delay between them, so decoding is genuinely incremental. */
async function writeFrames(response, frames, { gapMs = 5 } = {}) {
  response.writeHead(200, {
    'Content-Type': 'text/event-stream; charset=utf-8',
    'Cache-Control': 'no-cache',
    Connection: 'close',
  });
  for (const frame of frames) {
    if (response.writableEnded) return;
    response.write(frame);
    await new Promise((resolve) => setTimeout(resolve, gapMs));
  }
  response.end();
}

/** One `data:` frame. `raw` frames are emitted verbatim so tests can inject wire oddities. */
const data = (payload) => `data: ${JSON.stringify(payload)}\n\n`;

// ---------------------------------------------------------------------------
// OpenAI-compatible: POST /v1/chat/completions
// ---------------------------------------------------------------------------

function openaiTextFrames() {
  return [
    ': keepalive\n\n',
    data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: { role: 'assistant', content: '' } }] }),
    data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: { content: 'Hello' } }] }),
    // A frame split mid-JSON across two writes.
    'data: {"id":"chatcmpl-mock","choices":[{"index":0,"delta":{"content":" fro',
    'm the mock"}}]}\n\n',
    // CRLF framing, which the spec allows and some gateways emit.
    `data: ${JSON.stringify({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: { content: ' server' } }] })}\r\n\r\n`,
    data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] }),
    // Usage arrives after the finish reason, in a chunk with no choices.
    data({
      id: 'chatcmpl-mock',
      choices: [],
      usage: { prompt_tokens: 11, completion_tokens: 4, total_tokens: 15 },
    }),
    // `[DONE]` after a finish reason must not produce a second completion.
    'data: [DONE]\n\n',
  ];
}

function openaiToolCallFrames(toolName) {
  const call = (fields) => ({
    id: 'chatcmpl-mock',
    choices: [{ index: 0, delta: { role: 'assistant', content: null, tool_calls: [fields] } }],
  });
  return [
    // `content: null` beside a tool call: a `notNull` guard must skip it rather than fail.
    data(call({ index: 0, id: 'call_mock_1', type: 'function', function: { name: toolName, arguments: '' } })),
    // The id repeats in every fragment, as several OpenAI-compatible vendors do.
    data(call({ index: 0, id: 'call_mock_1', function: { arguments: '{"ci' } })),
    data(call({ index: 0, id: 'call_mock_1', function: { arguments: 'ty":"Paris"}' } })),
    data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] }),
    'data: [DONE]\n\n',
  ];
}

function openaiToolAnswerFrames(body) {
  const toolMessage = [...(body.messages ?? [])].reverse().find((message) => message.role === 'tool');
  let observed = 'nothing';
  try {
    const parsed = JSON.parse(toolMessage?.content ?? 'null');
    observed = parsed?.content?.[0]?.text ?? JSON.stringify(parsed);
  } catch {
    observed = String(toolMessage?.content ?? 'nothing');
  }
  return [
    data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: { content: `The tool said: ${observed}` } }] }),
    data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] }),
    'data: [DONE]\n\n',
  ];
}

function openaiWebSearchFrames() {
  const citation = (title, url) => ({ type: 'url_citation', url_citation: { title, url, start_index: 0, end_index: 9 } });
  return [
    data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: { content: 'Rust 1.90' } }] }),
    data({
      id: 'chatcmpl-mock',
      choices: [
        {
          index: 0,
          delta: {
            annotations: [
              citation('Rust Releases', 'https://releases.rs/'),
              citation('Rust Blog', 'https://blog.rust-lang.org/'),
              // No url: must be skipped, not fail the stream.
              { type: 'url_citation', url_citation: { title: 'Broken citation' } },
            ],
          },
        },
      ],
    }),
    data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] }),
    'data: [DONE]\n\n',
  ];
}

// ---------------------------------------------------------------------------
// Anthropic: POST /v1/messages
// ---------------------------------------------------------------------------

const anthropicOpen = () => [
  data({
    type: 'message_start',
    message: {
      id: 'msg_mock',
      role: 'assistant',
      usage: {
        input_tokens: 12,
        output_tokens: 0,
        cache_read_input_tokens: 5,
        cache_creation_input_tokens: 3,
      },
    },
  }),
  'event: ping\ndata: {"type":"ping"}\n\n',
];

const anthropicClose = (stopReason) => [
  data({ type: 'message_delta', delta: { stop_reason: stopReason }, usage: { output_tokens: 7 } }),
  data({ type: 'message_stop' }),
];

function anthropicTextFrames() {
  return [
    ...anthropicOpen(),
    data({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } }),
    data({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'Hello from' } }),
    data({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: ' the mock server' } }),
    data({ type: 'content_block_stop', index: 0 }),
    ...anthropicClose('end_turn'),
  ];
}

/**
 * Web search and a client tool interleaved in ONE content-block index space, both streaming
 * identical `input_json_delta` payloads. Routing these apart is the hard part of the format.
 */
function anthropicWebSearchFrames({ withClientTool, toolName }) {
  const frames = [
    ...anthropicOpen(),
    data({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } }),
    data({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'Let me search. ' } }),
    data({ type: 'content_block_stop', index: 0 }),
    // Block 1: the provider's own search.
    data({
      type: 'content_block_start',
      index: 1,
      content_block: { type: 'server_tool_use', id: 'srvtoolu_mock', name: 'web_search', input: {} },
    }),
    data({ type: 'content_block_delta', index: 1, delta: { type: 'input_json_delta', partial_json: '{"que' } }),
    data({ type: 'content_block_delta', index: 1, delta: { type: 'input_json_delta', partial_json: 'ry":"rust version"}' } }),
    data({ type: 'content_block_stop', index: 1 }),
    // Block 2: its results, carrying a payload no caller should ever see.
    data({
      type: 'content_block_start',
      index: 2,
      content_block: {
        type: 'web_search_tool_result',
        tool_use_id: 'srvtoolu_mock',
        content: [
          {
            type: 'web_search_result',
            title: 'Rust Releases',
            url: 'https://releases.rs/',
            page_age: 'June 2, 2026',
            encrypted_content: 'MOCK_ENCRYPTED_BLOB_'.repeat(64),
          },
          {
            type: 'web_search_result',
            url: 'https://blog.rust-lang.org/',
            encrypted_content: 'MOCK_ENCRYPTED_BLOB_'.repeat(64),
          },
        ],
      },
    }),
    data({ type: 'content_block_stop', index: 2 }),
  ];
  if (!withClientTool) {
    return [...frames, ...anthropicClose('end_turn')];
  }
  // Block 3: a client tool, in the same index space, with the same delta shape as block 1.
  return [
    ...frames,
    data({
      type: 'content_block_start',
      index: 3,
      content_block: { type: 'tool_use', id: 'toolu_mock', name: toolName, input: {} },
    }),
    data({ type: 'content_block_delta', index: 3, delta: { type: 'input_json_delta', partial_json: '{"city"' } }),
    data({ type: 'content_block_delta', index: 3, delta: { type: 'input_json_delta', partial_json: ':"Paris"}' } }),
    data({ type: 'content_block_stop', index: 3 }),
    ...anthropicClose('tool_use'),
  ];
}

function anthropicToolAnswerFrames(body) {
  const lastUser = [...(body.messages ?? [])].reverse().find((message) => message.role === 'user');
  const block = Array.isArray(lastUser?.content)
    ? lastUser.content.find((entry) => entry.type === 'tool_result')
    : undefined;
  let observed = 'nothing';
  try {
    const parsed = JSON.parse(block?.content ?? 'null');
    observed = parsed?.content?.[0]?.text ?? JSON.stringify(parsed);
  } catch {
    observed = String(block?.content ?? 'nothing');
  }
  return [
    ...anthropicOpen(),
    data({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } }),
    data({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: `The tool said: ${observed}` } }),
    data({ type: 'content_block_stop', index: 0 }),
    ...anthropicClose('end_turn'),
  ];
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

function lastUserText(body) {
  const message = [...(body.messages ?? [])].reverse().find((entry) => entry.role === 'user');
  if (!message) return '';
  if (typeof message.content === 'string') return message.content;
  return (message.content ?? [])
    .map((part) => part.text ?? '')
    .join(' ');
}

const hasToolResult = (body) =>
  (body.messages ?? []).some(
    (message) =>
      message.role === 'tool' ||
      (Array.isArray(message.content) && message.content.some((entry) => entry.type === 'tool_result')),
  );

/** First client (non-native) tool name the plugin advertised. */
function clientToolName(body, dialect) {
  const tools = body.tools ?? [];
  if (dialect === 'anthropic') {
    return tools.find((tool) => tool.input_schema)?.name ?? 'unknown_tool';
  }
  return tools.find((tool) => tool.type === 'function')?.function?.name ?? 'unknown_tool';
}

function json(response, status, payload) {
  const body = JSON.stringify(payload);
  response.writeHead(status, { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(body) });
  response.end(body);
}

const server = createServer((request, response) => {
  const chunks = [];
  request.on('data', (chunk) => chunks.push(chunk));
  request.on('end', async () => {
    const raw = Buffer.concat(chunks).toString('utf8');
    const url = new URL(request.url, 'http://mock');
    let body = {};
    try {
      body = raw ? JSON.parse(raw) : {};
    } catch {
      body = { _unparsed: raw };
    }

    if (url.pathname !== '/__requests') {
      received.push({ method: request.method, path: url.pathname, headers: request.headers, body });
    }

    switch (`${request.method} ${url.pathname}`) {
      case 'GET /__requests':
        return json(response, 200, received);
      case 'POST /__reset':
        received.length = 0;
        return json(response, 200, { ok: true });

      case 'POST /v1/chat/completions': {
        const prompt = lastUserText(body).toLowerCase();
        if (hasToolResult(body)) return writeFrames(response, openaiToolAnswerFrames(body));
        if (prompt.includes('search')) return writeFrames(response, openaiWebSearchFrames());
        if (prompt.includes('weather') && (body.tools ?? []).length > 0) {
          return writeFrames(response, openaiToolCallFrames(clientToolName(body, 'openai')));
        }
        return writeFrames(response, openaiTextFrames());
      }

      case 'POST /v1/messages': {
        const prompt = lastUserText(body).toLowerCase();
        if (hasToolResult(body)) return writeFrames(response, anthropicToolAnswerFrames(body));
        if (prompt.includes('search')) {
          return writeFrames(
            response,
            anthropicWebSearchFrames({
              withClientTool: prompt.includes('weather'),
              toolName: clientToolName(body, 'anthropic'),
            }),
          );
        }
        if (prompt.includes('weather') && (body.tools ?? []).length > 0) {
          return writeFrames(response, [
            ...anthropicOpen(),
            data({
              type: 'content_block_start',
              index: 0,
              content_block: { type: 'tool_use', id: 'toolu_mock', name: clientToolName(body, 'anthropic'), input: {} },
            }),
            data({ type: 'content_block_delta', index: 0, delta: { type: 'input_json_delta', partial_json: '{"city":"Paris"}' } }),
            data({ type: 'content_block_stop', index: 0 }),
            ...anthropicClose('tool_use'),
          ]);
        }
        return writeFrames(response, anthropicTextFrames());
      }

      case 'POST /v1/embeddings': {
        const inputs = Array.isArray(body.input) ? body.input : [body.input];
        // Returned out of order on purpose: the runtime must sort by index.
        const items = inputs
          .map((_, index) => ({ index, embedding: [index + 0.5, index + 1.5, index + 2.5] }))
          .reverse();
        return json(response, 200, { data: items, usage: { total_tokens: inputs.length * 3 } });
      }

      case 'POST /v1/images/generations': {
        const count = body.n ?? 1;
        return json(response, 200, {
          data: Array.from({ length: count }, () => ({ b64_json: Buffer.from('mock-png').toString('base64') })),
        });
      }

      case 'GET /v1/models':
        return json(response, 200, {
          data: [
            { id: 'mock-model', display_name: 'Mock Model' },
            { id: 'mock-model-mini' },
          ],
        });

      // Error scenarios, including a 429 whose body is far larger than the error-body cap.
      case 'POST /v1/chaos/rate-limit':
        return json(response, 429, { error: { message: 'slow down: '.repeat(4096) } });
      case 'POST /v1/chaos/payment':
        return json(response, 402, { error: { message: 'billing required' } });
      case 'POST /v1/chaos/server':
        return json(response, 503, { error: { message: 'upstream unavailable' } });
      case 'POST /v1/chaos/not-sse':
        return json(response, 200, { choices: [{ delta: { content: 'not a stream' } }] });
      case 'POST /v1/chaos/truncated':
        // A stream that stops without any completion event.
        return writeFrames(response, [
          data({ id: 'chatcmpl-mock', choices: [{ index: 0, delta: { content: 'partial' } }] }),
        ]);

      default:
        return json(response, 404, { error: { message: `no mock route for ${request.method} ${url.pathname}` } });
    }
  });
});

server.listen(port, '127.0.0.1', () => {
  process.stdout.write(`${JSON.stringify({ port: server.address().port })}\n`);
});

for (const signal of ['SIGTERM', 'SIGINT']) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
