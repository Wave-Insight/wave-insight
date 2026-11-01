// Verify the self-hosted editor: serve the built dist, block every external
// host, then assert the editor mounts and the language features still answer.
//
// Two ways to run it:
//   * `node run.mjs --offline` (the supported path): the runner starts the one
//     static server and exports QA_URL, this check reuses it. There used to be a
//     second server on the same port here, which made `-Offline` fail with
//     EADDRINUSE;
//   * standalone: `node monaco-offline.js` starts its own server on QA_PORT.
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');

const QA = __dirname;
const FIXTURES = path.join(QA, 'fixtures');
const DIST = process.env.QA_DIST ? path.resolve(process.env.QA_DIST) : path.join(QA, 'dist');
const PORT = Number(process.env.QA_PORT || 8918);
// when the runner already serves the build, reuse it (no second listener)
const EXTERNAL_URL = process.env.QA_URL || '';
const APP_URL = EXTERNAL_URL || 'http://127.0.0.1:' + PORT + '/';
const ORIGIN_RE = new RegExp('^' + APP_URL.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
const SHOTS = process.env.QA_SHOTS ? path.resolve(process.env.QA_SHOTS) : path.join(QA, 'artifacts');
fs.mkdirSync(SHOTS, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const types = {
  '.html': 'text/html; charset=utf-8', '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8', '.wasm': 'application/wasm', '.ttf': 'font/ttf',
  '.json': 'application/json', '.svg': 'image/svg+xml', '.map': 'application/json',
  '.vcd': 'text/plain', '.sv': 'text/plain', '.v': 'text/plain', '.txt': 'text/plain',
  '.fst': 'application/octet-stream', '.ghw': 'application/octet-stream',
  '.md': 'text/markdown; charset=utf-8', '.png': 'image/png',
};

function serve() {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const url = decodeURIComponent((req.url || '/').split('?')[0]);
      const file = path.join(DIST, url === '/' ? 'index.html' : url);
      if (!file.startsWith(DIST)) { res.writeHead(403).end('forbidden'); return; }
      fs.readFile(file, (err, data) => {
        if (err) { res.writeHead(404).end('not found'); return; }
        res.writeHead(200, { 'content-type': types[path.extname(file)] || 'application/octet-stream' });
        res.end(data);
      });
    });
    server.listen(PORT, '127.0.0.1', () => resolve(server));
  });
}

async function json(url) { const res = await fetch(url); return res.json(); }

class Cdp {
  constructor(ws) {
    this.ws = ws; this.id = 0; this.pending = new Map(); this.events = [];
    ws.onmessage = (event) => {
      const msg = JSON.parse(event.data);
      if (msg.id !== undefined) {
        const p = this.pending.get(msg.id);
        if (!p) return;
        this.pending.delete(msg.id);
        msg.error ? p.reject(new Error(JSON.stringify(msg.error))) : p.resolve(msg.result);
      } else this.events.push(msg);
    };
  }
  static async connect(url) {
    const ws = new WebSocket(url);
    await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
    return new Cdp(ws);
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => { this.pending.set(id, { resolve: res, reject: rej }); this.ws.send(JSON.stringify({ id, method, params })); });
  }
  async eval(expression) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error('eval: ' + JSON.stringify(r.exceptionDetails.exception || r.exceptionDetails.text));
    return r.result.value;
  }
  async upload(files) {
    const doc = await this.send('DOM.getDocument', { depth: -1 });
    const input = await this.send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: 'input[type=file]' });
    await this.send('DOM.setFileInputFiles', { nodeId: input.nodeId, files });
  }
}

