// Static server for the QA harness: the staged build at /, the fixtures at
// /fixtures/ (so a scenario can fetch a fixture from the page, e.g. the drop
// test, without copying fixtures into the build).
//
//   node serve.js <dist-dir> [port] [fixtures-dir]
const http = require('node:http');
const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(process.argv[2] || '.');
const port = Number(process.argv[3] || 8917);
const fixtures = process.argv[4] ? path.resolve(process.argv[4]) : null;
const FIXTURE_PREFIX = '/fixtures/';

const types = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.wasm': 'application/wasm', '.ttf': 'font/ttf',
  '.json': 'application/json', '.svg': 'image/svg+xml', '.map': 'application/json',
  '.vcd': 'text/plain', '.sv': 'text/plain', '.v': 'text/plain', '.txt': 'text/plain',
  '.fst': 'application/octet-stream', '.ghw': 'application/octet-stream',
  '.md': 'text/markdown; charset=utf-8', '.png': 'image/png',
};

function send(res, code, body, headers = {}) {
  res.writeHead(code, Object.assign({ 'cache-control': 'no-store, must-revalidate' }, headers));
  res.end(body);
}

http.createServer((req, res) => {
  const url = decodeURIComponent((req.url || '/').split('?')[0]);
  let base = root;
  let relative = url === '/' ? 'index.html' : url;
  if (fixtures && url.startsWith(FIXTURE_PREFIX)) {
    base = fixtures;
    relative = url.slice(FIXTURE_PREFIX.length);
  }
  const file = path.resolve(base, '.' + path.posix.join('/', relative));
  // path.resolve + the prefix check rejects ../ escapes for both roots
  if (!file.startsWith(base + path.sep) && file !== base) { send(res, 403, 'forbidden'); return; }
  fs.readFile(file, (err, data) => {
    if (err) { send(res, 404, 'not found'); return; }
    send(res, 200, data, { 'content-type': types[path.extname(file)] || 'application/octet-stream' });
  });
}).listen(port, '127.0.0.1', () => {
  console.log('serving ' + root + ' on ' + port + (fixtures ? ' (fixtures: ' + fixtures + ')' : ''));
});
