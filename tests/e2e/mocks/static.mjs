// SPDX-FileCopyrightText: 2026 Perter Technology Solutions Private Limited
// SPDX-License-Identifier: Apache-2.0

// Serves a grengin-list master-data checkout as the metadata catalog (what meta.grengin.com
// serves in production), so the backend's discovery runs against real catalog files offline.
import http from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import path from 'node:path';

const port = Number(process.env.CATALOG_PORT ?? 9403);
const root = path.resolve(process.env.CATALOG_DIR ?? 'master-data');
const types = { '.json': 'application/json', '.svg': 'image/svg+xml', '.png': 'image/png', '.md': 'text/markdown' };

http
  .createServer(async (req, res) => {
    const file = path.resolve(root, '.' + decodeURIComponent(new URL(req.url, 'http://x').pathname));
    if (!file.startsWith(root + path.sep)) {
      res.writeHead(403);
      return res.end();
    }
    try {
      if (!(await stat(file)).isFile()) throw new Error('not a file');
      res.writeHead(200, { 'content-type': types[path.extname(file)] ?? 'application/octet-stream' });
      res.end(await readFile(file));
    } catch {
      res.writeHead(404);
      res.end();
    }
  })
  .listen(port, '127.0.0.1', () => console.log(`catalog ${root} on http://127.0.0.1:${port}`));