(async () => {
  const server = EXTERNAL_URL ? null : await serve();
  const CDP_PORT = Number(process.env.CDP_PORT || 9222);
  const targets = await json('http://127.0.0.1:' + CDP_PORT + '/json/list');
  const page = targets.find((t) => t.type === 'page');
  const cdp = await Cdp.connect(page.webSocketDebuggerUrl);
  await cdp.send('Runtime.enable');
  await cdp.send('Page.enable');
  await cdp.send('Log.enable');
  await cdp.send('Network.enable');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });

  // Block every host the app used to fall back to; anything reaching them is a
  // regression, so we also assert it below from the network log.
  const blocked = ['*cdnjs.cloudflare.com*', '*unpkg.com*', '*jsdelivr.net*', '*fastly*'];
  await cdp.send('Network.setBlockedURLs', { urls: blocked });
  console.log('blocked: ' + blocked.join(', '));

  await cdp.send('Page.navigate', { url: APP_URL });
  for (let i = 0; i < 160; i++) {
    await sleep(250);
    try { if (await cdp.eval('!!window.__waveInsightLsp && !!document.querySelector(".app")')) break; } catch {}
  }
  await sleep(1000);

  const editorAppeared = async () => cdp.eval('!!(window.monaco && monaco.editor && monaco.editor.getEditors().length)');
  let mounted = false;
  for (let i = 0; i < 80; i++) {
    if (await editorAppeared()) { mounted = true; break; }
    await sleep(250);
  }

  const loader = await cdp.eval('JSON.stringify(Array.from(document.querySelectorAll("script[src]")).map(s => s.getAttribute("src")))');
  console.log('scripts: ' + loader);
  console.log('monaco namespace: ' + await cdp.eval('!!window.monaco'));

  const ok = [];
  ok.push(['editor mounted with every external host blocked', mounted]);
  ok.push(['loader is the shipped copy', /monaco\/vs\/loader\.js/.test(loader || '')]);
  ok.push(['monaco namespace present', await cdp.eval('!!window.monaco')]);

  // language features still answer from the shipped build
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv')]);
  for (let i = 0; i < 40; i++) {
    await sleep(250);
    if (await cdp.eval('document.querySelectorAll(".tab").length >= 2')) break;
  }
  await sleep(1200);
  await cdp.eval('(function(){ const t = Array.from(document.querySelectorAll(".tab .tab-name")).find(e => e.innerText === "cpu.sv"); if (t) t.click(); })()');
  await sleep(800);
  const symbols = await cdp.eval('(function(){\n    const hook = window.__waveInsightProviders;\n    if (!hook) return "no-hook";\n    const uri = hook.documentUri("cpu.sv");\n    const model = monaco.editor.getModels().find(m => m.uri.toString() === uri);\n    if (!model) return "no-model";\n    const token = {isCancellationRequested:false, onCancellationRequested:function(){ return {dispose:function(){}}; }};\n    return Promise.resolve(hook.documentSymbol.provideDocumentSymbols(model, token)).then(v => JSON.stringify(v));\n  })()');
  console.log('documentSymbol: ' + String(symbols).slice(0, 120));
  ok.push(['documentSymbol answers from the shipped editor', typeof symbols === 'string' && symbols.startsWith('[') && symbols.length > 10]);
  ok.push(['systemverilog tokenizer loaded', await cdp.eval('!!monaco.languages.getLanguages().find(l => l.id === "systemverilog")')]);

  // no request may leave the local origin, and no asset may 404
  const requests = cdp.events.filter((e) => e.method === 'Network.requestWillBeSent').map((e) => e.params.request.url);
  const external = requests.filter((u) => !ORIGIN_RE.test(u) && !/^(data|blob|about):/.test(u));
  const failed = cdp.events
    .filter((e) => e.method === 'Network.loadingFailed')
    .map((e) => 'FAILED ' + e.params.errorText);
  const notOk = cdp.events
    .filter((e) => e.method === 'Network.responseReceived' && e.params.response.status >= 400)
    .map((e) => e.params.response.status + ' ' + e.params.response.url)
    .filter((line) => !line.includes('/favicon.ico'));
  const monacoRequests = requests.filter((u) => u.includes('/monaco/vs/'));
  console.log('requests total: ' + requests.length + ', monaco assets: ' + monacoRequests.length);
  console.log('requests to external hosts: ' + JSON.stringify(external.slice(0, 5)));
  console.log('failed loads: ' + JSON.stringify(failed.slice(0, 5)));
  console.log('http >= 400: ' + JSON.stringify(notOk.slice(0, 5)));
  ok.push(['no request leaves the local origin', external.length === 0]);
  ok.push(['no failed loads', failed.length === 0]);
  ok.push(['no 404 for shipped assets', notOk.length === 0]);

  const exceptions = cdp.events.filter((e) => e.method === 'Runtime.exceptionThrown')
    .map((e) => e.params.exceptionDetails.exception?.description || e.params.exceptionDetails.text);
  ok.push(['no exceptions', exceptions.length === 0]);
  if (exceptions.length) console.log('exceptions: ' + JSON.stringify(exceptions.slice(0, 3)));

  const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(SHOTS, 'monaco-offline.png'), Buffer.from(data, 'base64'));

  let failedCount = 0;
  for (const [name, value] of ok) {
    console.log((value ? 'PASS ' : 'FAIL ') + name);
    if (!value) failedCount += 1;
  }
  console.log('\n=== ' + (ok.length - failedCount) + '/' + ok.length + ' checks passed ===');
  if (server) server.close();
  process.exit(failedCount ? 1 : 0);
})().catch((e) => { console.error('failed: ' + (e && e.stack)); process.exit(1); });
