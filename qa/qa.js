// QA harness: boots the app in headless Chrome and walks the scenario matrix.
// Usage: node qa.js [scenario ...]  (default: all)
const fs = require('node:fs');
const path = require('node:path');

// All fixtures live in qa/fixtures (provenance in README.md); the harness never
// reads them from the staged build or from %TEMP%.
const QA = __dirname;
const FIXTURES = path.join(QA, 'fixtures');
// Screenshots are artifacts, not sources: qa/artifacts/ is gitignored.
const SHOTS = process.env.QA_SHOTS ? path.resolve(process.env.QA_SHOTS) : path.join(QA, 'artifacts');
fs.mkdirSync(SHOTS, { recursive: true });
// Ports are configurable so the runner can pick free ones (and so the offline
// check can share the static server instead of starting a second one).
const PORT = Number(process.env.QA_PORT || 8917);
const CDP_PORT = Number(process.env.CDP_PORT || 9222);
const APP_URL = 'http://127.0.0.1:' + PORT + '/';
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

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
    const cdp = new Cdp(ws);
    // A crashed target used to leave the harness hanging or exiting silently;
    // say so instead, with the pending call that never answered.
    ws.onclose = (event) => {
      const pending = Array.from(cdp.pending.keys());
      console.error('CDP closed: code=' + event.code + ' reason=' + (event.reason || '') + ' pending=' + pending.length);
      if (pending.length) { console.error('CDP pending ids: ' + pending.join(',')); }
      process.exit(2);
    };
    return cdp;
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((res, rej) => { this.pending.set(id, { resolve: res, reject: rej }); this.ws.send(JSON.stringify({ id, method, params })); });
  }
  async eval(expression, awaitPromise = true) {
    const r = await this.send('Runtime.evaluate', { expression, awaitPromise, returnByValue: true });
    if (r.exceptionDetails) throw new Error('eval: ' + JSON.stringify(r.exceptionDetails.exception || r.exceptionDetails.text));
    return r.result.value;
  }
  async shot(name) {
    const { data } = await this.send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(SHOTS, name), Buffer.from(data, 'base64'));
  }
  async mouse(type, x, y, extra = {}) {
    await this.send('Input.dispatchMouseEvent', Object.assign({ type, x, y, button: 'left', clickCount: 1 }, extra));
  }
  async clickAt(x, y, button = 'left') {
    await this.mouse('mousePressed', x, y, { button, buttons: button === 'left' ? 1 : 2 });
    await this.mouse('mouseReleased', x, y, { button, buttons: 0 });
  }
  async key(type, keyName, code, vk) {
    await this.send('Input.dispatchKeyEvent', { type, key: keyName, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk });
  }
  async press(keyName, code, vk) {
    await this.key('keyDown', keyName, code, vk);
    await this.key('keyUp', keyName, code, vk);
  }
  async upload(files) {
    // node ids go stale when the page is still loading, so retry with a fresh
    // document snapshot
    let lastError = null;
    for (let attempt = 0; attempt < 5; attempt++) {
      try {
        const doc = await this.send('DOM.getDocument', { depth: -1 });
        const input = await this.send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: 'input[type=file]' });
        if (!input.nodeId) throw new Error('no file input yet');
        // a file input that still holds the previous selection does not fire a
        // change event when the same file is selected again (the app does not
        // clear it), so scenarios that re-open the same dump would silently do
        // nothing; clear it first to simulate a fresh pick
        await this.eval('(function(){ var input = document.querySelector("input[type=file]"); if (input) { input.value = ""; } return true; })()');
        await this.send('DOM.setFileInputFiles', { nodeId: input.nodeId, files });
        return;
      } catch (error) {
        lastError = error;
        await sleep(500);
      }
    }
    throw lastError;
  }
}

const PAGE_HELPERS = `
window.__qa = {
  q(sel){ return document.querySelector(sel); },
  qa(sel){ return Array.from(document.querySelectorAll(sel)); },
  text(sel){ const e = this.q(sel); return e ? e.innerText : null; },
  texts(sel){ return this.qa(sel).map(e => e.innerText); },
  click(sel, i=0){ const e = this.qa(sel)[i]; if (!e) return 'no-element:'+sel; e.click(); return 'ok'; },
  clickText(sel, txt){ const e = this.qa(sel).find(x => (x.innerText||'').trim() === txt); if (!e) return 'no-element:'+txt; e.click(); return 'ok'; },
  // R3 labels a sidebar signal name[width-1:0] (e.g. pc[7:0]); the wave rows
  // keep the plain hierarchical path, so match on the base name here.
  clickSignal(name){
    const row = this.qa('.sidebar .sig-row .sig-name').find((e) => {
      const text = (e.innerText || '').trim();
      return text === name || text.startsWith(name + '[');
    });
    if (!row) return 'no-element:' + name;
    row.parentElement.click();
    return 'ok';
  },
  rect(sel){ const e = this.q(sel); if (!e) return null; const r = e.getBoundingClientRect(); return {x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height)}; },
  editor(){ const eds = (window.monaco&&monaco.editor.getEditors)?monaco.editor.getEditors():[]; if(!eds.length) return null; const e=eds[0]; const p=e.getPosition(); const m=e.getModel(); return {line:p&&p.lineNumber, column:p&&p.column, uri:m&&m.uri.toString(), first:m&&m.getLineContent(1).slice(0,40)}; },
  models(){ return ((window.monaco&&monaco.editor.getModels)?monaco.editor.getModels():[]).map(m=>({uri:m.uri.toString(), lines:m.getLineCount(), len:m.getValue().length})); },
  axis(){ return this.qa('.timebar-svg .axis-text').map(t => (t.textContent||'') + '@' + (t.getAttribute('x')||'')).join(','); },
  lsp(){ return window.__waveInsightLsp ? JSON.parse(window.__waveInsightLsp.stats()) : null; },
  lspDocs(){ return window.__waveInsightLsp ? JSON.parse(window.__waveInsightLsp.documents()) : null; },
  provider(kind, file, line, col){
    const hook = window.__waveInsightProviders;
    if (!hook) return 'no-hook';
    const uri = hook.documentUri(file);
    const model = monaco.editor.getModels().find(m => m.uri.toString() === uri);
    if (!model) return 'no-model:'+uri;
    const pos = {lineNumber: line, column: col};
    const token = {isCancellationRequested:false, onCancellationRequested:function(){ return {dispose:function(){}}; }};
    let res;
    if (kind === 'definition') res = hook.definition.provideDefinition(model, pos, token);
    else if (kind === 'references') res = hook.references.provideReferences(model, pos, {includeDeclaration:true}, token);
    else if (kind === 'hover') res = hook.hover.provideHover(model, pos, token);
    // completion is the only 4-argument provider (monaco passes the trigger
    // context); the hook wraps the registered provider itself
    else if (kind === 'completion') res = hook.completion.provideCompletionItems(model, pos, { triggerKind: 1 }, token);
    else if (kind === 'symbols') res = hook.documentSymbol.provideDocumentSymbols(model, token);
    return Promise.resolve(res).then(v => JSON.stringify(v));
  },
  valueTexts(){ return this.qa('.wave-row text.wave-value').map(t => t.textContent); },
  strokes(){ return this.qa('.wave-row polyline').map(p => p.getAttribute('stroke')); },
  // inflight tone of the status chip: the top bar colours errors differently
  statusTone(){
    const chip = this.q('.topbar-hint .brand-tag');
    if (!chip) return null;
    const style = chip.getAttribute('style') || '';
    if (style.indexOf('--danger') >= 0) return 'error';
    if (style.indexOf('--accent') >= 0) return 'info';
    return null;
  },
  // the "nothing loaded yet" call to action (R2 polish)
  startHint(){ const hint = this.q('.start-hint'); return hint ? hint.innerText : null; },

  // R3 sidebar seam: the whole lists the virtualized sidebar would draw, plus
  // the number of rows actually in the DOM (see module_struct/seam.rs).
  sidebar(){ return window.__waveInsightSidebar || null; },
  sidebarDom(){ return { modules: this.qa('.tree-row').length, signals: this.qa('.sidebar .sig-row').length }; },

  // --- canvas renderer seam (R1 draws the rows on one canvas) ----------------
  // Everything that used to read the per-row SVG nodes goes through
  // window.__waveDebug (the renderer's own counters) and a pixel hash of the
  // surface, so the assertions survive any DOM change in the wave pane.
  debug(){ return window.__waveDebug || null; },
  dbg(field){ const d = window.__waveDebug; return d ? d[field] : null; },
  totalRows(){ const d = window.__waveDebug; return d ? d.total_rows : this.qa('.wave-name-cell').length; },
  surface(){ return document.querySelector('#wave-surface') || document.querySelector('canvas.wave-surface') || document.querySelector('.wave-canvas canvas'); },
  surfaceHash(){
    const c = this.surface(); if (!c) return null;
    const ctx = c.getContext('2d'); if (!ctx) return null;
    try {
      const d = ctx.getImageData(0, 0, c.width, c.height).data;
      let h = 2166136261 >>> 0;
      for (let i = 0; i < d.length; i += 4) {
        h ^= (d[i] | (d[i+1] << 8) | (d[i+2] << 16) | (d[i+3] << 24)) >>> 0;
        h = Math.imul(h, 16777619) >>> 0;
      }
      return h;
    } catch (error) { return 'no-pixels:' + String(error); }
  },
  // Ink in a horizontal band given in css pixels from the top of the surface.
  surfaceInk(y0, y1){
    const c = this.surface(); if (!c) return -1;
    const ctx = c.getContext('2d'); if (!ctx) return -1;
    const scale = c.height / (c.clientHeight || c.height || 1);
    const top = Math.max(0, Math.round(y0 * scale));
    const bottom = Math.min(c.height, Math.round(y1 * scale));
    try {
      const d = ctx.getImageData(0, top, c.width, Math.max(1, bottom - top)).data;
      let ink = 0;
      for (let i = 0; i < d.length; i += 4) { if (d[i + 3] > 8) ink += 1; }
      return ink;
    } catch (error) { return -1; }
  },
  watchMutations(sel){
    const target = document.querySelector(sel);
    if (!target) return false;
    this._mut = [];
    this._obs = new MutationObserver((records) => {
      for (const record of records) {
        this._mut.push({ type: record.type, attr: record.attributeName || null });
      }
    });
    this._obs.observe(target, { attributes: true, childList: true, subtree: true });
    return true;
  },
  takeMutations(){ if (this._obs) this._obs.takeRecords().forEach((r) => this._mut.push({ type: r.type, attr: r.attributeName || null })); return this._mut || []; },
  stopMutations(){ const records = this.takeMutations(); if (this._obs) this._obs.disconnect(); this._obs = null; return records; },

  state(){
    // the load/drive view is a tab of the wave frame now, so it is display:none
    // while the waveform is shown; read it with textContent (innerText is empty
    // for a hidden element).
    const textOf = (sel) => { const e = this.q(sel); return e ? (e.textContent || '').replace(/\\s+/g, ' ').trim() : null; };
    // R3 virtualizes both sidebar lists: the DOM only holds the rows inside the
    // scroll window, so the logical lists come from the seam the sidebar
    // publishes (window.__waveInsightSidebar). Falls back to the DOM when no
    // seam exists yet (first paint) so an assertion never reads a half state.
    const seam = (window.__waveInsightSidebar && Array.isArray(window.__waveInsightSidebar.modules) && Array.isArray(window.__waveInsightSidebar.signals)) ? window.__waveInsightSidebar : null;
    const body = textOf('.inspector-panel .panel-body');
    return {
      tabs: this.texts('.tab .tab-name'),
      activeTab: this.text('.tab.active .tab-name'),
      modules: seam ? seam.modules.map((name) => String(name).trim()) : this.texts('.tree-row .tree-name').map(s=>s.trim()),
      sidebarSignals: seam ? seam.signals.slice() : this.texts('.sidebar .sig-row .sig-name'),
      waveNames: this.texts('.wave-name-text'),
      status: this.text('.topbar-hint .brand-tag'),
      inspectorHead: textOf('.inspector-panel .panel-head'),
      inspectorBody: body ? body.slice(0, 260) : null,
      dockTab: (this.qa('.dock-tab.active')[0] || {}).innerText || null,
      modal: this.text('.modal-title'),
      lsp: this.lsp(),
      editor: this.editor(),
    };
  },

  // --- Wave 18 seams (task-1/2/3) -------------------------------------------
  // The label cell is name -> two hover-only action icons -> value column(s):
  // a cursor value column is the last element and flush to the waveform, and
  // before a cursor no value element exists at all (--wave-value-ch: 0).
  waveValueLayout(){
    const rect = (e) => { if (!e) return null; const r = e.getBoundingClientRect(); return { left: Math.round(r.left), right: Math.round(r.right), w: Math.round(r.width) }; };
    const pane = document.querySelector("#wave-pane");
    return {
      ch: pane ? getComputedStyle(pane).getPropertyValue("--wave-value-ch").trim() : null,
      canvas: this.rect("#wave-surface"),
      cells: Array.prototype.map.call(document.querySelectorAll(".wave-name-cell"), (cell) => ({
        name: rect(cell.querySelector(".wave-name-text")),
        mores: Array.prototype.map.call(cell.querySelectorAll(".wave-more"), rect),
        values: Array.prototype.map.call(cell.querySelectorAll(".wave-value"), (v) => ({ text: (v.innerText || "").trim(), rect: rect(v) })),
      })),
    };
  },
  // __waveDebug label seam (task-1): the band labels the last paint drew and
  // how many were ellipsized/dropped (labels is capped at 256 texts,
  // labels_drawn is the true count).
  bandLabels(){
    const d = window.__waveDebug || {};
    return { labels: (d.labels || []).slice(), drawn: d.labels_drawn, truncated: d.labels_truncated, hidden: d.labels_hidden, bands: d.bands };
  },
  // The split pane last frame, parsed out of its data-frame attribute (it is
  // written in view() and the paint fills it in rendered(); flush it first).
  splitPaneFrame(){
    const pane = document.querySelector(".split-pane");
    const raw = pane ? (pane.getAttribute("data-frame") || "") : "";
    const view = /view=(-?[0-9.]+):([0-9.]+)/.exec(raw);
    const scroll = /scroll=(-?[0-9.]+)/.exec(raw);
    return { raw: raw, start: view ? parseFloat(view[1]) : null, size: view ? parseFloat(view[2]) : null, scroll: scroll ? parseFloat(scroll[1]) : null };
  },
  // One dump snapshot for the reload scenario: open tabs, rows, cursor values,
  // canvas hash and the status line.
  dumpSnapshot(){
    return {
      tabs: Array.prototype.map.call(document.querySelectorAll(".dump-tab .dump-tab-name"), (b) => (b.innerText || "").trim()),
      rows: this.texts(".wave-name-text"),
      values: this.texts(".wave-name-cell .wave-value"),
      hash: this.surfaceHash(),
      status: this.text(".topbar-hint .brand-tag"),
      cursor1: (window.__waveDebug || {}).cursor1,
    };
  },
  // 1-bit alignment probe (task-1): the cursor column is the column painted in
  // the cursor colour, the edge ink is the blue coverage of the middle band of
  // the first row (only a vertical edge has ink there; the high/low levels sit
  // at ROW_PAD=6 and ROW_HEIGHT-ROW_PAD=24 css px, outside 9..21).
  alignProbe(){
    const d = window.__waveDebug || {};
    const c = this.surface();
    if (!c) return null;
    const img = c.getContext("2d").getImageData(0, 0, c.width, c.height);
    const W = c.width, H = c.height;
    const px = (x, y) => { const i = ((y * W) + x) * 4; return [img.data[i], img.data[i + 1], img.data[i + 2], img.data[i + 3]]; };
    const bg = [13, 18, 24], wave = [76, 194, 255], yellow = [255, 209, 102];
    const cov = (p) => {
      const dv0 = wave[0] - bg[0], dv1 = wave[1] - bg[1], dv2 = wave[2] - bg[2];
      const v0 = p[0] - bg[0], v1 = p[1] - bg[1], v2 = p[2] - bg[2];
      const k = (v0 * dv0 + v1 * dv1 + v2 * dv2) / (dv0 * dv0 + dv1 * dv1 + dv2 * dv2);
      const res = Math.hypot(p[0] - (bg[0] + k * dv0), p[1] - (bg[1] + k * dv1), p[2] - (bg[2] + k * dv2));
      return res < 40 ? Math.max(0, Math.min(1, k)) : 0;
    };
    const cols = [];
    for (let x = 0; x < W; x++) {
      let n = 0;
      for (let y = 0; y < H; y++) { const p = px(x, y); if (Math.hypot(p[0] - yellow[0], p[1] - yellow[1], p[2] - yellow[2]) < 60) n++; }
      if (n > H / 2) cols.push(x);
    }
    const xc = cols.length ? cols[0] : -1;
    const masses = [];
    if (xc >= 4) {
      for (let dx = -4; dx <= 4; dx++) {
        let m = 0;
        for (let y = 9; y < 21; y++) m += cov(px(xc + dx, y));
        masses.push([dx, +m.toFixed(2)]);
      }
    }
    const xraw = (d.cursor1 - d.x_axis) * d.size;
    return {
      cursor1: d.cursor1, x_axis: d.x_axis, size: d.size,
      dpr: c.width / c.getBoundingClientRect().width,
      xraw: +xraw.toFixed(3), xfrac: +(xraw - Math.floor(xraw)).toFixed(3),
      crisp: +(Math.round(xraw) + 0.5).toFixed(2),
      cursorCol: xc, cursorCols: cols.length, masses: masses,
    };
  },
};
'ready';
`;

let results = [];
// R1 replaced the per-row SVG with one canvas: assertions read the renderer
// through `window.__waveDebug` and a pixel hash of the surface, never through
// `.wave-row` nodes.
//
// R2's layout rules are asserted *before* the code exists so they are ready
// when step 1..4 land: they report PENDING, and `QA_LAYOUT=1` (or
// `run-qa.ps1 -Layout`) turns them into hard requirements.
let canvasRenderer = false;
const LAYOUT_REQUIRED = process.env.QA_LAYOUT === '1';

function record(name, ok, detail, extra) {
  results.push(Object.assign({ name, ok, detail }, extra || {}));
}
function check(name, ok, detail) {
  record(name, ok, detail);
  console.log((ok ? 'PASS ' : 'FAIL ') + name + (detail === undefined ? '' : ' :: ' + JSON.stringify(detail).slice(0, 260)));
}
// The layout rules that the design doc puts in step 4 (inspector dock + the
// wave-splitter/ratio clamps). While `-Layout` is the gate for the steps before
// that, these are reported as DEFERRED, never as regressions; anything else that
// goes red under `-Layout` is a real regression.
const STEP4_RULES = [
  'layout: the wave pane keeps four rows',
  'layout: the editor never goes below 120 px of content',
  'layout: the editor ratio is clamped to 75 %',
  'layout: the inspector is hidden with no design and no selection',
];

// A rule from the R2 design; `pending` while unimplemented, a hard requirement
// under `-Layout`, and DEFERRED for the step-4 rules.
function pending(name, ok, detail) {
  if (ok) { record(name, true, detail); console.log('PASS ' + name + (detail === undefined ? '' : ' :: ' + JSON.stringify(detail).slice(0, 200))); return; }
  if (LAYOUT_REQUIRED) {
    if (STEP4_RULES.includes(name)) {
      record(name, true, detail, { deferred: true });
      console.log('DEFERRED ' + name + ' (R2 step 4, expected until then)');
      return;
    }
    check(name, false, detail);
    return;
  }
  record(name, true, detail, { pending: true });
  console.log('PENDING ' + name + (detail === undefined ? '' : ' :: ' + JSON.stringify(detail).slice(0, 200)));
}
// An assertion that the canvas rewrite made unobservable; the behaviour is now
// covered elsewhere (spelled out in `reason`).
function retired(name, reason) {
  record(name, true, reason, { retired: true });
  console.log('RETIRED ' + name + ' (' + reason + ')');
}
function note(label, value) { console.log('     ' + label + ': ' + JSON.stringify(value).slice(0, 400)); }

async function attach() {
  for (let i = 0; i < 40; i++) {
    try {
      const targets = await json('http://127.0.0.1:' + CDP_PORT + '/json/list');
      const page = targets.find((t) => t.type === 'page');
      if (page) return Cdp.connect(page.webSocketDebuggerUrl);
    } catch {}
    await sleep(500);
  }
  throw new Error('no chrome page');
}

async function waitFor(cdp, expr, timeout = 20000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    try { const v = await cdp.eval(expr); if (v) return v; } catch {}
    await sleep(250);
  }
  return null;
}

async function boot(cdp, waitEditor = true) {
  cdp.events.length = 0;
  // The app persists the workspace (open sources, wave rows, signal settings) in
  // localStorage, and every scenario reuses the same browser profile. Without
  // this clear a scenario inherits the previous one's state and the matrix turns
  // order-dependent (formats leaves pc in ASCII and busExpand then reads a
  // hex-looking value; analog leaves a row analog and wave1/wave2 see it).
  // The first call runs on about:blank, where storage throws, hence the
  // try/catch; a failed clear only means there was nothing to clear.
  try { await cdp.eval('window.localStorage.clear()'); } catch (error) { /* about:blank has no storage origin yet */ }
  await cdp.send('Page.navigate', { url: APP_URL });
  // the wasm bundle is large and monaco is now a local asset; give the page
  // enough time on a cold cache
  const started = await waitFor(cdp, `!!window.__waveInsightLsp && !!document.querySelector('.app')`, 60000);
  await waitFor(cdp, `!!document.querySelector('input[type=file]')`, 15000);
  await cdp.eval(PAGE_HELPERS);
  if (waitEditor) {
    await waitFor(cdp, `!!(window.monaco && monaco.editor && monaco.editor.getEditors().length)`, 40000);
  }
  await sleep(600);
  return started;
}

async function exceptions(cdp) {
  return cdp.events
    .filter((e) => e.method === 'Runtime.exceptionThrown')
    .map((e) => {
      const details = e.params.exceptionDetails;
      const stack = (details.stackTrace && details.stackTrace.callFrames ? details.stackTrace.callFrames : [])
        .slice(0, 3)
        .map((f) => (f.functionName || '?') + '@' + (f.url || '') + ':' + f.lineNumber)
        .join(' <- ');
      return (details.exception?.description || details.text || '').split('\n')[0] + (stack ? ' || ' + stack : '');
    });
}
async function errorLogs(cdp) {
  return cdp.events
    .filter((e) => e.method === 'Log.entryAdded' && e.params.entry.level === 'error')
    .map((e) => e.params.entry.text);
}
async function state(cdp) { return cdp.eval('window.__qa.state()'); }
async function probeRenderer(cdp) {
  // R1: one <canvas> plus a debug hook. If either is missing the renderer
  // assertions below will tell us precisely which one.
  const hasCanvas = await cdp.eval("!!document.querySelector('#wave-surface') || !!document.querySelector('.wave-canvas canvas')");
  const hasDebug = await cdp.eval("!!window.__waveDebug");
  canvasRenderer = hasCanvas;
  return JSON.stringify({ canvas: hasCanvas, debugHook: hasDebug });
}
async function waitStatus(cdp, needle, timeout = 15000) {
  const expr = `(window.__qa.text('.topbar-hint .brand-tag')||'').includes(${JSON.stringify(needle)})`;
  return waitFor(cdp, expr, timeout);
}
async function waitTabs(cdp, count, timeout = 20000) {
  return waitFor(cdp, `window.__qa.qa('.tab').length >= ${count}`, timeout);
}

// Click a module row in the sidebar tree (this selects it and expands it).
async function selectModule(cdp, name) {
  const ok = await cdp.eval(`(function(){
    const row = Array.from(document.querySelectorAll('.tree-row')).find(r => r.innerText.trim() === ${JSON.stringify(name)});
    if (!row) return false;
    row.click();
    return true;
  })()`);
  await sleep(500);
  return ok;
}

// Add a signal from the sidebar signal list, by its local name.
async function addSignal(cdp, name) {
  const result = await cdp.eval(`window.__qa.clickSignal(${JSON.stringify(name)})`);
  await sleep(900);
  return result === 'ok';
}

// Wave 18 (task-2, Verdi wheel mapping): a plain wheel over the wave canvas
// PANS the time axis (wheel down = forward in time), Ctrl+wheel zooms at the
// pointer and Shift+wheel scrolls the row list (over the name column the wheel
// keeps scrolling the rows, as any list). This dispatcher is the single place
// that encodes the CDP modifier bitmask (Alt=1, Ctrl=2, Meta=4, Shift=8), so
// every scenario exercises the same gesture the app sees from a real wheel.
// The keyDown/keyUp pairs are kept too: the app tracks key state for the
// historical self.key() path, and the wheel event carries the modifiers itself
// (e.ctrl_key() / e.shift_key()), which covers a synthetic event with no
// preceding key event.
async function wheel(cdp, x, y, deltaY, options = {}) {
  const modifiers = (options.alt ? 1 : 0) | (options.ctrl ? 2 : 0) | (options.meta ? 4 : 0) | (options.shift ? 8 : 0);
  if (options.ctrl) await cdp.key("keyDown", "Control", "ControlLeft", 17);
  if (options.shift) await cdp.key("keyDown", "Shift", "ShiftLeft", 16);
  await cdp.send("Input.dispatchMouseEvent", {
    type: "mouseWheel", x, y, deltaX: options.deltaX || 0, deltaY, modifiers,
  });
  if (options.ctrl) await cdp.key("keyUp", "Control", "ControlLeft", 17);
  if (options.shift) await cdp.key("keyUp", "Shift", "ShiftLeft", 16);
}

// The wave pane view + row scroll, from the renderer seam and the DOM.
// `scroll_top` is the renderer copy of the list offset (gated on its scroll
// handler); `dom_scroll` is the list own scrollTop. Both must move for the rows
// gesture and both must stay put for a pan.
async function viewState(cdp) {
  return cdp.eval('(function(){ const d = window.__waveDebug || {}; const body = document.querySelector("#wave-body"); return { x_axis: d.x_axis, size: d.size, scroll_top: d.scroll_top, dom_scroll: body ? Math.round(body.scrollTop) : null }; })()');
}

// Flush the split pane's data-frame with two window resizes (the +1 px pass
// forces a render, the restore forces the second one), then leave the window at
// the caller width/height. The attribute is written in view() while the paint
// happens in rendered(), so it is exactly one paint behind; the frame fields are
// parsed by __qa.splitPaneFrame().
async function flushSplitPane(cdp, width, height) {
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: width + 1, height, deviceScaleFactor: 1, mobile: false });
  await sleep(550);
  await cdp.send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
  await sleep(850);
}

// ---------------------------------------------------------------------------
// scenarios
// ---------------------------------------------------------------------------
const scenarios = {};

scenarios.empty = async (cdp) => {
  await boot(cdp);
  const s = await state(cdp);
  note('state', s);
  check('empty: app renders', s !== null);
  check('empty: no tabs', s.tabs.length === 0, s.tabs);
  check('empty: no wave rows', s.waveNames.length === 0, s.waveNames);
  check('empty: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  check('empty: lsp initialised', s.lsp && s.lsp.initialized === true, s.lsp);
  // R2 polish: with nothing loaded the app must say what to do instead of
  // showing an empty tree and an empty wave pane
  const hint = await cdp.eval('window.__qa.startHint()');
  note('empty state hint', (hint || '').slice(0, 200));
  check('empty: the empty state says what to do', !!hint && /\.vcd/.test(hint) && /\.sv/.test(hint), (hint || '').slice(0, 120));
  await cdp.shot('qa-empty.png');
};

scenarios.vcd = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  const ok = await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  let s = await state(cdp);
  note('state', s);
  check('vcd: loaded', !!ok, s.status);
  check('vcd: module tree has top', s.modules.includes('top'), s.modules);
  check('vcd: top signals listed', s.sidebarSignals.includes('clk') && s.sidebarSignals.length >= 7, s.sidebarSignals.length);
  // add a 1 bit and a bus signal
  await cdp.eval(`window.__qa.clickSignal('clk')`);
  await sleep(500);
  await cdp.eval(`window.__qa.clickSignal('pc')`);
  await sleep(800);
  s = await state(cdp);
  note('after add', s.waveNames);
  check('vcd: two rows added', s.waveNames.length === 2, s.waveNames);
  const points = await cdp.eval(`window.__qa.qa('.wave-row polyline').map(p => (p.getAttribute('points')||'').length)`);
  note('legacy polyline lengths (empty on the canvas)', points);
  const dbg = await cdp.eval('window.__qa.debug()');
  note('debug hook', dbg);
  const ink = await cdp.eval('window.__qa.surfaceInk(0, 120)');
  check('vcd: the rows are drawn on the canvas', !!dbg && dbg.rows >= 2 && dbg.visible >= 2, { rows: dbg && dbg.rows, visible: dbg && dbg.visible });
  check('vcd: the surface has ink', ink > 0, ink);
  check('vcd: bus changes are drawn', !!dbg && dbg.changes > 0, dbg && dbg.changes);
  check('vcd: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-vcd.png');
};

scenarios.fst = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'io_mode_wave.fst')]);
  const ok = await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  let s = await state(cdp);
  note('state', s);
  check('fst: loaded', !!ok, s.status);
  check('fst: module tree not empty', s.modules.length > 0, s.modules);
  const count = await cdp.eval(`window.__qa.qa('.sidebar .sig-row .sig-name').length`);
  check('fst: signals listed', count > 0, count);
  await cdp.eval(`window.__qa.click('.sidebar .sig-row', 0)`);
  await sleep(900);
  s = await state(cdp);
  const fstDebug = await cdp.eval('window.__qa.debug()');
  const fstInk = await cdp.eval('window.__qa.surfaceInk(0, 60)');
  note('after add', { names: s.waveNames, debug: fstDebug, ink: fstInk });
  check('fst: row added', s.waveNames.length === 1, s.waveNames);
  check('fst: values decoded and drawn', !!fstDebug && fstDebug.changes > 0 && fstInk > 0, { changes: fstDebug && fstDebug.changes, ink: fstInk });
  check('fst: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-fst.png');
};

scenarios.bothDumps = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd'), path.join(FIXTURES, 'io_mode_wave.fst')]);
  await sleep(6000);
  const s = await state(cdp);
  note('state', s);
  check('both: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  check('both: one tree shown', s.modules.length > 0, s.modules);
  await cdp.eval(`window.__qa.click('.sidebar .sig-row', 0)`);
  await sleep(900);
  const after = await state(cdp);
  note('after add', after.waveNames);
  check('both: can still add a row', after.waveNames.length === 1, after.waveNames);
  await cdp.shot('qa-both.png');
};

scenarios.sourcesOnly = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv')]);
  await waitTabs(cdp, 2);
  await sleep(1200);
  const s = await state(cdp);
  note('state', s);
  check('sources: two tabs', s.tabs.length === 2, s.tabs);
  check('sources: design tree', s.modules.includes('u_cpu') && s.modules.includes('u_cpu16'), s.modules);
  check('sources: top signals', s.sidebarSignals.length === 7, s.sidebarSignals);
  check('sources: lsp documents opened', s.lsp && s.lsp.documents === 2, s.lsp);
  await cdp.eval(`window.__qa.clickSignal('clk')`);
  await sleep(900);
  const after = await state(cdp);
  note('after add', after.waveNames);
  check('sources: signal added without dump', after.waveNames.length === 1, after.waveNames);
  check('sources: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

scenarios.sourcesAndDump = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  const s = await state(cdp);
  note('state', s);
  check('combo: dump + sources', s.modules.length > 0 && s.tabs.length === 2, { modules: s.modules, tabs: s.tabs });
  // expand the top scope to reach the instance signals
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(700);
  const tree = await cdp.eval(`window.__qa.texts('.tree-row .tree-name').map(s=>s.trim())`);
  note('tree after expand', tree);
  check('combo: instances visible', tree.some((t) => t.includes('u_cpu')), tree);
  await cdp.shot('qa-combo.png');
};

scenarios.clickToAdd = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(600);
  const token = await cdp.eval(`(function(){
    const spans = Array.from(document.querySelectorAll('.view-line span span'));
    const span = spans.find(s => s.textContent.trim() === 'pc_reg');
    if (!span) return null;
    const r = span.getBoundingClientRect();
    return {x: Math.round(r.left + 4), y: Math.round(r.top + r.height / 2)};
  })()`);
  note('token position', token);
  check('clickToAdd: token found in editor', !!token, token);
  if (token) {
    await cdp.clickAt(token.x, token.y);
    await sleep(900);
    let s = await state(cdp);
    note('after click', { modal: s.modal, status: s.status });
    check('clickToAdd: picker opened', (s.modal || '').includes('pc_reg'), s.modal);
    const rows = await cdp.eval(`window.__qa.texts('.modal .sig-row .sig-name')`);
    note('picker rows', rows);
    check('clickToAdd: two candidates', rows.length === 2, rows);
    await cdp.eval(`window.__qa.click('.modal .sig-row', 0)`);
    await sleep(1200);
    s = await state(cdp);
    note('after picking', { names: s.waveNames, inspector: s.inspectorBody });
    check('clickToAdd: row added', s.waveNames.length === 1 && s.waveNames[0].includes('pc_reg'), s.waveNames);
    const addedDebug = await cdp.eval('window.__qa.debug()');
    check('clickToAdd: values decoded and drawn', !!addedDebug && addedDebug.changes > 0, addedDebug && addedDebug.changes);
    check('clickToAdd: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
    await cdp.shot('qa-click-to-add.png');
  }
};

scenarios.waveJump = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(500);
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(500);
  // add an instance signal from the sidebar (u_cpu's pc_reg)
  await selectModule(cdp, 'u_cpu');
  const ok = await addSignal(cdp, 'pc_reg');
  check('waveJump: instance signal available', !!ok);
  let s = await state(cdp);
  note('after sidebar add', { names: s.waveNames, editor: s.editor, status: s.status });
  check('waveJump: add jumps to declaration (top.sv)', (s.activeTab || '') === 'cpu.sv' && s.editor && s.editor.line === 13, { tab: s.activeTab, editor: s.editor });
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'top.sv')`);
  await sleep(600);
  await cdp.eval(`window.__qa.click('.wave-name-text', 0)`);
  await sleep(900);
  s = await state(cdp);
  note('after wave name click', { tab: s.activeTab, editor: s.editor, status: s.status });
  check('waveJump: wave name opens declaration', s.activeTab === 'cpu.sv' && s.editor && s.editor.line === 13 && s.editor.column === 23, { tab: s.activeTab, editor: s.editor });
  check('waveJump: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-wave-jump.png');
};

scenarios.inspector = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(500);
  await selectModule(cdp, 'u_cpu');
  const gotSignal = await addSignal(cdp, 'pc_reg');
  check('inspector: instance signal reachable', !!gotSignal);
  await sleep(600);
  // the panel is the second tab of the wave frame
  await cdp.eval(`window.__qa.clickText('.dock-tab', 'Drivers / loads')`);
  await sleep(700);
  let s = await state(cdp);
  note('inspector', { head: s.inspectorHead, body: s.inspectorBody, tab: s.dockTab });
  check('inspector: shows the signal', (s.inspectorHead || '').includes('pc_reg'), s.inspectorHead);
  check('inspector: declaration row', (s.inspectorBody || '').includes('cpu.sv:13'), s.inspectorBody);
  // the panel text is read with textContent (it is hidden on the waveform tab),
  // so the css `text-transform: uppercase` does not apply: match case-insensitively
  check('inspector: drivers listed', /drivers \(2\)/i.test(s.inspectorBody || ''), s.inspectorBody);
  const rows = await cdp.eval(`window.__qa.qa('.inspector-panel .panel-body .sig-row[title]').map(e => e.title)`);
  note('inspector rows', rows);
  check('inspector: clickable rows', rows.length >= 4, rows.length);
  // click the last (a load) and verify the editor moved
  await cdp.eval(`(function(){ const r = window.__qa.qa('.inspector-panel .panel-body .sig-row[title]'); if (r.length) r[r.length-1].click(); })()`);
  await sleep(800);
  s = await state(cdp);
  note('after inspector click', { editor: s.editor, status: s.status });
  check('inspector: row jumps', s.editor && s.editor.line === 20, s.editor);
  // and the waveform is one click away, still on the same zoom
  await cdp.eval(`window.__qa.clickText('.dock-tab', 'Waveform')`);
  await sleep(700);
  const back = await cdp.eval(`(function(){
    const c = document.querySelector('#wave-surface');
    const r = c.getBoundingClientRect();
    return { w: Math.round(r.width), h: Math.round(r.height), bitmap: [c.width, c.height], visible: getComputedStyle(document.querySelector('#wave-pane')).display };
  })()`);
  check('inspector: the waveform comes back at full size', back.visible === 'flex' && back.w > 100 && back.h > 100
    && Math.abs(back.bitmap[1] - back.h) <= 1, back);
};

scenarios.lsp = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv')]);
  await waitTabs(cdp, 2);
  await sleep(1500);
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(800);
  const s = await state(cdp);
  note('lsp stats', s.lsp);
  check('lsp: initialised', s && s.lsp && s.lsp.initialized === true, s.lsp);
  check('lsp: no failures yet', s.lsp.failures === 0, s.lsp);
  const definition = await cdp.eval(`window.__qa.provider('definition', 'cpu.sv', 25, 13)`);
  note('definition(pc_reg usage)', definition);
  check('lsp: definition answers', typeof definition === 'string' && definition !== 'null' && !definition.startsWith('no-'), definition);
  check('lsp: definition points at the declaration', /"startLineNumber":13/.test(definition || '') && /"startColumn":23/.test(definition || ''), definition);
  const declarationDefinition = await cdp.eval(`window.__qa.provider('definition', 'cpu.sv', 13, 23)`);
  note('definition(pc_reg declaration)', declarationDefinition);
  check('lsp: definition on the declaration answers', typeof declarationDefinition === 'string' && declarationDefinition !== 'null', declarationDefinition);
  const hover = await cdp.eval(`window.__qa.provider('hover', 'cpu.sv', 25, 13)`);
  note('hover(pc_reg)', hover);
  check('lsp: hover answers', typeof hover === 'string' && hover !== 'null' && !hover.startsWith('no-'), hover);
  const references = await cdp.eval(`window.__qa.provider('references', 'cpu.sv', 13, 23)`);
  note('references(pc_reg)', references);
  check('lsp: references answer', typeof references === 'string' && references.startsWith('[') && references.length > 10, references);
  const symbols = await cdp.eval(`window.__qa.provider('symbols', 'cpu.sv', 1, 1)`);
  note('symbols(cpu.sv)', (symbols || '').slice(0, 300));
  check('lsp: document symbols answer', typeof symbols === 'string' && symbols.startsWith('[') && symbols.length > 10, (symbols || '').slice(0, 80));
  // the same hover, through monaco's own UI
  const token = await cdp.eval(`(function(){
    const spans = Array.from(document.querySelectorAll('.view-line span span'));
    const span = spans.find(s => s.textContent.trim() === 'pc_reg' && s.getBoundingClientRect().top > 0);
    if (!span) return null;
    const r = span.getBoundingClientRect();
    return {x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2)};
  })()`);
  if (token) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: token.x, y: token.y });
    await sleep(1200);
    const hoverText = await cdp.eval(`window.__qa.text('.monaco-hover-content') || window.__qa.text('.monaco-hover')`);
    note('monaco hover', (hoverText || '').slice(0, 160));
    check('lsp: monaco shows the hover card', !!hoverText && /width/i.test(hoverText), (hoverText || '').slice(0, 80));
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 5, y: 300 });
    await sleep(300);
  } else {
    check('lsp: monaco shows the hover card', false, 'no token span');
  }
  const after = await state(cdp);
  note('lsp stats after', after.lsp);
  check('lsp: no failures after queries', after.lsp.failures === 0, after.lsp);
  check('lsp: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

scenarios.zoomPan = async (cdp) => {
  await boot(cdp);
  // Wave 18 (task-2): the wheel mapping is Verdi-like now - a plain wheel PANS
  // the time axis (wheel down = forward), Ctrl+wheel zooms at the pointer and
  // Shift+wheel scrolls the row list (over the label column the wheel keeps
  // scrolling, untouched). The scenario needs a list that overflows and a view
  // that can pan (a fitted view is clamped at the end), so it loads
  // many_signals.vcd, adds 20 rows and shrinks the window.
  await cdp.upload([path.join(FIXTURES, 'many_signals.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 40000);
  await sleep(700);
  const addedRows = await cdp.eval('(function(){ let n = 0; for (let i = 0; i < 20; i++) { if (window.__qa.clickSignal("sig_" + String(i).padStart(4, "0")) === "ok") n++; } return n; })()');
  await sleep(1500);
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1100, height: 600, deviceScaleFactor: 1, mobile: false });
  await sleep(1500);
  const canvas = await cdp.eval('window.__qa.rect(".wave-canvas")');
  note('canvas', { canvas: canvas, rows: addedRows });
  check('zoom: a list big enough for the row gesture is loaded', addedRows === 20, addedRows);
  const before = await viewState(cdp);
  const axisBefore = await cdp.eval('window.__qa.axis()');
  const hashBefore = await cdp.eval('window.__qa.surfaceHash()');
  note('view before', before);
  // ctrl + wheel zoom: the scale changes at the pointer, the rows stay put
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  for (let i = 0; i < 2; i++) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 200, y: canvas.y + 20, deltaX: 0, deltaY: -120, modifiers: 2 });
    await sleep(250);
  }
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(700);
  const zoomed = await viewState(cdp);
  const axisZoomed = await cdp.eval('window.__qa.axis()');
  const hashZoomed = await cdp.eval('window.__qa.surfaceHash()');
  note('after ctrl+wheel', { view: zoomed, axis: axisZoomed });
  check('zoom: ctrl+wheel changes the axis', axisZoomed !== axisBefore, { before: axisBefore, zoomed: axisZoomed });
  check('zoom: ctrl+wheel changes the scale', zoomed.size > before.size, { before: before.size, after: zoomed.size });
  check('zoom: ctrl+wheel does not scroll the rows', zoomed.dom_scroll === before.dom_scroll && zoomed.scroll_top === before.scroll_top, { before: before.dom_scroll, after: zoomed.dom_scroll });
  check('zoom: ctrl+wheel keeps the canvas in step', hashZoomed !== hashBefore, { same: hashZoomed === hashBefore });
  // plain wheel: pan the time axis (the gesture that used to be Shift+wheel)
  await wheel(cdp, canvas.x + 200, canvas.y + 20, 120, {});
  await sleep(700);
  const panned = await viewState(cdp);
  const axisPanned = await cdp.eval('window.__qa.axis()');
  const hashPanned = await cdp.eval('window.__qa.surfaceHash()');
  note('after plain wheel', panned);
  check('pan: a plain wheel changes the axis', axisPanned !== axisZoomed, { before: axisZoomed, after: axisPanned });
  check('pan: a plain wheel pans forward in time', panned.x_axis > zoomed.x_axis, { before: +zoomed.x_axis.toFixed(3), after: +panned.x_axis.toFixed(3) });
  check('pan: the canvas repainted at the new origin', hashPanned !== hashZoomed, { same: hashPanned === hashZoomed });
  check('pan: a plain wheel keeps the scale', panned.size === zoomed.size, { before: zoomed.size, after: panned.size });
  check('pan: a plain wheel does not scroll the rows', panned.dom_scroll === zoomed.dom_scroll && panned.scroll_top === zoomed.scroll_top, { before: zoomed.dom_scroll, after: panned.dom_scroll });
  // shift + wheel: scroll the row list; the view must not move (Shift+wheel was
  // the pan gesture before Wave 18, so this is the assertion that flipped)
  await cdp.key('keyDown', 'Shift', 'ShiftLeft', 16);
  for (let i = 0; i < 3; i++) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 200, y: canvas.y + 20, deltaX: 0, deltaY: 240, modifiers: 8 });
    await sleep(220);
  }
  await cdp.key('keyUp', 'Shift', 'ShiftLeft', 16);
  await sleep(800);
  const scrolled = await viewState(cdp);
  const axisScrolled = await cdp.eval('window.__qa.axis()');
  note('after shift+wheel', scrolled);
  check('pan: shift+wheel scrolls the row list', scrolled.dom_scroll > panned.dom_scroll, { before: panned.dom_scroll, after: scrolled.dom_scroll });
  check('pan: shift+wheel leaves the axis where it was', axisScrolled === axisPanned && scrolled.x_axis === panned.x_axis && scrolled.size === panned.size, { axis: [axisPanned, axisScrolled], view: [panned.x_axis, scrolled.x_axis] });
  check('pan: shift+wheel keeps the renderer scroll in step with the list', Math.abs(scrolled.scroll_top - scrolled.dom_scroll) <= 1, { renderer: scrolled.scroll_top, dom: scrolled.dom_scroll });
  // drag select zoom
  await cdp.mouse('mousePressed', canvas.x + 100, canvas.y + 15, { buttons: 1 });
  await cdp.mouse('mouseMoved', canvas.x + 260, canvas.y + 15, { buttons: 1 });
  await cdp.mouse('mouseReleased', canvas.x + 260, canvas.y + 15, { buttons: 0 });
  await sleep(800);
  const dragged = await cdp.eval('window.__qa.axis()');
  note('axis after drag', dragged);
  check('zoom: drag-select changes the axis', dragged !== axisPanned, { before: axisPanned, after: dragged });
  // z and f keys
  await cdp.press('z', 'KeyZ', 90);
  await sleep(600);
  const zoomedOut = await cdp.eval('window.__qa.axis()');
  check('zoom: z zooms out', zoomedOut !== dragged, { dragged, zoomedOut });
  await cdp.press('f', 'KeyF', 70);
  await sleep(600);
  const fitted = await cdp.eval('window.__qa.axis()');
  note('axis fitted', fitted);
  check('zoom: f fits the trace', fitted !== zoomedOut, { zoomedOut, fitted });
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await sleep(900);
  check('zoom: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-zoom.png');
};

scenarios.cursors = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  await cdp.eval(`window.__qa.clickSignal('pc')`);
  await sleep(800);
  const canvas = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  const hashBeforeCursor = await cdp.eval('window.__qa.surfaceHash()');
  await cdp.clickAt(canvas.x + 120, canvas.y + 15);
  await sleep(700);
  let readout = await cdp.eval(`window.__qa.text('.timebar-readout')`);
  check('cursor1: readout shows a cursor', /\u0394/.test(readout || ''), readout);
  const cursor1 = await cdp.eval('window.__qa.dbg("cursor1")');
  const hashWithCursor1 = await cdp.eval('window.__qa.surfaceHash()');
  check('cursor1: the renderer records the cursor', typeof cursor1 === 'number', cursor1);
  check('cursor1: the canvas repainted for it', hashWithCursor1 !== hashBeforeCursor, { same: hashWithCursor1 === hashBeforeCursor });
  retired('cursor1: value shown next to the cursor', 'canvas label; covered by the formats hash checks and the value formatter unit tests');
  await cdp.clickAt(canvas.x + 320, canvas.y + 15, 'right');
  await sleep(700);
  readout = await cdp.eval(`window.__qa.text('.timebar-readout')`);
  note('readout with two cursors', readout);
  check('cursor2: delta shown', /\u0394\s*-?\d+/.test(readout || ''), readout);
  const cursor2 = await cdp.eval('window.__qa.dbg("cursor2")');
  const hashWithCursor2 = await cdp.eval('window.__qa.surfaceHash()');
  check('cursor2: the renderer records the cursor', typeof cursor2 === 'number', cursor2);
  check('cursor2: the canvas repainted for it', hashWithCursor2 !== hashWithCursor1, { same: hashWithCursor2 === hashWithCursor1 });
  const prevented = await cdp.eval(`(function(){
    const element = document.querySelector('#wave-surface') || document.querySelector('.wave-canvas');
    const event = new MouseEvent('contextmenu', {bubbles: true, cancelable: true});
    element.dispatchEvent(event);
    return event.defaultPrevented;
  })()`);
  check('cursor2: right click does not open the browser menu', prevented === true, prevented);
  check('cursors: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-cursors.png');
};

scenarios.formats = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  await cdp.eval(`window.__qa.clickSignal('pc')`);
  await sleep(900);
  const canvas = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  await cdp.clickAt(canvas.x + 420, canvas.y + 15);
  await sleep(600);
  // zoom in until a bus value label fits inside its band, so the format change
  // is visible both in the value column and on the surface
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  for (let i = 0; i < 9; i++) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 200, y: canvas.y + 20, deltaX: 0, deltaY: -120, modifiers: 2 });
    await sleep(250);
  }
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(700);
  await cdp.eval(`window.__qa.click('.wave-more', 1)`);
  await sleep(500);
  let s = await state(cdp);
  check('formats: settings modal opens', !!s.modal, s.modal);
  // Hex is the default, and some formats legitimately paint the same string
  // (`Signed` of a positive value equals `Dec`), so this records a hash per
  // format and compares only the representations that must differ.
  const seen = {};
  const values = {};
  for (const label of ['Dec', 'Hex', 'Signed', 'Oct', 'Bin', 'ASCII']) {
    const clicked = await cdp.eval(`window.__qa.clickText('.modal .btn', '${label}')`);
    await sleep(500);
    const hash = await cdp.eval('window.__qa.surfaceHash()');
    const ink = await cdp.eval('window.__qa.surfaceInk(0, 400)');
    // the value column is the primary readout now that the on-canvas one is
    // gone (task-12); the band labels are the secondary one
    const value = await cdp.eval(`window.__qa.texts('.wave-name-cell .wave-value')[0]`);
    seen[label] = hash;
    values[label] = value;
    note('format ' + label, { hash, ink, value });
    check('formats: ' + label + ' is applied and painted', clicked === 'ok' && hash !== null && ink > 0 && !!value, { hash, ink, value });
    // reopen the modal: choosing a format closes it
    if (label !== 'ASCII') {
      await cdp.eval(`window.__qa.click('.wave-more', 1)`);
      await sleep(300);
    }
  }
  check('formats: hex and dec paint differently', seen['Hex'] !== seen['Dec'], { hex: seen['Hex'], dec: seen['Dec'] });
  check('formats: hex and bin paint differently', seen['Hex'] !== seen['Bin'], { hex: seen['Hex'], bin: seen['Bin'] });
  check('formats: the value column follows the format',
    values['Hex'] !== values['Dec'] && values['Hex'] !== values['Bin'],
    { hex: values['Hex'], dec: values['Dec'], bin: values['Bin'] });
  const distinctValues = new Set(Object.values(values).map(String));
  check('formats: the six formats produce at least four distinct values', distinctValues.size >= 4, Array.from(distinctValues));
  // Wave 18 (task-1): the wave view prints bare digits - the Verilog radix
  // prefix belongs to the data form (editor inlay, export, find), not to the
  // value column. pc is 0x49 = 73 at this cursor, so the six formats must print
  // exactly these strings.
  const expected = { Dec: '73', Hex: '49', Signed: '73', Oct: '111', Bin: '01001001', ASCII: 'I' };
  note('formats bare values', values);
  check('formats: the value column prints the bare formatted digits',
    Object.keys(expected).every((label) => values[label] === expected[label]), { values: values, expected: expected });
  check('formats: no value carries a Verilog radix prefix',
    Object.values(values).every((value) => typeof value === 'string' && value.length > 0 && value.charCodeAt(0) !== 39), values);
  check('formats: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-formats.png');
};

scenarios.colourAndDelete = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  await cdp.eval(`window.__qa.clickSignal('pc')`);
  await sleep(900);
  const before = await cdp.eval('window.__qa.surfaceHash()');
  await cdp.eval(`window.__qa.click('.wave-more', 1)`);
  await sleep(500);
  await cdp.eval(`window.__qa.click('.modal .swatch', 0)`);
  await sleep(700);
  const after = await cdp.eval('window.__qa.surfaceHash()');
  note('surface hashes', { before, after });
  check('colour: swatch recolours the row', before !== null && after !== before, { before, after });
  await cdp.eval(`window.__qa.click('.wave-more', 1)`);
  await sleep(500);
  await cdp.eval(`window.__qa.clickText('.modal .btn', 'Remove signal')`);
  await sleep(900);
  let s = await state(cdp);
  note('after delete', { names: s.waveNames, inspector: s.inspectorHead });
  check('delete: row removed', s.waveNames.length === 0, s.waveNames);
  check('delete: inspector cleared', !(s.inspectorHead || '').toUpperCase().includes('PC'), s.inspectorHead);
  // adding another row forces a re-render; the deleted one must stay gone
  await addSignal(cdp, 'clk');
  s = await state(cdp);
  note('after adding another row', s.waveNames);
  check('delete: row stays deleted across re-renders', s.waveNames.length === 1 && s.waveNames[0].includes('clk'), s.waveNames);
  // and the very same signal can be added again
  await addSignal(cdp, 'pc');
  s = await state(cdp);
  note('after re-adding', s.waveNames);
  check('delete: the same signal can be re-added', s.waveNames.length === 2 && s.waveNames.some((n) => n.includes('pc')), s.waveNames);
  check('delete: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

scenarios.layout = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  await cdp.eval(`window.__qa.clickSignal('clk')`);
  await sleep(800);
  const canvasBefore = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  const sidebarBefore = await cdp.eval(`window.__qa.rect('.sidebar')`);
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1100, height: 760, deviceScaleFactor: 1, mobile: false });
  await sleep(1500);
  const canvasAfter = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  note('canvas resize', { canvasBefore, canvasAfter });
  check('layout: window resize reflows the canvas', canvasAfter.w !== canvasBefore.w, { before: canvasBefore.w, after: canvasAfter.w });
  // drag the splitter
  const splitter = await cdp.eval(`window.__qa.rect('.splitter')`);
  note('splitter', splitter);
  await cdp.mouse('mousePressed', splitter.x + 2, splitter.y + 200, { buttons: 1 });
  await cdp.mouse('mouseMoved', splitter.x + 142, splitter.y + 200, { buttons: 1 });
  await cdp.mouse('mouseReleased', splitter.x + 142, splitter.y + 200, { buttons: 0 });
  await sleep(900);
  const sidebarAfter = await cdp.eval(`window.__qa.rect('.sidebar')`);
  const canvasFinal = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  note('after splitter drag', { sidebarAfter, canvasFinal });
  check('layout: splitter resizes the sidebar', sidebarAfter.w > sidebarBefore.w + 40, { before: sidebarBefore.w, after: sidebarAfter.w });
  check('layout: canvas follows the splitter', canvasFinal.w < canvasAfter.w, { before: canvasAfter.w, after: canvasFinal.w });
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  check('layout: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-layout.png');
};

scenarios.drop = async (cdp) => {
  await boot(cdp);
  const dropped = await cdp.eval(`(async function(){
    const response = await fetch('/fixtures/wave.vcd');
    const text = await response.text();
    const dt = new DataTransfer();
    dt.items.add(new File([text], 'dropped.vcd', {type: 'text/plain'}));
    const zone = document.querySelector('.file-drop');
    zone.dispatchEvent(new DragEvent('dragover', {bubbles: true, cancelable: true, dataTransfer: dt}));
    zone.dispatchEvent(new DragEvent('drop', {bubbles: true, cancelable: true, dataTransfer: dt}));
    return 'dropped';
  })()`);
  note('drop', dropped);
  const ok = await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  const s = await state(cdp);
  note('state', s);
  check('drop: drop zone loads a dump', !!ok, s.status);
  check('drop: module tree present', s.modules.length > 0, s.modules);
  check('drop: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

scenarios.badInputs = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'bad.vcd')]);
  await sleep(2500);
  let s = await state(cdp);
  let tone = await cdp.eval('window.__qa.statusTone()');
  note('after bad.vcd', { status: s.status, tone });
  check('bad: corrupt dump reports an error', /unknown waveform|not a|expected/i.test(s.status || ''), s.status);
  check('bad: corrupt dump says what to do', /check that|intact|VCD\/FST\/GHW/i.test(s.status || ''), s.status);
  check('bad: a failed load is coloured as an error', tone === 'error', tone);
  check('bad: corrupt dump does not crash', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  // an unsupported extension is refused instead of being parsed as Verilog
  await cdp.upload([path.join(FIXTURES, 'wave-as-txt.txt')]);
  await sleep(3000);
  s = await state(cdp);
  tone = await cdp.eval('window.__qa.statusTone()');
  note('after wrong extension', { status: s.status, tabs: s.tabs, tone });
  check('bad: wrong extension does not crash', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  check('bad: wrong extension is refused with advice', /unsupported file type/i.test(s.status || '') && /\.vcd/.test(s.status || ''), s.status);
  check('bad: the refused file is not opened as a tab', !s.tabs.some((t) => t.includes('wave-as-txt')), s.tabs);
  // malformed source
  await cdp.upload([path.join(FIXTURES, 'malformed.sv')]);
  await sleep(2500);
  s = await state(cdp);
  note('after malformed.sv', s.status);
  check('bad: malformed source does not crash', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-bad.png');
};

scenarios.tabClose = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  let s = await state(cdp);
  note('before close', { tabs: s.tabs, lsp: s.lsp });
  await cdp.eval(`window.__qa.click('.tab .tab-close', 1)`);
  await sleep(1200);
  s = await state(cdp);
  note('after close', { tabs: s.tabs, active: s.activeTab, status: s.status, lsp: s.lsp, editor: s.editor });
  check('close: tab removed', s.tabs.length === 1, s.tabs);
  check('close: editor shows the remaining file', (s.activeTab || '') === 'top.sv', s.activeTab);
  check('close: lsp document closed', s.lsp && s.lsp.documents === 1, s.lsp);
  // the signal from the closed file must still resolve (design keeps it)
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(500);
  await selectModule(cdp, 'u_cpu');
  const added = await addSignal(cdp, 'pc_reg');
  check('close: closed source still addable', !!added);
  s = await state(cdp);
  note('after add from closed source', { names: s.waveNames, editor: s.editor, status: s.status });
  check('close: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

// ---------------------------------------------------------------------------
// deeper / second-round scenarios
// ---------------------------------------------------------------------------

// x/z values, cursor stability across zoom, reverse drag zoom, row selection
// vs. row reordering, the settings button, the jump button.
scenarios.deepWave = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  await addSignal(cdp, 'data_out');       // zzzz at time 0
  await addSignal(cdp, 'pc');             // xxxx at time 0
  let s = await state(cdp);
  note('rows', s.waveNames);
  check('deep: two rows', s.waveNames.length === 2, s.waveNames);

  const canvas = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  // cursor at the very left: time 0, where both signals are unknown
  await cdp.clickAt(canvas.x + 3, canvas.y + 15);
  await sleep(700);
  const unknownDebug = await cdp.eval('window.__qa.debug()');
  const unknownInk = await cdp.eval('window.__qa.surfaceInk(0, 60)');
  note('z/x row', { debug: unknownDebug, ink: unknownInk });
  check('deep: the z/x rows are drawn', !!unknownDebug && unknownDebug.changes > 0 && unknownInk > 0, { changes: unknownDebug && unknownDebug.changes, ink: unknownInk });
  retired('deep: unknown values are shown (z/x)', 'canvas labels are not DOM readable; z/x formatting is covered by the value formatter unit tests and the format-hash assertions');

  // cursor time must not change when the trace is zoomed.
  // A cursor on the very first time step sits at x = 0 forever (0 - x_axis is
  // negative after a pan, zero at the left edge), so move it into the trace
  // first: that is what the "follows the zoom" assertion is about.
  await cdp.clickAt(canvas.x + 300, canvas.y + 15);
  await sleep(600);
  let cursorTime = await cdp.eval(`(window.__qa.q('.axis-text.cursor-a')||{}).textContent`);
  let cursorX = await cdp.eval(`(window.__qa.q('.axis-text.cursor-a')||{}).getAttribute && window.__qa.q('.axis-text.cursor-a').getAttribute('x')`);
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 300, y: canvas.y + 20, deltaX: 0, deltaY: -120, modifiers: 2 });
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(800);
  const cursorTimeAfter = await cdp.eval(`(window.__qa.q('.axis-text.cursor-a')||{}).textContent`);
  const cursorXAfter = await cdp.eval(`(window.__qa.q('.axis-text.cursor-a')||{}).getAttribute && window.__qa.q('.axis-text.cursor-a').getAttribute('x')`);
  note('cursor under zoom', { cursorTime, cursorX, cursorTimeAfter, cursorXAfter });
  check('deep: the cursor keeps its time when zooming', cursorTime === cursorTimeAfter, { cursorTime, cursorTimeAfter });
  check('deep: the cursor line follows the zoom', cursorX !== cursorXAfter, { cursorX, cursorXAfter });

  // second cursor, then delta must survive another zoom
  await cdp.clickAt(canvas.x + 420, canvas.y + 15, 'right');
  await sleep(700);
  const deltaBefore = await cdp.eval(`window.__qa.text('.timebar-readout')`);
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 300, y: canvas.y + 20, deltaX: 0, deltaY: 120, modifiers: 2 });
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(800);
  const deltaAfter = await cdp.eval(`window.__qa.text('.timebar-readout')`);
  note('delta', { deltaBefore, deltaAfter });
  check('deep: the delta keeps its value when zooming', deltaBefore === deltaAfter, { deltaBefore, deltaAfter });

  // reverse drag: right-to-left must zoom too (and not invert the scale)
  await cdp.press('f', 'KeyF', 70);
  await sleep(600);
  const beforeReverse = await cdp.eval('window.__qa.axis()');
  await cdp.mouse('mousePressed', canvas.x + 500, canvas.y + 15, { buttons: 1 });
  await cdp.mouse('mouseMoved', canvas.x + 200, canvas.y + 15, { buttons: 1 });
  await cdp.mouse('mouseReleased', canvas.x + 200, canvas.y + 15, { buttons: 0 });
  await sleep(800);
  const afterReverse = await cdp.eval('window.__qa.axis()');
  note('reverse drag', { beforeReverse: beforeReverse.slice(0, 80), afterReverse: afterReverse.slice(0, 80) });
  check('deep: reverse drag-select zooms', beforeReverse !== afterReverse, { changed: beforeReverse !== afterReverse });
  const reverseDebug = await cdp.eval('window.__qa.debug()');
  note('renderer after the reverse drag', reverseDebug);
  check('deep: reverse drag keeps a valid scale', !!reverseDebug && reverseDebug.rows > 0 && reverseDebug.size > 0, reverseDebug && { rows: reverseDebug.rows, size: reverseDebug.size });

  // a plain click on a name must not reorder anything
  await cdp.press('f', 'KeyF', 70);
  await sleep(500);
  const orderBefore = await cdp.eval(`window.__qa.texts('.wave-name-text')`);
  const firstRow = await cdp.eval(`window.__qa.rect('.wave-name-cell')`);
  await cdp.clickAt(firstRow.x + 40, firstRow.y + 15);
  await sleep(700);
  const orderAfter = await cdp.eval(`window.__qa.texts('.wave-name-text')`);
  note('order', { orderBefore, orderAfter });
  check('deep: clicking a name does not reorder rows', JSON.stringify(orderBefore) === JSON.stringify(orderAfter), { orderBefore, orderAfter });

  // dragging a name *does* reorder
  const rows = await cdp.eval(`window.__qa.qa('.wave-name-cell').map(e => { const r = e.getBoundingClientRect(); return {x: Math.round(r.x), y: Math.round(r.y), h: Math.round(r.height)}; })`);
  if (rows.length >= 2) {
    await cdp.mouse('mousePressed', rows[1].x + 40, rows[1].y + 15, { buttons: 1 });
    await cdp.mouse('mouseMoved', rows[0].x + 40, rows[0].y + 5, { buttons: 1 });
    await cdp.mouse('mouseReleased', rows[0].x + 40, rows[0].y + 5, { buttons: 0 });
    await sleep(700);
    const orderDragged = await cdp.eval(`window.__qa.texts('.wave-name-text')`);
    note('order after drag', orderDragged);
    check('deep: dragging a name reorders rows', JSON.stringify(orderDragged) !== JSON.stringify(orderAfter), { orderAfter, orderDragged });
  }

  // the settings button must open the settings of *the row it belongs to*
  await cdp.eval(`window.__qa.click('.wave-name-text', 0)`);   // select row 0
  await sleep(500);
  await cdp.eval(`window.__qa.qa('.wave-more')[3].click()`);   // the "..." of row 1
  await sleep(600);
  s = await state(cdp);
  const rowsNow = await cdp.eval(`window.__qa.texts('.wave-name-text')`);
  note('settings target', { modal: s.modal, rowsNow });
  check('deep: settings open for the row whose button was clicked', (s.modal || '') === (rowsNow[1] || ''), { modal: s.modal, row1: rowsNow[1] });
  await cdp.eval(`window.__qa.click('.overlay')`);
  await sleep(500);
  s = await state(cdp);
  check('deep: the overlay closes the dialog', !s.modal, s.modal);
  check('deep: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-deep-wave.png');
};

// editor / LSP interactions
scenarios.deepEditor = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(700);

  // pick the *second* candidate
  const token = await cdp.eval(`(function(){
    const spans = Array.from(document.querySelectorAll('.view-line span span'));
    const span = spans.find(s => s.textContent.trim() === 'pc_reg');
    if (!span) return null;
    const r = span.getBoundingClientRect();
    return {x: Math.round(r.left + 4), y: Math.round(r.top + r.height / 2)};
  })()`);
  if (token) {
    await cdp.clickAt(token.x, token.y);
    await sleep(800);
    await cdp.eval(`window.__qa.click('.modal .sig-row', 1)`);
    await sleep(1200);
    const s = await state(cdp);
    note('second candidate', s.waveNames);
    check('deep: the second candidate is added', (s.waveNames[0] || '').includes('u_cpu16'), s.waveNames);
  } else {
    check('deep: the second candidate is added', false, 'no token');
  }

  // switching tabs keeps both buffers
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'top.sv')`);
  await sleep(700);
  const topEditor = await cdp.eval('window.__qa.editor()');
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(700);
  const cpuEditor = await cdp.eval('window.__qa.editor()');
  note('tab switch', { top: topEditor, cpu: cpuEditor });
  check('deep: switching tabs switches the buffer', topEditor.uri.includes('top.sv') && cpuEditor.uri.includes('cpu.sv'), { top: topEditor.uri, cpu: cpuEditor.uri });
  const models = await cdp.eval('window.__qa.models()');
  check('deep: one model per opened file', models.length === 2, models.length);

  // F12 = go to definition, through monaco itself
  await cdp.eval(`(function(){
    const editors = monaco.editor.getEditors();
    const editor = editors[0];
    const model = editor.getModel();
    const line = model.getLineContent(19);
    editor.setPosition({lineNumber: 19, column: line.indexOf('pc_reg') + 2});
    editor.revealLineInCenter(19);
  })()`);
  await sleep(700);
  const beforeDefinition = await cdp.eval('window.__qa.editor()');
  await cdp.eval(`(function(){ const e = monaco.editor.getEditors()[0]; e.focus(); })()`);
  await cdp.press('F12', 'F12', 123);
  await sleep(1500);
  const afterDefinition = await cdp.eval('window.__qa.editor()');
  note('F12 definition', { beforeDefinition, afterDefinition });
  check('deep: F12 jumps to the declaration', afterDefinition && afterDefinition.line === 13, afterDefinition);

  // an unsaved edit must survive a tab switch: the model, not the app's copy,
  // is what the user is looking at
  const editResult = await cdp.eval(`(function(){
    const editor = monaco.editor.getEditors()[0];
    const model = editor.getModel();
    editor.executeEdits('qa', [{range: {startLineNumber: 2, startColumn: 1, endLineNumber: 2, endColumn: 1}, text: '// qa edit\\n', forceMoveMarkers: true}]);
    return JSON.stringify({line2: model.getLineContent(2), lines: model.getLineCount()});
  })()`);
  note('edit applied', editResult);
  await sleep(800);
  const lspAfterEdit = await state(cdp);
  note('lsp after edit', lspAfterEdit.lsp);
  check('deep: editing the buffer keeps the lsp in sync', lspAfterEdit.lsp.failures === 0, lspAfterEdit.lsp);
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'top.sv')`);
  await sleep(700);
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(700);
  const editedLine2 = await cdp.eval(`(function(){ const m = monaco.editor.getEditors()[0].getModel(); return m.getLineContent(2); })()`);
  note('line 2 after the edit', editedLine2);
  check('deep: an unsaved edit survives a tab switch', (editedLine2 || '').trim() === '// qa edit', editedLine2);
  check('deep: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-deep-editor.png');
};

// sidebar: filter box, duplicate loads, closing every tab
scenarios.deepSidebar = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  // filter
  await cdp.eval(`(function(){
    const input = document.querySelector('.filter-wrap input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, 'pc');
    input.dispatchEvent(new Event('input', {bubbles: true}));
  })()`);
  await sleep(600);
  const filtered = await cdp.eval(`window.__qa.texts('.sidebar .sig-row .sig-name')`);
  note('filtered signals', filtered);
  check('deep: the filter narrows the signal list', filtered.length > 0 && filtered.every((n) => n.includes('pc')), filtered);
  await cdp.eval(`(function(){
    const input = document.querySelector('.filter-wrap input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
    setter.call(input, '');
    input.dispatchEvent(new Event('input', {bubbles: true}));
  })()`);
  await sleep(400);

  // loading the same source twice must not duplicate the tab
  await cdp.upload([path.join(FIXTURES, 'cpu.sv')]);
  await sleep(1500);
  let s = await state(cdp);
  note('after reloading cpu.sv', { tabs: s.tabs, lsp: s.lsp });
  check('deep: reloading a source keeps one tab', s.tabs.length === 2, s.tabs);
  check('deep: reloading a source keeps one document', s.lsp.documents === 2, s.lsp);

  // loading the same dump twice keeps working
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await sleep(2500);
  const added = await addSignal(cdp, 'clk');
  s = await state(cdp);
  note('after reloading the dump', { names: s.waveNames, status: s.status });
  check('deep: reloading the dump keeps adding rows', !!added && s.waveNames.length === 1, s.waveNames);

  // closing every tab must not leave a dangling editor; a user closes them one
  // at a time, so re-query the button after every close
  for (let i = 0; i < 4; i++) {
    const closed = await cdp.eval(`(function(){ const b = document.querySelector('.tab .tab-close'); if (!b) return false; b.click(); return true; })()`);
    if (!closed) break;
    await sleep(800);
  }
  await sleep(600);
  s = await state(cdp);
  note('all tabs closed', { tabs: s.tabs, editor: s.editor, lsp: s.lsp });
  check('deep: closing every tab clears the tab strip', s.tabs.length === 0, s.tabs);
  check('deep: closing every tab keeps the app alive', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  const addAfterClose = await addSignal(cdp, 'reset');
  note('add after closing every tab', { ok: addAfterClose, status: (await state(cdp)).status });
  check('deep: a signal can still be added after closing every tab', !!addAfterClose, addAfterClose);
  await cdp.shot('qa-deep-sidebar.png');
};


// keys must not act on the waveform while the user is typing in the editor
scenarios.keysInEditor = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await addSignal(cdp, 'clk');
  await addSignal(cdp, 'pc');
  const before = await cdp.eval('window.__qa.axis()');
  // focus the editor and type z / f
  await cdp.eval(`(function(){ const e = monaco.editor.getEditors()[0]; e.focus(); e.setPosition({lineNumber: 2, column: 1}); })()`);
  await sleep(400);
  await cdp.press('z', 'KeyZ', 90);
  await sleep(500);
  const afterZ = await cdp.eval('window.__qa.axis()');
  note('axis after typing z in the editor', { changed: before !== afterZ });
  check('keys: typing z in the editor does not zoom the waveform', before === afterZ, { before: before.slice(0, 60), afterZ: afterZ.slice(0, 60) });
  await cdp.press('f', 'KeyF', 70);
  await sleep(500);
  const afterF = await cdp.eval('window.__qa.axis()');
  check('keys: typing f in the editor does not refit the waveform', before === afterF, { afterZ: afterZ.slice(0, 60), afterF: afterF.slice(0, 60) });
  // the same keys must still work when the wave area has the focus
  await cdp.eval(`document.querySelector('.wave-canvas').focus && document.querySelector('.wave-canvas').focus()`);
  await cdp.eval(`document.activeElement && document.activeElement.blur()`);
  await sleep(300);
  await cdp.press('z', 'KeyZ', 90);
  await sleep(500);
  const afterBodyZ = await cdp.eval('window.__qa.axis()');
  note('axis after z outside the editor', { changed: afterF !== afterBodyZ });
  check('keys: z still zooms when the editor is not focused', afterF !== afterBodyZ, { changed: afterF !== afterBodyZ });
  check('keys: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

// adding a signal whose source tab is closed must not report an error
scenarios.addWithClosedTabs = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  for (let i = 0; i < 4; i++) {
    const closed = await cdp.eval(`(function(){ const b = document.querySelector('.tab .tab-close'); if (!b) return false; b.click(); return true; })()`);
    if (!closed) break;
    await sleep(700);
  }
  const added = await addSignal(cdp, 'clk');
  const s = await state(cdp);
  note('after adding with no tabs open', { names: s.waveNames, status: s.status });
  check('add: the signal is added with every tab closed', !!added && s.waveNames.length === 1, s.waveNames);
  check('add: no confusing error status', !/is not open/i.test(s.status || ''), s.status);
  check('add: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

// remaining corners: FST buses, multi-delete, jump button, cancel, small
// windows, legacy .v and a file that is not valid utf-8
scenarios.extras = async (cdp) => {
  await boot(cdp);
  // FST: a bus with real values
  await cdp.upload([path.join(FIXTURES, 'io_mode_wave.fst')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  const fstNames = await cdp.eval(`window.__qa.texts('.sidebar .sig-row .sig-name')`);
  note('fst signals', fstNames.slice(0, 20));
  const bus = fstNames.find((n) => /payload|args|cmd/.test(n)) || fstNames[0];
  await addSignal(cdp, bus);
  const canvas = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  await cdp.clickAt(canvas.x + 40, canvas.y + 15);
  await sleep(700);
  let s = await state(cdp);
  const fstDebug = await cdp.eval('window.__qa.debug()');
  const fstInk = await cdp.eval('window.__qa.surfaceInk(0, 60)');
  note('fst cursor values', { debug: fstDebug, ink: fstInk });
  check('extras: fst bus is drawn with a value at the cursor', !!fstDebug && fstDebug.changes > 0 && typeof fstDebug.cursor1 === 'number' && fstInk > 0, { changes: fstDebug && fstDebug.changes, cursor1: fstDebug && fstDebug.cursor1, ink: fstInk });
  check('extras: fst axis has a sane range', (await cdp.eval('window.__qa.axis()')).length > 0);

  // jump button of a row name (fresh page: the FST above is still the dump)
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(400);
  await selectModule(cdp, 'u_cpu');
  await addSignal(cdp, 'pc_reg');
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'top.sv')`);
  await sleep(600);
  await cdp.eval(`window.__qa.click('.wave-more', 0)`);   // the "go to declaration" arrow
  await sleep(900);
  s = await state(cdp);
  note('after the jump arrow', { tab: s.activeTab, editor: s.editor, status: s.status });
  check('extras: the arrow jumps to the declaration', s.activeTab === 'cpu.sv' && s.editor && s.editor.line === 13, { tab: s.activeTab, editor: s.editor });

  // cancel the picker
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(600);
  const token = await cdp.eval(`(function(){
    const spans = Array.from(document.querySelectorAll('.view-line span span'));
    const span = spans.find(s => s.textContent.trim() === 'pc_reg');
    if (!span) return null;
    const r = span.getBoundingClientRect();
    return {x: Math.round(r.left + 4), y: Math.round(r.top + r.height / 2)};
  })()`);
  const rowsBefore = (await state(cdp)).waveNames.length;
  if (token) {
    await cdp.clickAt(token.x, token.y);
    await sleep(700);
    await cdp.eval(`window.__qa.clickText('.modal .btn', 'Cancel')`);
    await sleep(600);
    const after = await state(cdp);
    check('extras: cancelling the picker adds nothing', !after.modal && after.waveNames.length === rowsBefore, { modal: after.modal, rows: after.waveNames.length, rowsBefore });
  }

  // delete a multi-selection (ctrl click two rows, then Remove signal)
  await addSignal(cdp, 'clk');
  const rowBoxes = await cdp.eval(`window.__qa.qa('.wave-name-cell').map(e => { const r = e.getBoundingClientRect(); return {x: Math.round(r.x), y: Math.round(r.y)}; })`);
  if (rowBoxes.length >= 2) {
    await cdp.mouse('mousePressed', rowBoxes[0].x + 30, rowBoxes[0].y + 15, { buttons: 1 });
    await cdp.mouse('mouseReleased', rowBoxes[0].x + 30, rowBoxes[0].y + 15, { buttons: 0 });
    await sleep(400);
    await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
    await cdp.mouse('mousePressed', rowBoxes[1].x + 30, rowBoxes[1].y + 15, { buttons: 1, modifiers: 2 });
    await cdp.mouse('mouseReleased', rowBoxes[1].x + 30, rowBoxes[1].y + 15, { buttons: 0, modifiers: 2 });
    await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
    await sleep(600);
    const chosen = await cdp.eval(`window.__qa.qa('.wave-name-cell.chosen').length`);
    note('chosen rows', chosen);
    if (chosen >= 2) {
      await cdp.eval(`window.__qa.click('.wave-more', 1)`);
      await sleep(500);
      await cdp.eval(`window.__qa.clickText('.modal .btn', 'Remove signal')`);
      await sleep(800);
      s = await state(cdp);
      note('after multi delete', s.waveNames);
      check('extras: deleting a selection removes every chosen row', s.waveNames.length === 0, s.waveNames);
    } else {
      check('extras: deleting a selection removes every chosen row', false, 'could not select two rows');
    }
  }

  // a legacy .v file joins the design like .sv
  await cdp.upload([path.join(FIXTURES, 'legacy.v')]);
  await sleep(2500);
  s = await state(cdp);
  note('after legacy.v', { tabs: s.tabs, lsp: s.lsp, status: s.status });
  check('extras: a .v file opens as a source', s.tabs.some((t) => t.endsWith('.v')), s.tabs);
  check('extras: the .v file is a document too', s.lsp.documents === s.tabs.length, { docs: s.lsp.documents, tabs: s.tabs.length });

  // a source that is not utf-8 must be reported, not parsed as garbage
  await cdp.upload([path.join(FIXTURES, 'binary.sv')]);
  await sleep(2000);
  s = await state(cdp);
  note('after binary.sv', { tabs: s.tabs, status: s.status });
  check('extras: a non utf-8 source is reported', /utf-8/i.test(s.status || ''), s.status);
  check('extras: a non utf-8 source is not opened as a tab', !s.tabs.some((t) => t === 'binary.sv'), s.tabs);

  // a tiny window must not break the layout
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 700, height: 520, deviceScaleFactor: 1, mobile: false });
  await sleep(1200);
  const tiny = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  note('tiny window canvas', tiny);
  check('extras: the wave canvas stays usable in a tiny window', tiny && tiny.w > 50, tiny);
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await sleep(800);
  check('extras: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-extras.png');
};

// ---------------------------------------------------------------------------
// R2 layout rules (docs/state-store.md §3)
//
// They are asserted before the code exists: `pending()` reports them as
// PENDING and `QA_LAYOUT=1` (run-qa.ps1 -Layout) turns them into hard
// requirements once R2 step 1..4 land.
// ---------------------------------------------------------------------------
const paneBox = (cdp) => cdp.eval(`(function(){
  const box = (sel) => { const e = document.querySelector(sel); if (!e) return null; const r = e.getBoundingClientRect(); return { h: Math.round(r.height), w: Math.round(r.width), visible: r.height > 0 && r.width > 0 }; };
  const app = document.querySelector('.app');
  const sidebarVar = app ? getComputedStyle(app).getPropertyValue('--sidebar-w').trim() : '';
  const editorVar = app ? getComputedStyle(app).getPropertyValue('--editor-h').trim() : '';
  return {
    code: box('.code-pane'), codeHost: box('.code-host'), wave: box('.wave-pane'), sidebar: box('.sidebar'),
    tabs: box('.tabs'), main: box('.main-pane'), splitter: box('.splitter'),
    waveSplitter: box('.wave-splitter'),
    // the load/drive view shares the wave frame as a tab (no dock card anymore)
    inspector: box('.inspector-panel'),
    dockTabs: box('.dock-tabs'),
    sidebarVar, editorVar,
    rowHeight: parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--row-h')) || 30,
  };
})()`);

async function dragWaveSplitter(cdp, dy) {
  const rect = await cdp.eval(`(function(){ const e = document.querySelector('.wave-splitter'); if (!e) return null; const r = e.getBoundingClientRect(); return { x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2) }; })()`);
  if (!rect) return false;
  await cdp.mouse('mousePressed', rect.x, rect.y, { buttons: 1 });
  await cdp.mouse('mouseMoved', rect.x, rect.y + Math.round(dy / 2), { buttons: 1 });
  await cdp.mouse('mouseMoved', rect.x, rect.y + dy, { buttons: 1 });
  await cdp.mouse('mouseReleased', rect.x, rect.y + dy, { buttons: 0 });
  await sleep(700);
  return true;
}

// The shell re-render counter (R2 step 1). `null` while the seam is missing.
async function renderCount(cdp) {
  return cdp.eval('window.__waveDebug && typeof window.__waveDebug.render_count === "number" ? window.__waveDebug.render_count : null');
}

async function dragSidebarSplitter(cdp, dx) {
  const box = await paneBox(cdp);
  if (!box.splitter) return null;
  const x = box.splitter.w === 0 ? 0 : Math.round((box.sidebar ? box.sidebar.w : 280) + 2);
  const y = 400;
  const renderBefore = await renderCount(cdp);
  await cdp.mouse('mousePressed', x, y, { buttons: 1 });
  await sleep(200);
  // the drag start itself renders once (shield + dragging class); the window we
  // care about starts after that
  const renderAtStart = await renderCount(cdp);
  await cdp.mouse('mouseMoved', x + Math.round(dx / 2), y, { buttons: 1 });
  await sleep(150);
  await cdp.mouse('mouseMoved', x + dx, y, { buttons: 1 });
  await sleep(250);
  // measured BEFORE mouseup: this window must not re-render the shell
  const mid = await paneBox(cdp);
  const renderMid = await renderCount(cdp);
  await cdp.mouse('mouseReleased', x + dx, y, { buttons: 0 });
  await sleep(500);
  const after = await paneBox(cdp);
  const renderAfter = await renderCount(cdp);
  return { mid, after, renderBefore, renderAtStart, renderMid, renderAfter };
}

// R2 step 2 (B2) tier-A gate: a gesture may repaint the canvas and re-render the
// time bar on commit, but it must not re-render the pane (or the shell) while the
// pointer is moving. Counts are `{shell, pane}` from `__waveDebug`.
scenarios.tierA = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  await addSignal(cdp, 'clk');
  await addSignal(cdp, 'pc');
  await sleep(500);
  // the gestures must land on the *wave surface*, not on the name column
  // (left of the canvas rect): a name-column click is a reorder/selection
  const surface = await cdp.eval(`window.__qa.rect('#wave-surface')`);
  if (!surface) { check('tierA: the wave area is there', false, 'no #wave-surface'); return; }
  const x0 = Math.round(surface.x + Math.min(160, surface.w / 3));
  const y = Math.round(surface.y + 40);
  const counts = async () => ({
    shell: await cdp.eval('window.__qa.dbg("render_count")'),
    pane: await cdp.eval('window.__qa.dbg("pane_render_count")'),
  });

  // --- a cursor click: press, a one pixel hover, release --------------------
  const hashBefore = await cdp.eval('window.__qa.surfaceHash()');
  const readoutBefore = await cdp.eval(`window.__qa.text('.timebar-readout code')`);
  const before = await counts();
  await cdp.mouse('mousePressed', x0, y, { buttons: 1 });
  await sleep(300);
  // the press itself may settle one render (focus/scroll); the window that must
  // stay render free is the pointer movement before the commit
  const atStart = await counts();
  await cdp.mouse('mouseMoved', x0 + 1, y, { buttons: 1 });
  await sleep(250);
  const mid = await counts();
  await cdp.mouse('mouseReleased', x0 + 1, y, { buttons: 0 });
  await sleep(500);
  const after = await counts();
  const hashAfter = await cdp.eval('window.__qa.surfaceHash()');
  const readoutAfter = await cdp.eval(`window.__qa.text('.timebar-readout code')`);
  note('cursor click renders', { before, atStart, mid, after });
  pending('tierA: a cursor click re-renders nothing while the button is down',
    atStart.pane === mid.pane && atStart.shell === mid.shell, { before, atStart, mid });
  check('tierA: the cursor click is committed on mouseup',
    hashAfter !== hashBefore || readoutAfter !== readoutBefore,
    { hashBefore, hashAfter, readoutBefore, readoutAfter });
  check('tierA: the cursor commit does not re-render the shell', after.shell === atStart.shell,
    { before, atStart, after });

  // --- drag a range to zoom: press, two moves, release ----------------------
  const hashB2 = await cdp.eval('window.__qa.surfaceHash()');
  await cdp.mouse('mousePressed', x0, y, { buttons: 1 });
  await sleep(200);
  const atDrag = await counts();
  await cdp.mouse('mouseMoved', x0 + 80, y, { buttons: 1 });
  await sleep(200);
  await cdp.mouse('mouseMoved', x0 + 160, y, { buttons: 1 });
  await sleep(250);
  const mid2 = await counts();
  await cdp.mouse('mouseReleased', x0 + 160, y, { buttons: 0 });
  await sleep(600);
  const after2 = await counts();
  const hashA2 = await cdp.eval('window.__qa.surfaceHash()');
  const sizeAfter = await cdp.eval('window.__qa.dbg("size")');
  const axisAfter = await cdp.eval('window.__qa.axis()');
  note('drag zoom renders', { atDrag, mid2, after2, sizeAfter });
  pending('tierA: a drag-zoom re-renders nothing during the drag',
    atDrag.pane === mid2.pane && atDrag.shell === mid2.shell, { atDrag, mid2 });
  check('tierA: the drag-zoom is committed on mouseup', hashA2 !== hashB2, { same: hashA2 === hashB2 });
  check('tierA: the drag-zoom commit does not re-render the shell', after2.shell === atDrag.shell,
    { after2, atDrag });
  check('tierA: the committed zoom reaches the ruler', !!axisAfter, (axisAfter || '').slice(0, 80));
};

scenarios.layoutRules = async (cdp) => {
  // -- the two rules that already hold: live resize + commit on mouseup -----
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  const start = await paneBox(cdp);
  check('layout: the sidebar and the main pane are laid out', !!start.sidebar && !!start.main, { sidebar: start.sidebar, main: start.main });

  const dragged = await dragSidebarSplitter(cdp, 140);
  note('splitter drag', dragged && { mid: dragged.mid.sidebar, after: dragged.after.sidebar, vars: [dragged.after.sidebarVar, dragged.after.editorVar] });
  check('layout: the splitter follows the pointer before mouseup', !!dragged && dragged.mid.sidebar.w > start.sidebar.w + 100, { before: start.sidebar && start.sidebar.w, mid: dragged && dragged.mid.sidebar.w });
  check('layout: the width is committed on mouseup', !!dragged && dragged.after.sidebar.w === dragged.mid.sidebar.w, { mid: dragged && dragged.mid.sidebar.w, after: dragged && dragged.after.sidebar.w });

  // an unrelated re-render must not lose the committed width
  await cdp.eval(`window.__qa.click('.topbar .icon-btn')`);
  await sleep(400);
  await cdp.eval(`window.__qa.click('.topbar .icon-btn')`);
  await sleep(500);
  const afterRerender = await paneBox(cdp);
  check('layout: the committed width survives a re-render', !!afterRerender.sidebar && afterRerender.sidebar.w === dragged.after.sidebar.w, { committed: dragged.after.sidebar.w, after: afterRerender.sidebar && afterRerender.sidebar.w });

  // R2 step 1: the drag window itself must not re-render the shell (that is the
  // whole point of writing the CSS variable directly), and the commit must
  // re-render it exactly once.
  const renders = dragged && { before: dragged.renderBefore, atStart: dragged.renderAtStart, mid: dragged.renderMid, after: dragged.renderAfter };
  note('shell renders around the drag', renders);
  pending('layout: a splitter drag re-renders nothing', !!renders && typeof renders.atStart === 'number' && renders.atStart === renders.mid,
    { renders, hint: 'window.__waveDebug.render_count (R2 step 1)' });
  check('layout: the splitter commit re-renders the shell exactly once', !!renders && typeof renders.after === 'number' && renders.after === renders.mid + 1, renders);

  // -- the collapse rule: no sources -> editor becomes a tab strip ----------
  const emptyBoot = await boot(cdp);
  const emptyBox = await paneBox(cdp);
  note('empty app panes', emptyBox);
  const tabStripMax = 40;
  pending('layout: the editor collapses with no sources', !!emptyBox.code && emptyBox.code.h <= tabStripMax, { codeH: emptyBox.code && emptyBox.code.h, max: tabStripMax });
  pending('layout: the collapsed editor has no content', !emptyBox.codeHost || emptyBox.codeHost.h === 0, { contentH: emptyBox.codeHost && emptyBox.codeHost.h, rule: 'tab strip only, content height 0' });
  // the wave *frame* takes the rest: its tab strip (28 px) and the splitter
  // (5 px) are part of the column now
  pending('layout: the wave pane takes the rest of an empty app', !!emptyBox.wave && !!emptyBox.main
      && emptyBox.wave.h >= emptyBox.main.h - tabStripMax - 34,
    { wave: emptyBox.wave && emptyBox.wave.h, main: emptyBox.main && emptyBox.main.h, rule: 'main - editor strip - frame tabs - splitter' });

  // a dump alone (still no sources) keeps the editor collapsed
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  const dumpOnly = await paneBox(cdp);
  pending('layout: a dump alone keeps the editor collapsed', !!dumpOnly.code && dumpOnly.code.h <= tabStripMax, { codeH: dumpOnly.code && dumpOnly.code.h });

  // -- opening a source expands it; the splitter keeps four rows -----------
  await cdp.upload([path.join(FIXTURES, 'top.sv')]);
  await waitTabs(cdp, 1);
  await sleep(1200);
  // The four-row rule is about *rows*: with no signal in the wave view
  // `__waveDebug.visible` is 0 whatever the pane height is, so add signals
  // before dragging the splitter.
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(500);
  for (const name of ['clk', 'reset', 'instr', 'pc', 'data_out', 'result']) {
    await addSignal(cdp, name);
  }
  await sleep(900);
  const withSource = await paneBox(cdp);
  note('panes with a source', withSource);
  pending('layout: opening a source expands the editor', !!withSource.code && !!dumpOnly.code && withSource.code.h >= dumpOnly.code.h + 100, { collapsed: dumpOnly.code && dumpOnly.code.h, expanded: withSource.code && withSource.code.h });
  pending('layout: the expanded editor keeps at least 120 px of content', !!withSource.codeHost && withSource.codeHost.h >= 120, { contentH: withSource.codeHost && withSource.codeHost.h, min: 120 });

  if (withSource.waveSplitter) {
    const rowH = withSource.rowHeight || 30;
    const mainH = (withSource.main && withSource.main.h) || 800;
    // (1) drag to the bottom: the 4-row wave floor wins over the ratio, and the
    // editor never shrinks below its 120 px content floor
    await dragWaveSplitter(cdp, 600);
    const squashed = await paneBox(cdp);
    const squashedDebug = await cdp.eval('window.__qa.debug()');
    note('after squashing the wave pane', { wave: squashed.wave, codeHost: squashed.codeHost, debug: squashedDebug });
    pending('layout: the wave pane keeps four rows', !!squashed.wave && squashed.wave.h >= 4 * rowH && !!squashedDebug && squashedDebug.visible >= 4,
      { waveH: squashed.wave && squashed.wave.h, visible: squashedDebug && squashedDebug.visible, rowHeight: rowH });
    pending('layout: the editor never goes below 120 px of content', !!squashed.codeHost && squashed.codeHost.h >= 120,
      { contentH: squashed.codeHost && squashed.codeHost.h, min: 120 });
    // (2) drag it back to the top: the ratio is clamped to 75 %
    await dragWaveSplitter(cdp, -900);
    const tall = await paneBox(cdp);
    note('after dragging the editor to the top', { code: tall.code, codeHost: tall.codeHost, main: tall.main });
    pending('layout: the editor ratio is clamped to 75 %', !!tall.code && tall.code.h <= mainH * 0.75 + 40,
      { codeH: tall.code && tall.code.h, max: Math.round(mainH * 0.75 + 40) });
  } else {
    pending('layout: the wave pane keeps four rows', false, 'no .wave-splitter yet (R2 step 4)');
    pending('layout: the editor never goes below 120 px of content', false, 'no .wave-splitter yet (R2 step 4)');
    pending('layout: the editor ratio is clamped to 75 %', false, 'no .wave-splitter yet (R2 step 4)');
  }

  // -- the load/drive view is a tab of the wave frame, not a dock ----------
  const freshBoot = await boot(cdp);
  const freshBox = await paneBox(cdp);
  const freshTabs = await cdp.eval(`(function(){
    const tabs = Array.from(document.querySelectorAll('.dock-tab')).map((b) => b.innerText.trim());
    const old = !!document.querySelector('.inspector');
    const panel = document.querySelector('.inspector-panel');
    return { tabs, oldDock: old, panelHidden: panel ? getComputedStyle(panel).display === 'none' : null,
             active: (document.querySelector('.dock-tab.active') || {}).innerText || null };
  })()`);
  check('layout: the load/drive view is a tab of the wave frame with the waveform',
    freshTabs.tabs.length === 2 && freshTabs.tabs[0].includes('Waveform') && freshTabs.tabs[1].includes('Drivers')
      && !freshTabs.oldDock && freshTabs.panelHidden === true,
    Object.assign({ tabs: freshTabs }, { dock: freshBox && freshBox.inspector }));
  pending('layout: the wave frame keeps its tab strip above the ruler',
    !!freshBox.dockTabs && freshBox.dockTabs.h > 0 && freshBox.dockTabs.h <= 40, freshBox.dockTabs);

  check('layout: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-layout-rules.png');
};
// ---------------------------------------------------------------------------
// The end of the trace: nothing is drawn past it and the view stops there.
// ---------------------------------------------------------------------------
scenarios.traceEnd = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await addSignal(cdp, 'clk');
  await addSignal(cdp, 'pc');
  const read = () => cdp.eval(`(function(){
    const d = window.__qa.debug(), r = document.querySelector('#wave-surface').getBoundingClientRect();
    return { x_axis: d.x_axis, size: d.size, trace_end: d.trace_end, w: r.width, h: r.height, left: Math.round(r.left), top: Math.round(r.top) };
  })()`);
  let s = await read();
  check('traceEnd: the dump reports its end', typeof s.trace_end === 'number' && s.trace_end > 0, s);
  const x = s.left + Math.round(s.w * 0.5), y = s.top + 20;
  const wheel = async (dy) => {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x, y, deltaX: 0, deltaY: dy, button: 'none', buttons: 0 });
    await sleep(200);
  };
  // zoom out (ctrl + wheel down) until the trace ends inside the surface
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17, 2);
  await sleep(120);
  for (let i = 0; i < 3; i++) await wheel(120);
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17, 0);
  await sleep(600);
  s = await read();
  const endX = (s.trace_end - s.x_axis) * s.size;
  note('traceEnd geometry', { endX, w: s.w, view: { x_axis: s.x_axis, size: s.size } });
  check('traceEnd: zooming out puts the end inside the surface', endX < s.w - 80, { endX, w: s.w });
  const beyond = await cdp.eval(`(function(){
    const c = document.querySelector('#wave-surface');
    const ctx = c.getContext('2d');
    const dpr = c.width / c.getBoundingClientRect().width;
    const x0 = Math.max(0, Math.round((${endX} + 14) * dpr));
    const w = c.width - x0;
    if (w <= 0) return -1;
    const d = ctx.getImageData(x0, 0, w, Math.round(26 * dpr)).data;
    const near = (r,g,b) => Math.abs(r-76)<=45 && Math.abs(g-194)<=45 && Math.abs(b-255)<=45;
    let n = 0;
    for (let i = 0; i < d.length; i += 4) if (d[i+3] > 120 && near(d[i], d[i+1], d[i+2])) n++;
    return n;
  })()`);
  check('traceEnd: no trace is drawn past the end of the dump', beyond === 0, { pixels: beyond });
  // Pan right with the plain wheel: since Wave 18 a plain wheel pans the time
  // axis (before it scrolled the row list, so this block was not exercising the
  // pan clamp at all). Zoom back in first: a fully zoomed-out view cannot pan.
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17, 2);
  await sleep(120);
  for (let i = 0; i < 4; i++) await wheel(-120);
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17, 0);
  await sleep(400);
  const beforePan = await read();
  note('before panning right', { x_axis: beforePan.x_axis, size: beforePan.size, trace_end: beforePan.trace_end });
  for (let i = 0; i < 30; i++) await wheel(120);
  await sleep(700);
  s = await read();
  const span = s.w / s.size;
  const rightEdge = s.x_axis + span;
  note('after panning right', { x_axis: s.x_axis, size: s.size, span, rightEdge, trace_end: s.trace_end });
  check('traceEnd: the plain wheel pans the view forward', s.x_axis > beforePan.x_axis + 1, { before: +beforePan.x_axis.toFixed(1), after: +s.x_axis.toFixed(1) });
  check('traceEnd: the view cannot be panned past the end',
    span >= s.trace_end ? Math.abs(s.x_axis) <= 0.5 : rightEdge <= s.trace_end + 1.0,
    { span: +span.toFixed(1), rightEdge: +rightEdge.toFixed(1), trace_end: s.trace_end, x_axis: +s.x_axis.toFixed(1) });
  check('traceEnd: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-trace-end.png');
};

// ---------------------------------------------------------------------------
// The waveform and the load/drive view share the wave frame as two tabs.
// ---------------------------------------------------------------------------
scenarios.waveTabs = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await addSignal(cdp, 'clk');
  await addSignal(cdp, 'pc');
  const tabs = () => cdp.eval(`Array.from(document.querySelectorAll('.dock-tab')).map((b) => ({ text: b.innerText.trim(), active: b.className.includes('active') }))`);
  const frame = () => cdp.eval(`(function(){
    const q = (s) => document.querySelector(s);
    const r = (s) => { const e = q(s); if (!e) return null; const b = e.getBoundingClientRect(); return [Math.round(b.width), Math.round(b.height)]; };
    const c = q('#wave-surface');
    return {
      wave: q('#wave-pane') ? getComputedStyle(q('#wave-pane')).display : null,
      panel: q('.inspector-panel') ? getComputedStyle(q('.inspector-panel')).display : null,
      canvas: r('#wave-surface'), bitmap: c ? [c.width, c.height] : null,
      view: [window.__qa.dbg('x_axis'), window.__qa.dbg('size')],
      oldDock: !!q('.inspector'),
    };
  })()`);
  const first = await tabs();
  check('tabs: the wave frame has a waveform and a load/drive tab',
    first.length === 2 && /waveform/i.test(first[0].text) && /drivers/i.test(first[1].text) && first[0].active === true, first);
  const before = await frame();
  note('frame before switching', before);
  check('tabs: no dock card between the editor and the waveform', before.oldDock === false && before.wave === 'flex', before);
  await cdp.eval(`window.__qa.clickText('.dock-tab', 'Drivers / loads')`);
  await sleep(800);
  const loads = await frame();
  note('frame on the load/drive tab', loads);
  check('tabs: the load/drive tab replaces the waveform in the same frame',
    loads.panel === 'flex' && loads.wave === 'none', loads);
  check('tabs: switching keeps the zoom', JSON.stringify(loads.view) === JSON.stringify(before.view), { before: before.view, loads: loads.view });
  await cdp.eval(`window.__qa.clickText('.dock-tab', 'Waveform')`);
  await sleep(900);
  const back = await frame();
  note('frame back on the waveform', back);
  check('tabs: the waveform comes back at full size',
    back.wave === 'flex' && back.canvas && back.canvas[0] > 100 && back.canvas[1] > 100, back);
  check('tabs: the canvas is repainted for the visible frame',
    back.bitmap && back.canvas && Math.abs(back.bitmap[0] - back.canvas[0]) <= 1 && Math.abs(back.bitmap[1] - back.canvas[1]) <= 1, back);
  check('tabs: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-wave-tabs.png');
};

// ---------------------------------------------------------------------------
// Navigation a waveform viewer is expected to have: next/previous transition,
// a goto-time box, a draggable ruler, select-all/clear and copy path.
// ---------------------------------------------------------------------------
scenarios.navigation = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await addSignal(cdp, 'clk');
  await addSignal(cdp, 'pc');
  await sleep(900);
  const surface = await cdp.eval(`window.__qa.rect('#wave-surface')`);
  // select row 0 with ctrl (a plain click would also jump to the source)
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  await cdp.clickAt(surface.x - 120, surface.y + 15);
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(600);
  const chosen = await cdp.eval(`window.__qa.qa('.wave-name-cell.chosen').length`);
  note('selected rows', chosen);
  // a cursor to start from
  await cdp.clickAt(surface.x + 40, surface.y + 15);
  await sleep(500);
  const first = await cdp.eval('window.__qa.dbg("cursor1")');
  await cdp.press('n', 'KeyN', 78);
  await sleep(350);
  const next1 = await cdp.eval('window.__qa.dbg("cursor1")');
  await cdp.press('n', 'KeyN', 78);
  await sleep(350);
  const next2 = await cdp.eval('window.__qa.dbg("cursor1")');
  note('transitions', { first, next1, next2 });
  check('navigation: n moves cursor 1 to the next transition',
    typeof next1 === 'number' && typeof next2 === 'number' && next1 > first && next2 > next1,
    { first, next1, next2 });
  await cdp.key('keyDown', 'Shift', 'ShiftLeft', 16);
  await cdp.press('N', 'KeyN', 78);
  await cdp.key('keyUp', 'Shift', 'ShiftLeft', 16);
  await sleep(400);
  const back = await cdp.eval('window.__qa.dbg("cursor1")');
  check('navigation: shift+n goes back to the previous transition', back === next1, { next1, back });

  // goto box
  await cdp.eval(`(function(){ const i = document.querySelector('.timebar-goto'); i.focus(); i.value = '800'; })()`);
  await cdp.press('Enter', 'Enter', 13);
  await sleep(600);
  // the box gives the keyboard back to the wave area (checked by the shortcuts
  // below); blur here only as a belt-and-braces for the harness
  await cdp.eval('document.activeElement && document.activeElement.blur && document.activeElement.blur()');
  await sleep(200);
  const jumped = await cdp.eval('window.__qa.dbg("cursor1")');
  check('navigation: the goto box puts cursor 1 on the typed time', jumped === 800, jumped);

  // zoom in so there is something to pan, then drag the ruler
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  for (let i = 0; i < 3; i++) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: surface.x + 300, y: surface.y + 20, deltaX: 0, deltaY: -120, modifiers: 2 });
    await sleep(250);
  }
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(500);
  const ruler = await cdp.eval(`window.__qa.rect('.timebar-svg')`);
  const axisBefore = await cdp.eval('window.__qa.dbg("x_axis")');
  await cdp.mouse('mousePressed', ruler.x + 200, ruler.y + 15, { buttons: 1 });
  await cdp.mouse('mouseMoved', ruler.x + 150, ruler.y + 15, { buttons: 1 });
  await sleep(250);
  await cdp.mouse('mouseMoved', ruler.x + 120, ruler.y + 15, { buttons: 1 });
  await sleep(250);
  await cdp.mouse('mouseReleased', ruler.x + 120, ruler.y + 15, { buttons: 0 });
  await sleep(700);
  const axisAfter = await cdp.eval('window.__qa.dbg("x_axis")');
  note('ruler drag', { axisBefore, axisAfter });
  check('navigation: dragging the ruler pans the trace', axisAfter > axisBefore, { axisBefore, axisAfter });

  // a click on the ruler (no drag) drops cursor 1
  const cursorBeforeClick = await cdp.eval('window.__qa.dbg("cursor1")');
  await cdp.clickAt(ruler.x + 240, ruler.y + 15);
  await sleep(600);
  const cursorAfterClick = await cdp.eval('window.__qa.dbg("cursor1")');
  note('ruler click', { cursorBeforeClick, cursorAfterClick });
  check('navigation: clicking the ruler sets cursor 1',
    typeof cursorAfterClick === 'number' && cursorAfterClick !== cursorBeforeClick,
    { cursorBeforeClick, cursorAfterClick });

  // ctrl+a then one Delete removes every row
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  await cdp.press('a', 'KeyA', 65);
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(500);
  const all = await cdp.eval(`window.__qa.qa('.wave-name-cell.chosen').length`);
  check('navigation: ctrl+a selects every row', all === 2, all);
  await cdp.press('Delete', 'Delete', 46);
  await sleep(800);
  const left = await cdp.eval('window.__qa.totalRows()');
  check('navigation: one Delete removes the selected rows', left === 0, left);

  // copy path, in the row settings dialog
  await addSignal(cdp, 'clk');
  await sleep(700);
  await cdp.eval(`(function(){
    const cell = document.querySelectorAll('.wave-name-cell')[0];
    const more = Array.from(cell.querySelectorAll('.wave-more')).find((e) => e.getAttribute('title') === 'Signal settings');
    if (more) more.click();
  })()`);
  await sleep(600);
  const copyButton = await cdp.eval(`(function(){
    const b = Array.from(document.querySelectorAll('.modal .btn')).find((x) => /cop(y|ied)/i.test(x.innerText || ''));
    if (!b) return null;
    const r = b.getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2), text: b.innerText };
  })()`);
  note('copy path button', copyButton);
  check('navigation: the row menu has a copy-path button', !!copyButton, copyButton && copyButton.text);
  if (copyButton) {
    await cdp.clickAt(copyButton.x, copyButton.y);
    await sleep(600);
    const afterCopy = await cdp.eval(`(function(){
      const b = Array.from(document.querySelectorAll('.modal .btn')).find((x) => /cop(y|ied)/i.test(x.innerText || ''));
      return b ? b.innerText : null;
    })()`);
    check('navigation: copy path reports success', !!afterCopy && /copied/i.test(afterCopy), afterCopy);
  }
  check('navigation: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-navigation.png');
};

// ---------------------------------------------------------------------------
// Expand a bus into its bits: one row per bit, most significant first, and the
// derived values must agree with the bus value at the cursor.
// ---------------------------------------------------------------------------
scenarios.busExpand = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await addSignal(cdp, 'pc');
  await sleep(900);
  const openMenu = async () => {
    const gear = await cdp.eval(`(function(){
      const cell = document.querySelectorAll('.wave-name-cell')[0];
      const more = Array.from(cell.querySelectorAll('.wave-more')).find((e) => e.getAttribute('title') === 'Signal settings');
      const r = more.getBoundingClientRect();
      return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
    })()`);
    await cdp.clickAt(gear.x, gear.y);
    await sleep(600);
  };
  const findButton = (re) => cdp.eval(`(function(){
    const b = Array.from(document.querySelectorAll('.modal .btn')).find((x) => ${re}.test(x.innerText || ''));
    if (!b) return null;
    const r = b.getBoundingClientRect();
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2), text: b.innerText };
  })()`);
  await openMenu();
  const expand = await findButton('/expand/i');
  check('busExpand: the row menu offers expanding the bus',
    !!expand && /8/.test(expand.text), expand);
  if (expand) { await cdp.clickAt(expand.x, expand.y); await sleep(1400); }
  const names = await cdp.eval(`Array.from(document.querySelectorAll('.wave-name-text')).map((e) => e.innerText)`);
  note('rows after expanding', names);
  check('busExpand: one row per bit, most significant first',
    names.length === 9 && /\[7\]$/.test(names[1]) && /\[0\]$/.test(names[8]), names);
  const debug = await cdp.eval('window.__qa.debug()');
  check('busExpand: the bits are decoded and drawn',
    !!debug && debug.rows === 9 && debug.changes > 0, debug && { rows: debug.rows, changes: debug.changes });
  // the derived values must be the bus value's bits
  const surface = await cdp.eval(`window.__qa.rect('#wave-surface')`);
  await cdp.clickAt(surface.x + 200, surface.y + 15);
  await sleep(800);
  const rowValues = await cdp.eval(`Array.from(document.querySelectorAll('.wave-name-cell')).map((c) => ({
    name: ((c.querySelector('.wave-name-text') || {}).innerText || ''),
    value: ((c.querySelector('.wave-value') || {}).innerText || '').trim(),
  }))`);
  note('values at the cursor', rowValues);
  // Wave 18 (task-1): the value column prints bare digits now, so the bus value
  // is plain hex ("23") instead of "'h23".
  const hex = /^([0-9a-f]+)$/i.exec(((rowValues[0] && rowValues[0].value) || '').trim());
  const bits = rowValues.slice(1);
  const number = hex ? parseInt(hex[1], 16) : null;
  const agree = number !== null && bits.length === 8 && bits.every((row, index) => {
    const bit = 7 - index; // the rows are msb first
    return row.value === String((number >> bit) & 1);
  });
  check('busExpand: the bit rows show the bits of the bus value', agree, { bus: rowValues[0] && rowValues[0].value, bits: bits.map((b) => b.value) });
  // collapse again
  await openMenu();
  const collapse = await findButton('/collapse/i');
  check('busExpand: the row menu offers collapsing the bits', !!collapse, collapse && collapse.text);
  if (collapse) { await cdp.clickAt(collapse.x, collapse.y); await sleep(1200); }
  const after = await cdp.eval('window.__qa.totalRows()');
  check('busExpand: collapsing removes the bit rows', after === 1, after);
  check('busExpand: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-bus-expand.png');
};

// ---------------------------------------------------------------------------
// Verdi renderer checks (task-12): bus bands, cursor snapping, value column,
// analog mode. Fixtures live next to the harness (busy.vcd, sparse.vcd).
// ---------------------------------------------------------------------------
scenarios.busBench = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'busy.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 40000);
  await sleep(800);
  const added = await cdp.eval(`(function(){
    const names = Array.from(document.querySelectorAll('.sidebar .sig-row .sig-name'));
    for (const name of names) name.parentElement.click();
    return names.length;
  })()`);
  await sleep(3000);
  const rows = await cdp.eval('window.__qa.totalRows()');
  const canvas = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  const samples = [];
  // Every wave gesture forces a redraw: since Wave 18 a plain wheel pans the
  // time axis (it used to scroll the rows). The sample measures the redraw cost,
  // not the gesture, so the mapping change does not alter what is asserted.
  for (let i = 0; i < 6; i += 1) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 200, y: canvas.y + 30, deltaX: 0, deltaY: i % 2 ? -120 : 120, modifiers: 0 });
    await sleep(450);
    samples.push(await cdp.eval('window.__qa.debug()'));
  }
  const times = samples.map((s) => s && s.redraw_ms).filter((n) => typeof n === 'number').sort((a, b) => a - b);
  const median = times[Math.floor(times.length / 2)];
  note('bus redraw', {
    added, rows, median,
    samples: samples.map((s) => s && ({ redraw_ms: s.redraw_ms, rows: s.rows, changes: s.changes, bands: s.bands, decode_ms: s.decode_ms })),
  });
  check('busy: the bus-heavy dump loads', rows > 10, { added, rows });
  check('busy: redraws are measured', times.length >= 5, times);
  check('busy: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-busy.png');
};

scenarios.busBands = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await addSignal(cdp, 'clk');
  await sleep(900);
  let debug = await cdp.eval('window.__qa.debug()');
  check('busBands: a 1-bit row keeps the step edges (no bands)', debug && debug.bands === 0, { bands: debug && debug.bands, changes: debug && debug.changes });
  const hashStep = await cdp.eval('window.__qa.surfaceHash()');
  await addSignal(cdp, 'pc');
  await sleep(900);
  debug = await cdp.eval('window.__qa.debug()');
  const hashBand = await cdp.eval('window.__qa.surfaceHash()');
  check('busBands: a multi-bit row is drawn as a band', debug && debug.bands > 0, { bands: debug && debug.bands, changes: debug && debug.changes, rows: debug && debug.rows });
  check('busBands: the bus row repainted the canvas', hashBand !== hashStep, { same: hashBand === hashStep });
  note('bus band counts', { bands: debug && debug.bands, changes: debug && debug.changes });
  check('busBands: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-bus-bands.png');
  // zoom in so the hexagon/band shape and the centred value labels are visible
  const canvas = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  for (let i = 0; i < 9; i += 1) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 300, y: canvas.y + 30, deltaX: 0, deltaY: -120, modifiers: 2 });
    await sleep(250);
  }
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(700);
  note('zoomed band counts', await cdp.eval('window.__qa.debug()'));
  await cdp.shot('qa-bus-bands-zoom.png');
};

scenarios.snap = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'sparse.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(900);
  await addSignal(cdp, 'a');
  await sleep(900);
  const surface = await cdp.eval(`window.__qa.rect('#wave-surface')`);
  const debug = await cdp.eval('window.__qa.debug()');
  if (!surface || !debug) { check('snap: the wave surface is there', false, { surface, debug }); return; }
  const xOf = (time) => Math.round((time - debug.x_axis) * debug.size);
  const y = surface.y + 15;
  note('snap geometry', { x_axis: debug.x_axis, size: debug.size, x4000: xOf(4000), surface });
  // 1. a click a few pixels away from an edge lands exactly on the edge time
  await cdp.clickAt(surface.x + xOf(4000) + 3, y);
  await sleep(500);
  const snapped = await cdp.eval('window.__qa.dbg("cursor1")');
  check('snap: a click next to a value edge snaps to that edge', snapped === 4000, { snapped, expected: 4000 });
  // 2. halfway between two edges there is nothing to snap to
  await cdp.clickAt(surface.x + xOf(5000), y);
  await sleep(500);
  const free = await cdp.eval('window.__qa.dbg("cursor1")');
  check('snap: a click in the middle of a value does not snap', typeof free === 'number' && free !== 4000 && free !== 6000 && Math.abs(free - 5000) <= 15, { free });
  // 3. the second cursor snaps too
  await cdp.clickAt(surface.x + xOf(8000) + 3, y, 'right');
  await sleep(500);
  const second = await cdp.eval('window.__qa.dbg("cursor2")');
  check('snap: cursor 2 snaps to a value edge', second === 8000, { second, expected: 8000 });
  // 4. dragging the cursor line snaps while dragging
  await cdp.clickAt(surface.x + xOf(6000) + 3, y);
  await sleep(500);
  const placed = await cdp.eval('window.__qa.dbg("cursor1")');
  await cdp.mouse('mousePressed', surface.x + xOf(6000) + 1, y, { buttons: 1 });
  await sleep(200);
  await cdp.mouse('mouseMoved', surface.x + xOf(10000) - 2, y, { buttons: 1 });
  await sleep(250);
  await cdp.mouse('mouseReleased', surface.x + xOf(10000) - 2, y, { buttons: 0 });
  await sleep(500);
  const dragged = await cdp.eval('window.__qa.dbg("cursor1")');
  check('snap: dragging the cursor line snaps to the next edge', placed === 6000 && dragged === 10000, { placed, dragged, expected: 10000 });
  check('snap: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-snap.png');
};

scenarios.valueColumn = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await addSignal(cdp, 'clk');
  await addSignal(cdp, 'pc');
  await sleep(900);
  // Wave 18 (task-2): the value column is compact and the LAST element of the
  // label cell. The DOM order is name -> two hover-only action icons ->
  // value column(s), so nothing can reserve a blank strip after the value; a
  // value element exists only while its cursor does, and the pane advertises
  // --wave-value-ch: 0 with no cursor (no fixed 54 px slot at all).
  const layout = () => cdp.eval('window.__qa.waveValueLayout()');
  let box = await layout();
  const cells = box.cells.length;
  const slots = box.cells.reduce((n, cell) => n + cell.values.length, 0);
  note('value layout before a cursor', { ch: box.ch, cells: cells, values: slots, canvas: box.canvas });
  check('value: no value column is rendered before a cursor', cells > 0 && slots === 0, { cells: cells, slots: slots, ch: box.ch });
  check('value: no value space is reserved before a cursor', box.ch === '0', { ch: box.ch });
  const surface = await cdp.eval('window.__qa.rect("#wave-surface")');
  await cdp.clickAt(surface.x + 120, surface.y + 15);
  await sleep(700);
  box = await layout();
  const slots1 = box.cells.reduce((n, cell) => n + cell.values.length, 0);
  const texts1 = box.cells.reduce((all, cell) => all.concat(cell.values.map((value) => value.text)), []);
  const filled1 = texts1.filter((text) => text.length > 0);
  check('value: cursor 1 fills the value column once per row', slots1 === cells && filled1.length >= 1, { cells: cells, slots: slots1, texts: texts1 });
  check('value: the cursor values are bare digits (no radix prefix)', filled1.every((text) => text.charCodeAt(0) !== 39), filled1);
  // Flush to the waveform: the value column ends a few px left of the canvas
  // (6 px cell padding + separator/slack), never a whole empty slot away, and
  // the hover-only icons sit before it.
  const gaps = box.cells.map((cell) => (cell.values.length && box.canvas ? box.canvas.x - cell.values[cell.values.length - 1].rect.right : null));
  const iconsBeforeValues = box.cells.every((cell) => cell.values.length > 0 && cell.mores.length === 2
    && cell.mores.every((more) => more && more.right <= cell.values[0].rect.left));
  note('value column geometry', { gaps: gaps.slice(0, 8), ch: box.ch, canvas: box.canvas, cell: box.cells[0] });
  check('value: the value column is flush to the waveform', gaps.length === cells && gaps.every((gap) => gap !== null && gap >= 0 && gap <= 14), { gaps: gaps.slice(0, 8), canvas: box.canvas });
  check('value: the hover action icons sit before the value columns', iconsBeforeValues, box.cells.slice(0, 2).map((cell) => ({ mores: cell.mores, first: cell.values[0] && cell.values[0].rect })));
  await cdp.clickAt(surface.x + 320, surface.y + 15);
  await sleep(700);
  const texts2 = await cdp.eval('window.__qa.texts(".wave-name-cell .wave-value")');
  check('value: the column follows cursor 1', JSON.stringify(texts1) !== JSON.stringify(texts2), { before: texts1.slice(0, 4), after: texts2.slice(0, 4) });
  const slotsBefore2 = await cdp.eval('window.__qa.qa(".wave-name-cell .wave-value").length');
  await cdp.clickAt(surface.x + 420, surface.y + 15, 'right');
  await sleep(700);
  const slotsWith2 = await cdp.eval('window.__qa.qa(".wave-name-cell .wave-value").length');
  const texts3 = await cdp.eval('window.__qa.texts(".wave-name-cell .wave-value")');
  check('value: cursor 2 adds its own column', slotsWith2 === cells * 2 && slotsBefore2 === cells, { cells: cells, slotsBefore2: slotsBefore2, slotsWith2: slotsWith2 });
  check('value: cursor 2 shows a value', texts3.filter((text) => text && text.length).length >= 2, texts3.slice(0, 6));
  const names = await cdp.eval('window.__qa.texts(".wave-name-text")');
  check('value: the signal names are still there', names.length === cells && names.every((name) => name && name.length), names.slice(0, 4));
  check('value: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-value-column.png');
};

scenarios.analog = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await addSignal(cdp, 'clk');
  await addSignal(cdp, 'pc');
  await sleep(900);
  const before = await cdp.eval('window.__qa.debug()');
  check('analog: off by default', before && before.analog === 0, { analog: before && before.analog });
  // open the settings dialog of the first row and switch analog on
  await cdp.eval(`window.__qa.click('.wave-more', 1)`);
  await sleep(600);
  note('analog modal', await cdp.eval(`({title: window.__qa.text('.modal-title'), sections: window.__qa.texts('.modal-label')})`));
  const toggled = await cdp.eval(`(function(){
    const sections = Array.from(document.querySelectorAll('.modal-section'));
    const section = sections.find((s) => (((s.querySelector('.modal-label') || {}).innerText || '').trim().toUpperCase()) === 'ANALOG');
    if (!section) return 'no-analog-section';
    const button = section.querySelector('button');
    if (!button) return 'no-button';
    button.click();
    return 'ok';
  })()`);
  await sleep(800);
  const debug = await cdp.eval('window.__qa.debug()');
  check('analog: the setting renders an analog trace', toggled === 'ok' && debug && debug.analog >= 1, { toggled, analog: debug && debug.analog, rows: debug && debug.rows });
  check('analog: the other row keeps its digital shape', debug && debug.bands >= 1, { bands: debug && debug.bands });
  // a bus row in analog mode too (it starts as xxxxxxxx, so the trace has a gap)
  await cdp.eval(`window.__qa.click('.wave-more', 3)`);
  await sleep(600);
  await cdp.eval(`(function(){
    const sections = Array.from(document.querySelectorAll('.modal-section'));
    const section = sections.find((s) => (((s.querySelector('.modal-label') || {}).innerText || '').trim().toUpperCase()) === 'ANALOG');
    if (section) section.querySelector('button').click();
  })()`);
  await sleep(800);
  const both = await cdp.eval('window.__qa.debug()');
  check('analog: a bus row renders as a polyline too', both && both.analog === 2 && both.bands === 0, { analog: both && both.analog, bands: both && both.bands });
  check('analog: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-analog-bus.png');
};

// ---------------------------------------------------------------------------
// The analog trace is a zero-order hold, not an interpolated line: between two
// changes the trace must be flat. `sparse.vcd` has an edge every 2000 steps,
// i.e. every ~81 px at fit zoom, so a flat/steep split is unambiguous.
// ---------------------------------------------------------------------------
scenarios.analogStep = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'sparse.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await addSignal(cdp, 'b');
  await sleep(800);
  await cdp.eval(`window.__qa.click('.wave-more', 1)`);
  await sleep(600);
  const toggled = await cdp.eval(`(function(){
    const sections = Array.from(document.querySelectorAll('.modal-section'));
    const section = sections.find((s) => (((s.querySelector('.modal-label') || {}).innerText || '').trim().toUpperCase()) === 'ANALOG');
    if (!section) return 'no-analog-section';
    const button = section.querySelector('button');
    if (!button) return 'no-button';
    button.click();
    return 'ok';
  })()`);
  await sleep(800);
  const debug = await cdp.eval('window.__qa.debug()');
  check('analogStep: the row renders as analog', toggled === 'ok' && debug && debug.analog === 1, { toggled, analog: debug && debug.analog });
  const shape = await cdp.eval(`(function(){
    const c = document.querySelector('#wave-surface');
    const ctx = c.getContext('2d');
    const w = c.width, h = c.height;
    const dpr = c.width / c.getBoundingClientRect().width;
    const y1 = Math.min(h, Math.round(28 * dpr));
    const data = ctx.getImageData(0, 0, w, y1).data;
    const near = (r,g,b) => Math.abs(r-76)<=45 && Math.abs(g-194)<=45 && Math.abs(b-255)<=45;
    const cols = [];
    for (let x = 0; x < w; x++) {
      let sum = 0, n = 0, lo = 1e9, hi = -1e9;
      for (let y = 0; y < y1; y++) {
        const i = (y * w + x) * 4;
        if (data[i+3] > 120 && near(data[i], data[i+1], data[i+2])) { sum += y; n++; if (y < lo) lo = y; if (y > hi) hi = y; }
      }
      cols.push(n ? { y: sum / n, span: hi - lo, n } : null);
    }
    const seen = cols.filter(Boolean);
    let same = 0, moved = 0;
    for (let i = 1; i < seen.length; i++) { if (Math.abs(seen[i].y - seen[i-1].y) < 0.6) same++; else moved++; }
    const tall = seen.filter((c) => c.span > 6).length;
    return { columns: seen.length, same, moved, flat: seen.length ? +(same / (same + moved)).toFixed(3) : null, tall };
  })()`);
  note('analog step shape', shape);
  check('analogStep: the trace holds the value between changes (no interpolation)',
    shape && shape.columns > 50 && shape.flat > 0.85, shape);
  // an interpolated line moves a little in almost every column; a step plot
  // only moves where the value changes
  check('analogStep: only the changes move the trace',
    shape && shape.moved > 0 && shape.moved <= shape.columns / 5, shape);
  check('analogStep: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-analog-step.png');
};

// ---------------------------------------------------------------------------
// task-13: design paths vs dump paths.
//
// `mismatch.vcd` was dumped from a testbench whose root scope is `tb_top`,
// while the sources elaborate `top`; `gen-mismatch.js` adds a scope `x` with its
// own `clk` so `tb_top.x.clk` has no direct design counterpart.
// ---------------------------------------------------------------------------

// The last occurrence of a token in the visible editor (a usage, not the
// declaration): clicking it must add the signal without navigating away.
async function lastToken(cdp, word) {
  return cdp.eval(`(function(){
    const spans = Array.from(document.querySelectorAll('.view-line span span'))
      .filter((s) => s.textContent.trim() === ${JSON.stringify(word)});
    const span = spans[spans.length - 1];
    if (!span) return null;
    const r = span.getBoundingClientRect();
    return {x: Math.round(r.left + 4), y: Math.round(r.top + r.height / 2), count: spans.length};
  })()`);
}

scenarios.mismatch = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'mismatch.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(900);
  const names = await cdp.eval(`window.__qa.texts('.tree-row .tree-name')`);
  note('mismatch tree', names);
  check('mismatch: the dump hierarchy is shown with its own root', names.some((n) => n === 'tb_top'), names);

  // 1. a click on a *usage* of pc_reg: two instances match, the picker opens
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(700);
  const before = await state(cdp);
  const token = await lastToken(cdp, 'pc_reg');
  note('pc_reg token', token);
  check('mismatch: the source token is there', !!token, token);
  if (!token) { return; }
  await cdp.clickAt(token.x, token.y);
  await sleep(900);
  let s = await state(cdp);
  note('after source click', { modal: s.modal, tab: s.activeTab, editor: s.editor, status: s.status });
  check('mismatch: the click opens the candidate picker', !!s.modal, s.modal);
  await cdp.eval(`window.__qa.clickText('.modal .sig-name', 'top.u_cpu.pc_reg')`);
  await sleep(1100);
  s = await state(cdp);
  const debug = await cdp.eval('window.__qa.debug()');
  note('after pick', { names: s.waveNames, editor: s.editor, status: s.status, debug: debug && { rows: debug.rows, changes: debug.changes, bands: debug.bands } });
  check('mismatch: the added row carries the dump path', s.waveNames.some((n) => n.indexOf('tb_top.u_cpu.pc_reg') === 0), s.waveNames);
  check('mismatch: the row added from the source decodes and draws', !!debug && debug.rows >= 1 && debug.changes > 0 && debug.bands > 0, debug && { rows: debug.rows, changes: debug.changes, bands: debug.bands });
  // A plain click on a source name must add the signal and NOT navigate: the
  // old bug made the app jump to the declaration, which the status line shows
  // as a `file:line:col` note instead of the "added" note.
  check('mismatch: adding from the source did not jump to the declaration',
    s.activeTab === before.activeTab && /^added /.test(s.status || '') && !/cpu\.sv:\d+:\d+/.test(s.status || ''),
    { tabBefore: before.activeTab, tabAfter: s.activeTab, status: s.status });

  // 2. the row's ↗?button finds the declaration through the mismatched root
  await cdp.eval(`window.__qa.click('.wave-more', 0)`);
  await sleep(1000);
  s = await state(cdp);
  note('after jump arrow', { tab: s.activeTab, editor: s.editor, status: s.status });
  check('mismatch: the declaration arrow finds the declaration', s.activeTab === 'cpu.sv' && s.editor && s.editor.line === 13, { tab: s.activeTab, editor: s.editor });

  // 3. an ambiguous name is reported instead of guessed: tb_top.x.clk has two
  //    candidate `clk` signals in the design (top.clk, top.u_cpu.clk)
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'top.sv')`);
  await sleep(500);
  // expand the dump root, then select the extra scope `x`
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(500);
  await selectModule(cdp, 'x');
  const added = await addSignal(cdp, 'clk');
  check('mismatch: the extra-scope signal can be added from the sidebar', !!added);
  await sleep(600);
  const rows = await cdp.eval(`window.__qa.texts('.wave-name-text')`);
  note('rows before the ambiguous jump', rows);
  check('mismatch: the added x.clk row carries the dump path', rows.some((row) => row.indexOf('tb_top.x.clk') === 0), rows);
  await cdp.eval(`window.__qa.click('.wave-more', 2)`); // the x.clk row's jump arrow
  await sleep(900);
  const ambiguous = await state(cdp);
  note('after ambiguous jump', { status: ambiguous.status });
  check('mismatch: an ambiguous declaration is reported, not guessed',
    /ambiguous/i.test(ambiguous.status || '') && /\d+ signals match/.test(ambiguous.status || '') && /top\.clk/.test(ambiguous.status || ''),
    ambiguous.status);

  // 4. with no sources at all the message says so instead of "no declaration"
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'mismatch.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  await selectModule(cdp, 'tb_top');
  const addedOnly = await addSignal(cdp, 'clk');
  check('mismatch: a dump-only signal can be added', !!addedOnly);
  await sleep(500);
  await cdp.eval(`window.__qa.click('.wave-more', 0)`);
  await sleep(900);
  const noSources = await state(cdp);
  note('after dump-only jump', { status: noSources.status });
  check('mismatch: the no-sources message is honest', /no sources loaded/i.test(noSources.status || ''), noSources.status);

  check('mismatch: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-mismatch.png');
};

// ---------------------------------------------------------------------------
// task-13: the settings dialog's load/drive buttons, and cursor values as editor
// inlay hints.
// ---------------------------------------------------------------------------

scenarios.loadDrive = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(900);
  // expand the dump root and add three u_cpu signals from the sidebar
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(500);
  await selectModule(cdp, 'u_cpu');
  for (const name of ['active', 'next_pc', 'pc_reg']) {
    await addSignal(cdp, name);
  }
  await sleep(900);
  let s = await state(cdp);
  note('loadDrive rows', s.waveNames);
  check('loadDrive: three rows are there', (s.waveNames || []).length === 3, s.waveNames);
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'top.sv')`);
  await sleep(500);

  // row 0 = active: exactly one driver (assign, cpu.sv:18)
  await cdp.eval(`window.__qa.click('.wave-more', 1)`);
  await sleep(500);
  await cdp.eval(`window.__qa.clickText('.modal .btn', 'drive')`);
  await sleep(900);
  s = await state(cdp);
  note('after drive (active)', { tab: s.activeTab, editor: s.editor, status: s.status });
  check('loadDrive: the drive button jumps to the driver',
    s.activeTab === 'cpu.sv' && !!s.editor && s.editor.line === 18,
    { tab: s.activeTab, editor: s.editor, status: s.status });

  // row 0 = active: nothing reads it, so the load button says so honestly
  await cdp.eval(`window.__qa.click('.wave-more', 1)`);
  await sleep(500);
  await cdp.eval(`window.__qa.clickText('.modal .btn', 'load')`);
  await sleep(700);
  s = await state(cdp);
  note('after load (active)', { status: s.status });
  check('loadDrive: a signal without loads says so', /no load found/i.test(s.status || ''), s.status);

  // row 1 = next_pc: one load (cpu.sv:27)
  await cdp.eval(`window.__qa.click('.wave-more', 3)`);
  await sleep(500);
  await cdp.eval(`window.__qa.clickText('.modal .btn', 'load')`);
  await sleep(900);
  s = await state(cdp);
  note('after load (next_pc)', { tab: s.activeTab, editor: s.editor, status: s.status });
  check('loadDrive: the load button jumps to the load',
    s.activeTab === 'cpu.sv' && !!s.editor && s.editor.line === 27,
    { tab: s.activeTab, editor: s.editor, status: s.status });

  // row 2 = pc_reg: two loads -> open the first and list them in the inspector
  await cdp.eval(`window.__qa.click('.wave-more', 5)`);
  await sleep(500);
  await cdp.eval(`window.__qa.clickText('.modal .btn', 'load')`);
  await sleep(900);
  s = await state(cdp);
  note('after load (pc_reg)', { tab: s.activeTab, editor: s.editor, status: s.status, inspector: s.inspectorBody });
  check('loadDrive: several loads are reported', /2 loads/.test(s.status || '') && /Drivers \/ loads/.test(s.status || ''), s.status);
  check('loadDrive: the inspector lists the loads', /loads \(2\)/i.test(s.inspectorBody || ''), s.inspectorBody);
  check('loadDrive: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-load-drive.png');
};

scenarios.inlayHints = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv'), path.join(FIXTURES, 'wave.vcd')]);
  await waitTabs(cdp, 2);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(900);
  await cdp.eval(`window.__qa.click('.tree-row', 0)`);
  await sleep(400);
  await selectModule(cdp, 'u_cpu');
  await addSignal(cdp, 'pc_reg');
  await sleep(700);
  const canvas = await cdp.eval(`window.__qa.rect('.wave-canvas')`);
  await cdp.clickAt(canvas.x + 200, canvas.y + 15);
  await sleep(700);
  const cursor = await cdp.eval('window.__qa.dbg("cursor1")');
  note('cursor for hints', cursor);
  check('inlayHints: a cursor is set', typeof cursor === 'number', cursor);

  const hintsAt = async () => cdp.eval(`(function(){
    const hook = window.__waveInsightProviders;
    if (!hook || !hook.inlayHints) return {error: 'no-hook'};
    const uri = hook.documentUri('cpu.sv');
    const model = monaco.editor.getModels().find(function (m) { return m.uri.toString() === uri; });
    if (!model) return {error: 'no-model'};
    const result = hook.inlayHints.provideInlayHints(model, {startLineNumber: 1, endLineNumber: 30}, {isCancellationRequested: false, onCancellationRequested: function () { return {dispose: function () {}}; }});
    // monaco 0.44 providers return an object with a hints array; accept a bare array too
    const hints = result && Array.isArray(result.hints) ? result.hints : result;
    return {hasEvent: typeof hook.inlayHints.onDidChangeInlayHints === 'function', hints: hints, wrapper: !!(result && result.hints), debug: window.__waveInlayDebug || null};
  })()`);

  // the hints live in the editor: activate the cpu.sv tab so its model exists
  await cdp.eval(`window.__qa.clickText('.tab .tab-name', 'cpu.sv')`);
  await sleep(800);
  const first = await hintsAt();
  note('inlay hints', { debug: first.debug, error: first.error, count: (first.hints || []).length, first: (first.hints || []).slice(0, 6) });
  check('inlayHints: the provider is registered with a change event',
    !first.error && first.hasEvent === true, first.error || first.hasEvent);
  check('inlayHints: the editor shows cursor values after signal names',
    !first.error && Array.isArray(first.hints) && first.hints.length > 0
      && first.hints.every((hint) => /^[:=] /.test(hint.label) && hint.position && hint.position.lineNumber >= 1),
    first.error || (first.hints || []).slice(0, 4));
  const values1 = (first.hints || []).map((hint) => hint.label);
  const pcRegHints = (first.hints || []).filter((hint) => hint.position.lineNumber === 13);
  note('pc_reg line hints', pcRegHints);
  check('inlayHints: the declaration line has a hint after the name',
    pcRegHints.length >= 1 && pcRegHints.some((hint) => hint.position.column > 8), pcRegHints);

  // moving cursor 1 must change the shown values
  await cdp.clickAt(canvas.x + 620, canvas.y + 15);
  await sleep(800);
  const second = await hintsAt();
  const values2 = (second.hints || []).map((hint) => hint.label);
  note('inlay hints after the cursor moved', { count: values2.length, first: values2.slice(0, 6) });
  check('inlayHints: moving the cursor refreshes the values',
    values1.length > 0 && JSON.stringify(values1) !== JSON.stringify(values2),
    { before: values1.slice(0, 4), after: values2.slice(0, 4) });
  check('inlayHints: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-inlay-hints.png');
};

scenarios.wave1 = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  const added = await cdp.eval('window.__qa.clickSignal("pc")');
  check('wave1: signal added', added === 'ok', added);
  await sleep(900);
  const canvas = await cdp.eval('window.__qa.rect(".wave-canvas")');
  await cdp.clickAt(canvas.x + 420, canvas.y + 15);
  await sleep(600);
  const before = await cdp.eval('window.__qa.texts(".wave-name-cell .wave-value")[0]');
  const opened = await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(500);
  let s = await state(cdp);
  check('wave1: settings modal opens', !!s.modal, s.modal);
  const toggled = await cdp.eval('(function(){ var sec = Array.from(document.querySelectorAll(".modal .modal-section")).filter(function(e){ return (e.textContent||"").indexOf("Fixed point") >= 0; })[0]; if (!sec) return "no-section"; var b = sec.querySelector(".btn"); if (!b) return "no-button"; b.click(); return "ok"; })()');
  check('wave1: fixed point can be enabled', toggled === 'ok', toggled);
  await sleep(700);
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(500);
  const exponent = await cdp.eval('(function(){ var sec = Array.from(document.querySelectorAll(".modal .modal-section")).filter(function(e){ return (e.textContent||"").indexOf("Fixed point") >= 0; })[0]; if (!sec) return "no-section"; var i = sec.querySelector("input"); if (!i) return "no-input"; i.value = "4"; i.dispatchEvent(new Event("change", {bubbles: true})); return "ok"; })()');
  check('wave1: the exponent input accepts a value', exponent === 'ok', exponent);
  await sleep(500);
  await cdp.eval('window.__qa.click(".overlay", 0)');
  await sleep(800);
  const after = await cdp.eval('window.__qa.texts(".wave-name-cell .wave-value")[0]');
  note('fixed point', { before: before, after: after });
  check('wave1: fixed point changes the value readout', !!before && !!after && before !== after && after.indexOf('.') >= 0, { before: before, after: after });
  const flags = await cdp.eval('document.querySelectorAll(".filter-wrap button").length');
  const beforeCount = (await state(cdp)).sidebarSignals.length;
  const typed = await cdp.eval('(function(){ var i = document.querySelector(".filter-wrap input"); if (!i) return "no-input"; i.value = "pc"; i.dispatchEvent(new Event("input", {bubbles: true})); return "ok"; })()');
  await sleep(600);
  const afterCount = (await state(cdp)).sidebarSignals.length;
  note('search', { flags: flags, typed: typed, beforeCount: beforeCount, afterCount: afterCount });
  check('wave1: search option toggles are rendered (case/whole-word/regex)', flags >= 3, flags);
  check('wave1: the filter still narrows the signal list', typed === 'ok' && afterCount > 0 && afterCount < beforeCount, { beforeCount: beforeCount, afterCount: afterCount });
  check('wave1: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-wave1-fixed-point.png');
};
// --- Wave 2: value export, diagnostics chip, loader status -----------------
// The export writes a real file through the browser download path, so this
// scenario watches the download directory instead of the DOM.
scenarios.wave2 = async (cdp) => {
  await boot(cdp);
  const downloads = path.join(QA, 'artifacts', 'downloads');
  fs.rmSync(downloads, { recursive: true, force: true });
  fs.mkdirSync(downloads, { recursive: true });
  await cdp.send('Page.setDownloadBehavior', { behavior: 'allow', downloadPath: downloads });
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  const loaded = await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  let s = await state(cdp);
  note('load status', s.status);
  check('wave2: a normal load still reports the stable prefix', !!loaded, s.status);
  check('wave2: the load report carries timing and wasm memory',
    /opened in \d+ ms/.test(s.status || '') && /wasm memory \d+ MB/.test(s.status || ''), s.status);
  const added = await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(800);
  check('wave2: a row is on the wave', added === 'ok', added);
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(500);
  const hasExport = await cdp.eval('window.__qa.texts(".modal .btn").filter(function(t){ return t.indexOf("CSV (row)") >= 0; }).length');
  check('wave2: the settings dialog offers an export', hasExport === 1, hasExport);
  const clicked = await cdp.eval('window.__qa.clickText(".modal .btn", "CSV (row)")');
  check('wave2: the export button clicks', clicked === 'ok', clicked);
  let file = null;
  for (let i = 0; i < 50 && !file; i++) {
    await sleep(300);
    const names = fs.existsSync(downloads) ? fs.readdirSync(downloads).filter((n) => n.endsWith('.csv')) : [];
    for (const name of names) {
      const full = path.join(downloads, name);
      try { const stat = fs.statSync(full); if (stat.size > 0) { file = { name: name, text: fs.readFileSync(full, 'utf8') }; } } catch (error) { /* still downloading */ }
    }
  }
  note('downloaded', file ? { name: file.name, bytes: file.text.length } : null);
  check('wave2: the export produced a csv file', !!file, file && file.name);
  const lines = file ? file.text.split('\n').filter((l) => l.length > 0) : [];
  check('wave2: the csv has the time,value header', lines.length > 0 && lines[0] === 'time,value', lines.slice(0, 2));
  check('wave2: the csv has one line per change', lines.length > 3, lines.length);
  check('wave2: the csv value column uses the on-screen format',
    /^\d+,\S+$/.test(lines[1] || ''), lines.slice(1, 3));
  // the same row also exports as a MATLAB level-5 file
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(400);
  const matClicked = await cdp.eval('window.__qa.clickText(".modal .btn", "MAT (row)")');
  let mat = null;
  for (let i = 0; i < 50 && !mat; i++) {
    await sleep(300);
    const names = fs.existsSync(downloads) ? fs.readdirSync(downloads).filter((n) => n.endsWith('.mat')) : [];
    for (const name of names) {
      const full = path.join(downloads, name);
      try { const stat = fs.statSync(full); if (stat.size > 0) { mat = { name: name, bytes: fs.readFileSync(full) }; } } catch (error) { /* still downloading */ }
    }
  }
  note('mat download', mat ? { name: mat.name, bytes: mat.bytes.length } : null);
  check('wave2: the MAT export produced a file', matClicked === 'ok' && !!mat, mat && mat.name);
  check('wave2: the MAT file is a level-5 MAT-file',
    !!mat && mat.bytes.length > 128 && mat.bytes.slice(124, 126).toString('hex') === '0001' && mat.bytes.slice(126, 128).toString() === 'IM',
    mat && { header: mat.bytes.slice(0, 116).toString().replace(/\0+$/, ''), version: mat.bytes.slice(124, 128).toString('hex') });
  s = await state(cdp);
  note('export status', s.status);
  check('wave2: the export reports success in the status line', /export/i.test(s.status || ''), s.status);
  await cdp.upload([path.join(FIXTURES, 'malformed.sv')]);
  await sleep(2500);
  // the debug hooks return JSON strings (same shape as stats()), not objects
  const diagnostics = await cdp.eval('window.__waveInsightLsp && window.__waveInsightLsp.diagnostics ? JSON.parse(window.__waveInsightLsp.diagnostics()) : null');
  note('lsp diagnostics', diagnostics);
  check('wave2: the server publishes diagnostics for a broken source',
    !!diagnostics && diagnostics.total > 0, diagnostics && diagnostics.total);
  const markers = await cdp.eval('window.monaco && window.monaco.editor ? window.monaco.editor.getModelMarkers({}).length : -1');
  note('monaco markers', markers);
  check('wave2: the editor receives markers', markers > 0, markers);
  const chip = await cdp.eval('window.__qa.text(".problems-tag")');
  note('problems chip', chip);
  check('wave2: the top bar shows the problem count', /problem/.test(chip || ''), chip);
  check('wave2: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-wave2-export.png');
};

// --- R3: the sidebar virtualizes the module tree and the signal list --------
// A dump with a couple of thousand signals must not put every row in the DOM:
// the seam reports the whole list, the DOM only holds the scroll window, and
// scrolling, filtering, click-to-add and expanding still work.
scenarios.virtualization = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'many_signals.vcd')]);
  const loaded = await waitStatus(cdp, 'waveform loaded', 40000);
  await sleep(1200);
  let s = await state(cdp);
  note('virtualization load', { status: s.status, signals: s.sidebarSignals.length, modules: s.modules.length });
  check('virtualization: the big dump loads', !!loaded, s.status);
  check('virtualization: the seam reports the whole signal list', s.sidebarSignals.length > 2000, s.sidebarSignals.length);
  const seam = await cdp.eval('window.__qa.sidebar()');
  const domRows = await cdp.eval('window.__qa.sidebarDom()');
  note('sidebar seam', seam && { signals: seam.signals.length, total: seam.signals_total, dom: seam.signals_dom, windowed: seam.signals_windowed });
  check('virtualization: the DOM holds only the visible window', domRows.signals > 0 && domRows.signals < 80, domRows);
  check('virtualization: the seam knows the signal list is windowed', !!seam && seam.signals_windowed === true, seam && seam.signals_windowed);
  check('virtualization: the seam window matches the DOM', !!seam && seam.signals_dom === domRows.signals, { seam: seam && seam.signals_dom, dom: domRows.signals });
  const geometry = await cdp.eval('(function(){ var s = document.querySelector(".sidebar .sig-scroll"); if (!s) return null; return { scrollHeight: s.scrollHeight, clientHeight: s.clientHeight }; })()');
  note('signal scroll geometry', geometry);
  check('virtualization: the scrollbar covers the whole list', !!geometry && geometry.scrollHeight > geometry.clientHeight * 3, geometry);
  // scrolling to the bottom draws the end of the list, still bounded
  await cdp.eval('(function(){ var s = document.querySelector(".sidebar .sig-scroll"); s.scrollTop = s.scrollHeight; s.dispatchEvent(new Event("scroll", { bubbles: true })); })()');
  await sleep(900);
  const bottom = await cdp.eval('(function(){ var names = Array.prototype.map.call(document.querySelectorAll(".sidebar .sig-row .sig-name"), function(n){ return n.textContent; }); var seam = window.__waveInsightSidebar || null; return { names: names, rows: names.length, last: seam ? seam.signals[seam.signals.length - 1] : null }; })()');
  note('after scrolling to the bottom', bottom && { rows: bottom.rows, last: bottom.last, tail: bottom.names.slice(-2) });
  check('virtualization: scrolling draws the end of the list', !!bottom && bottom.last !== null && bottom.names.indexOf(bottom.last) >= 0, bottom && { last: bottom.last, tail: bottom.names.slice(-2) });
  check('virtualization: scrolling keeps the DOM bounded', !!bottom && bottom.rows > 0 && bottom.rows < 80, bottom && bottom.rows);
  // the filter still narrows the list, and a filtered big list stays bounded
  await cdp.eval('(function(){ var i = document.querySelector(".filter-wrap input"); var set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set; set.call(i, "sig_00"); i.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await sleep(800);
  s = await state(cdp);
  const filtered = await cdp.eval('window.__qa.sidebar()');
  note('filtered sidebar', { matches: s.sidebarSignals.length, dom: filtered && filtered.signals_dom });
  check('virtualization: the filter still narrows the logical list', s.sidebarSignals.length > 0 && s.sidebarSignals.length < 500 && s.sidebarSignals.every(function (name) { return name.indexOf('sig_00') >= 0; }), s.sidebarSignals.slice(0, 3));
  check('virtualization: the filtered DOM stays bounded', !!filtered && filtered.signals_dom > 0 && filtered.signals_dom < 80, filtered && filtered.signals_dom);
  // clearing the filter restores the whole list
  await cdp.eval('(function(){ var i = document.querySelector(".filter-wrap input"); var set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set; set.call(i, ""); i.dispatchEvent(new Event("input", { bubbles: true })); })()');
  await sleep(700);
  s = await state(cdp);
  check('virtualization: clearing the filter restores the whole list', s.sidebarSignals.length > 2000, s.sidebarSignals.length);
  // a row from the window can still be added to the wave view
  const clicked = await cdp.eval('window.__qa.click(".sidebar .sig-row", 0)');
  await sleep(800);
  s = await state(cdp);
  check('virtualization: a windowed signal row can still be added', clicked === 'ok' && s.waveNames.length === 1, { clicked: clicked, names: s.waveNames });
  // expanding the top scope lists its 120 children; the tree DOM stays bounded
  const treeClick = await cdp.eval('window.__qa.click(".tree-row", 0)');
  await sleep(900);
  const tree = await cdp.eval('(function(){ var seam = window.__waveInsightSidebar || null; return { rows: document.querySelectorAll(".sidebar .tree-row").length, full: seam ? seam.modules.length : 0, total: seam ? seam.modules_total : 0, windowed: seam ? seam.modules_windowed : null }; })()');
  note('module tree after expanding the top scope', tree);
  check('virtualization: expanding the top scope lists every module', treeClick === 'ok' && !!tree && tree.full > 120, tree);
  check('virtualization: the tree DOM stays bounded', !!tree && tree.rows > 0 && tree.rows < 80, tree && tree.rows);
  check('virtualization: the seam knows the tree is windowed', !!tree && tree.windowed === true, tree && tree.windowed);
  check('virtualization: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-virtualization.png');
};
// --- Wave 3: persistence across a reload -----------------------------------
// The app stores layout and per-row display settings in localStorage. This
// scenario bypasses boot()'s storage clear on purpose: it writes, reloads with
// Page.navigate, and asserts the restored state.
scenarios.persistence = async (cdp) => {
  await boot(cdp, false);
  const before = await cdp.eval('window.__qa.rect(".sidebar")');
  const splitter = await cdp.eval('window.__qa.rect(".splitter")');
  await cdp.mouse('mousePressed', splitter.x + 2, splitter.y + 200, { buttons: 1 });
  await cdp.mouse('mouseMoved', splitter.x + 122, splitter.y + 200, { buttons: 1 });
  await cdp.mouse('mouseReleased', splitter.x + 122, splitter.y + 200, { buttons: 0 });
  await sleep(800);
  const committed = await cdp.eval('window.__qa.rect(".sidebar")');
  const layout = await cdp.eval('window.localStorage.getItem("wave-insight.layout.v1")');
  note('layout', { before: before && before.w, committed: committed && committed.w, stored: layout });
  const parsed = layout ? JSON.parse(layout) : null;
  check('persistence: the committed layout is written to localStorage',
    !!parsed && Math.abs(parsed.sidebar_w - committed.w) <= 1.5, { stored: parsed && parsed.sidebar_w, committed: committed && committed.w });
  // reload without clearing storage: the shell must come back at the same width
  await cdp.send('Page.navigate', { url: APP_URL });
  await waitFor(cdp, `!!window.__waveInsightLsp && !!document.querySelector('.app')`, 60000);
  await cdp.eval(PAGE_HELPERS);
  await sleep(1000);
  const restored = await cdp.eval('window.__qa.rect(".sidebar")');
  note('restored layout', restored);
  check('persistence: the sidebar width survives a reload',
    !!restored && Math.abs(restored.w - committed.w) <= 1.5, { committed: committed && committed.w, restored: restored && restored.w });
  // per-row display setting: set Bin on top.pc, reload, re-add the row
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(900);
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(500);
  const chose = await cdp.eval('window.__qa.clickText(".modal .btn", "Bin")');
  await sleep(700);
  const rows = await cdp.eval('window.localStorage.getItem("wave-insight.rows.v1")');
  note('row settings', rows && rows.slice(0, 220));
  check('persistence: the chosen row format is written',
    chose === 'ok' && !!rows && rows.indexOf('Bin') >= 0, { chose: chose, rows: rows && rows.slice(0, 120) });
  await cdp.send('Page.navigate', { url: APP_URL });
  await waitFor(cdp, `!!window.__waveInsightLsp && !!document.querySelector('.app')`, 60000);
  await cdp.eval(PAGE_HELPERS);
  await sleep(1000);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(900);
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(500);
  const active = await cdp.eval('window.__qa.texts(".modal .btn.active")');
  note('restored format', active);
  check('persistence: the remembered format is applied to the re-added row',
    !!active && active.indexOf('Bin') >= 0, active);
  check('persistence: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

// --- Wave 4: the row list of a dump is remembered ---------------------------
//
// boot() clears storage, so this starts from nothing: add two rows, reload
// without clearing, reopen the same file and the rows come back (in order, as
// real rows whose value column decodes) because the dump identity and its row
// list are persisted in localStorage.
scenarios.session = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  await cdp.eval(`window.__qa.clickSignal('pc')`);
  await sleep(700);
  await cdp.eval(`window.__qa.clickSignal('clk')`);
  await sleep(900);
  let s = await state(cdp);
  note('rows before the reload', s.waveNames);
  check('session: two rows are on the wave before the reload', s.waveNames.length === 2, s.waveNames);
  const stored = await cdp.eval('window.localStorage.getItem("wave-insight.sessions.v1")');
  note('stored session', stored && stored.slice(0, 260));
  check('session: the open row list is written for the dump',
    !!stored && stored.indexOf('wave.vcd') >= 0 && stored.indexOf('top.pc') >= 0,
    stored && stored.slice(0, 160));
  // reload without clearing storage, then reopen the same file: the rows return
  await cdp.send('Page.navigate', { url: APP_URL });
  await waitFor(cdp, `!!window.__waveInsightLsp && !!document.querySelector('.app')`, 60000);
  await cdp.eval(PAGE_HELPERS);
  await sleep(1000);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(900);
  s = await state(cdp);
  note('rows after reopening the dump', s.waveNames);
  check('session: reopening the same dump restores its rows in order',
    s.waveNames.length === 2 && s.waveNames[0].includes('pc') && s.waveNames[1].includes('clk'),
    s.waveNames);
  // the restored rows are real rows: the value column decodes at the cursor
  const surface = await cdp.eval(`window.__qa.rect('#wave-surface')`);
  await cdp.clickAt(surface.x + 120, surface.y + 15);
  await sleep(700);
  const values = await cdp.eval(`window.__qa.texts('.wave-name-cell .wave-value')`);
  note('restored value column', values);
  check('session: the restored rows show decoded values',
    values.length === 2 && values.filter((text) => !!text).length >= 1, values);
  check('session: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-session.png');
};

// --- Wave 3: accessibility, keyboard and modal focus -----------------------
scenarios.a11y = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await cdp.eval('window.__qa.clickSignal("clk")');
  await sleep(500);
  await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(900);
  const optionRows = await cdp.eval('window.__qa.qa(".wave-name-cell[role=option]").length');
  const listboxes = await cdp.eval('window.__qa.qa("[role=listbox]").length');
  note('listbox semantics', { options: optionRows, listboxes: listboxes });
  check('a11y: the wave rows are listbox options', optionRows >= 2 && listboxes >= 1, { options: optionRows, listboxes: listboxes });
  // the settings dialog is a labelled modal that takes focus
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(600);
  const dialog = await cdp.eval('(function(){ var d = document.querySelector(".modal"); if (!d) return null; return { role: d.getAttribute("role"), modal: d.getAttribute("aria-modal"), label: d.getAttribute("aria-label"), focusInside: d.contains(document.activeElement) }; })()');
  note('settings dialog', dialog);
  check('a11y: the settings dialog is a labelled modal',
    !!dialog && dialog.role === 'dialog' && dialog.modal === 'true' && !!dialog.label, dialog);
  check('a11y: focus moves into the dialog', !!dialog && dialog.focusInside === true, dialog && dialog.focusInside);
  const rowsBefore = (await state(cdp)).waveNames.length;
  await cdp.press('Escape', 'Escape', 27);
  await sleep(700);
  const afterEscape = await state(cdp);
  note('after Escape', { modal: afterEscape.modal, rows: afterEscape.waveNames.length });
  check('a11y: Escape closes the dialog', afterEscape.modal === null, afterEscape.modal);
  check('a11y: Escape keeps the row selection', afterEscape.waveNames.length === rowsBefore, { before: rowsBefore, after: afterEscape.waveNames.length });
  // arrow keys move the selection between rows
  await cdp.eval('window.__qa.click(".wave-name-text", 0)');
  await sleep(400);
  const first = await cdp.eval('Array.prototype.findIndex.call(document.querySelectorAll(".wave-name-cell"), function(e){ return e.getAttribute("aria-selected") === "true"; })');
  await cdp.press('ArrowDown', 'ArrowDown', 40);
  await sleep(600);
  const second = await cdp.eval('Array.prototype.findIndex.call(document.querySelectorAll(".wave-name-cell"), function(e){ return e.getAttribute("aria-selected") === "true"; })');
  note('selection', { first: first, second: second });
  check('a11y: the arrow keys move the row selection', first >= 0 && second >= 0 && second !== first, { first: first, second: second });
  // tab closers are real buttons with an accessible name
  await cdp.upload([path.join(FIXTURES, 'top.sv')]);
  await sleep(1500);
  const closer = await cdp.eval('(function(){ var b = document.querySelector(".tab .tab-close"); return b ? { tag: b.tagName, label: b.getAttribute("aria-label") } : null; })()');
  note('tab closer', closer);
  check('a11y: the tab closer is a labelled button', !!closer && closer.tag === 'BUTTON' && !!closer.label, closer);
  check('a11y: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-a11y.png');
};
// --- Wave 5: find a value in the selected row ------------------------------
// The ruler box accepts a time (integer) or a value (anything else): Enter
// finds the next change with that value, Shift+Enter the previous one.
scenarios.findValue = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(800);
  // find-value works on the *selected* row: a plain click on the wave name
  await cdp.eval('window.__qa.click(".wave-name-text", 0)');
  await sleep(500);
  // put cursor 1 at t=0 through the same box (the integer path) first
  await cdp.eval('(function(){ var i = document.querySelector(".timebar-goto"); var set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set; set.call(i, "0"); i.dispatchEvent(new Event("input", {bubbles: true})); i.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); })()');
  await sleep(700);
  const before = await cdp.eval('window.__qa.debug().cursor1');
  // type a value that exists later in wave.vcd (pc is 0x01 at t=10) and Enter
  const typed = await cdp.eval('(function(){ var i = document.querySelector(".timebar-goto"); if (!i) return "no-input"; var set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set; set.call(i, "\'h01"); i.dispatchEvent(new Event("input", {bubbles: true})); i.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); return "ok"; })()');
  await sleep(900);
  const after = await cdp.eval('window.__qa.debug().cursor1');
  let s = await state(cdp);
  note('find value', { before: before, after: after, status: s.status });
  check('findValue: the ruler box accepts a value', typed === 'ok', typed);
  check('findValue: the cursor jumps to the next change with that value',
    before === 0 && after === 10, { before: before, after: after });
  check('findValue: the status line reports the match', /found/i.test(s.status || ''), s.status);
  // Wave 18 (task-1/find.rs): the wave view prints bare digits, so typing what
  // the value column shows must find the same change; the prefixed query above
  // keeps working because find strips an optional 'h/'d/'sd/'o/'b prefix. The
  // query must not be integer-looking (the ruler box reads a plain integer as a
  // time), so the probe uses pc's bare hex '2a' - the value at t=420.
  const goto = (text) => cdp.eval('(function(){ var i = document.querySelector(".timebar-goto"); if (!i) return "no-input"; var set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set; set.call(i, ' + JSON.stringify(String(text)) + '); i.dispatchEvent(new Event("input", {bubbles: true})); i.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); return "ok"; })()');
  await goto(0);
  await sleep(700);
  const bareStart = await cdp.eval('window.__qa.debug().cursor1');
  const bareTyped = await goto('2a');
  await sleep(900);
  const bareFound = await cdp.eval('window.__qa.debug().cursor1');
  const bareShown = await cdp.eval('window.__qa.text(".wave-name-cell .wave-value")');
  s = await state(cdp);
  note('find value (bare digits)', { typed: bareTyped, start: bareStart, found: bareFound, shown: bareShown, status: s.status });
  check('findValue: the bare digits the value view shows find the same change', bareStart === 0 && bareFound === 420, { start: bareStart, found: bareFound });
  check('findValue: the bare-digit query is what the value column displays', bareFound === 420 && bareShown === '2a', { shown: bareShown, found: bareFound });
  check('findValue: the bare-digit query reports the match', /found/i.test(s.status || ''), s.status);
  // an unknown value reports no match and does not move the cursor
  const beforeMiss = await cdp.eval('window.__qa.debug().cursor1');
  await cdp.eval('(function(){ var i = document.querySelector(".timebar-goto"); var set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set; set.call(i, "no_such_value"); i.dispatchEvent(new Event("input", {bubbles: true})); i.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", bubbles: true})); })()');
  await sleep(700);
  const missed = await cdp.eval('window.__qa.debug().cursor1');
  s = await state(cdp);
  const tone = await cdp.eval('window.__qa.statusTone()');
  note('find miss', { cursor: missed, status: s.status, tone: tone });
  check('findValue: an unknown value reports no match', missed === beforeMiss, { before: beforeMiss, missed: missed });
  check('findValue: the miss is an error status', /no match/i.test(s.status || '') && tone === 'error', { status: s.status, tone: tone });
  check('findValue: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

// --- Wave 18 (task-3): reload a dump re-written with the same name and size --
// DumpId is file name + byte size, so the normal "re-simulate, then open the
// dump again" loop used to focus the existing tab and keep showing the OLD
// waveform. A content fingerprint now reloads that tab in place: the tab, the
// row list, the viewport and the cursors stay while the waveform, ruler and
// cursor values become the new file's, and the status line says "waveform
// reloaded". The pair of dumps is generated here from qa/fixtures/wave.vcd
// (qa/artifacts is gitignored, so the pair is not a committed fixture): A is a
// verbatim copy, B rewrites every pc value to 8'b10101010 while keeping every
// line length, so the two files share the name and the byte size by
// construction and only their bytes differ.
scenarios.reload = async (cdp) => {
  const dir = path.join(QA, 'artifacts', 'reload-qa');
  fs.mkdirSync(dir, { recursive: true });
  const fixture = fs.readFileSync(path.join(FIXTURES, 'wave.vcd'), 'utf8');
  const lines = fixture.split('\n');
  const rewritten = lines.map((line) => (line.length === 11 && line.charCodeAt(0) === 98
    && line.charAt(9) === ' ' && line.charAt(10) === '$' ? 'b10101010 $' : line));
  const changedLines = lines.filter((line, index) => rewritten[index] !== line).length;
  const aFile = path.join(dir, 'wave-A.vcd');
  const bFile = path.join(dir, 'wave-B.vcd');
  fs.writeFileSync(aFile, fixture);
  fs.writeFileSync(bFile, rewritten.join('\n'));
  const live = path.join(dir, 'wave.vcd');
  const aBytes = fs.statSync(aFile).size;
  const bBytes = fs.statSync(bFile).size;
  note('reload fixtures', { a: aBytes, b: bBytes, changedLines: changedLines });
  check('reload: the changed dump has the same name and byte size', aBytes === bBytes && aBytes > 0 && changedLines > 0, { a: aBytes, b: bBytes, changedLines: changedLines });
  if (aBytes !== bBytes || changedLines === 0) return;
  fs.copyFileSync(aFile, live);
  await boot(cdp);
  await cdp.upload([live]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(800);
  const addedPc = await addSignal(cdp, 'pc');
  const addedClk = await addSignal(cdp, 'clk');
  const surface = await cdp.eval('window.__qa.rect("#wave-surface")');
  await cdp.clickAt(surface.x + 300, surface.y + 15);
  await sleep(800);
  const before = await cdp.eval('window.__qa.dumpSnapshot()');
  note('reload before', before);
  check('reload: file A is open with rows and cursor values', addedPc && addedClk && before.tabs.length === 1 && before.rows.length === 2 && before.values.length >= 2, { addedPc: addedPc, addedClk: addedClk, before: before });
  // overwrite the SAME path with B and pick it again: same name, same byte
  // size, different content -> that tab must reload in place
  fs.copyFileSync(bFile, live);
  await cdp.upload([live]);
  await waitStatus(cdp, 'waveform reloaded', 25000);
  await sleep(1200);
  const after = await cdp.eval('window.__qa.dumpSnapshot()');
  note('reload after', after);
  check('reload: re-opening a changed dump does not add a tab', after.tabs.length === 1 && after.tabs[0] === before.tabs[0], { before: before.tabs, after: after.tabs });
  check('reload: the row list survives the reload', JSON.stringify(after.rows) === JSON.stringify(before.rows) && after.rows.length === 2, { before: before.rows, after: after.rows });
  check('reload: the canvas repaints with the new content', after.hash !== before.hash, { before: before.hash, after: after.hash });
  check('reload: the cursor value column shows the new file values', after.values.indexOf('aa') >= 0 && before.values.indexOf('aa') < 0, { before: before.values, after: after.values });
  check('reload: the cursor survives the reload', typeof after.cursor1 === 'number' && after.cursor1 === before.cursor1, { before: before.cursor1, after: after.cursor1 });
  check('reload: the status line says reloaded', /^waveform reloaded/.test(after.status || ''), after.status);
  // the unchanged-file contract: picking the same bytes again focuses the tab,
  // says "loaded" and does not repaint the waveform
  await cdp.upload([live]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(1000);
  const same = await cdp.eval('window.__qa.dumpSnapshot()');
  note('reload unchanged', same);
  check('reload: an unchanged re-open keeps the loaded status', /^waveform loaded/.test(same.status || ''), same.status);
  check('reload: an unchanged re-open adds no tab and does not repaint', same.tabs.length === 1 && same.hash === after.hash, { tabs: same.tabs, hash: [after.hash, same.hash] });
  check('reload: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-reload.png');
};

// --- Wave 18 (task-1): bare band labels + ellipsis truncation -----------------
// The wave view prints bare digits (no Verilog radix prefix) and a band label
// that does not fit is shortened with a single ellipsis instead of being
// dropped. The canvas publishes what it drew on window.__waveDebug (additive
// seam: labels / labels_drawn / labels_truncated / labels_hidden).
scenarios.rendererLabels = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await addSignal(cdp, 'pc');
  await sleep(900);
  const canvas = await cdp.eval('window.__qa.rect(".wave-canvas")');
  await cdp.clickAt(canvas.x + 200, canvas.y + 20);
  await sleep(700);
  const values = await cdp.eval('window.__qa.texts(".wave-name-cell .wave-value")');
  check('rendererLabels: the value column prints bare digits',
    values.length > 0 && values.every((text) => text && text.length > 0 && text.charCodeAt(0) !== 39), values);
  // Bin gives the longest labels: at this zoom the old renderer dropped the
  // ones that did not fit, the new one truncates them with an ellipsis.
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(500);
  const bin = await cdp.eval('window.__qa.clickText(".modal .btn", "Bin")');
  await sleep(600);
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  for (let i = 0; i < 9; i++) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 200, y: canvas.y + 20, deltaX: 0, deltaY: -120, modifiers: 2 });
    await sleep(220);
  }
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(900);
  const info = await cdp.eval('window.__qa.bandLabels()');
  const ellipsis = String.fromCharCode(8230);
  const truncatedLabels = info.labels.filter((text) => text.indexOf(ellipsis) >= 0);
  note('band labels', { bin: bin, drawn: info.drawn, truncated: info.truncated, hidden: info.hidden, bands: info.bands, sample: info.labels.slice(0, 12) });
  check('rendererLabels: the seam publishes the drawn band labels',
    bin === 'ok' && Array.isArray(info.labels) && info.labels.length > 0, { bin: bin, labels: info.labels.slice(0, 8) });
  check('rendererLabels: no band label carries a radix prefix',
    info.labels.every((text) => text.length > 0 && text.charCodeAt(0) !== 39),
    info.labels.filter((text) => text.charCodeAt(0) === 39).slice(0, 6));
  check('rendererLabels: the drawn counter is at least the published array',
    typeof info.drawn === 'number' && info.drawn >= info.labels.length, { drawn: info.drawn, published: info.labels.length });
  check('rendererLabels: narrow bands are ellipsized, not dropped',
    info.truncated > 0 && info.labels.length === Math.min(256, info.drawn), { truncated: info.truncated, hidden: info.hidden, drawn: info.drawn, published: info.labels.length });
  check('rendererLabels: every ellipsized label has exactly one ellipsis',
    truncatedLabels.length > 0 && truncatedLabels.every((text) => text.split(ellipsis).length === 2), truncatedLabels.slice(0, 6));
  const binValues = await cdp.eval('window.__qa.texts(".wave-name-cell .wave-value")');
  check('rendererLabels: the value column goes on printing bare digits',
    binValues.length > 0 && binValues.every((text) => text && text.length > 0 && text.charCodeAt(0) !== 39), binValues);
  check('rendererLabels: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-renderer-labels.png');
};

// --- Wave 18 (task-1): 1-bit level vs cursor alignment -----------------------
// A cursor placed exactly on a clk transition must sit centred on the edge: the
// cursor line and every value-change x are painted on the same crisp device
// column now, so the blue edge ink is symmetric around the cursor column. The
// old renderer painted the edge at the raw x, so at this zoom (the change is
// 0.25 px off the 0.5 px cursor column) the visible ink was one-sided to the
// left. The probe finds the cursor column by its colour and measures the middle
// band of the clk row, where only a vertical edge has ink.
scenarios.rendererAlign = async (cdp) => {
  await boot(cdp);
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await sleep(400);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  await cdp.eval('window.__qa.clickSignal("clk")');
  await sleep(600);
  await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(900);
  const canvas = await cdp.eval('window.__qa.rect(".wave-canvas")');
  await cdp.key('keyDown', 'Control', 'ControlLeft', 17);
  for (let i = 0; i < 8; i++) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: canvas.x + 300, y: canvas.y + 20, deltaX: 0, deltaY: -120, modifiers: 2 });
    await sleep(200);
  }
  await cdp.key('keyUp', 'Control', 'ControlLeft', 17);
  await sleep(700);
  // t=575 is a clk edge in wave.vcd (clk falls 1 -> 0 there); the goto box puts
  // cursor 1 exactly on it and the probe derives the pixel phase from the
  // renderer seam instead of assuming it.
  await cdp.eval('(function(){ var i = document.querySelector(".timebar-goto"); if (!i) return "no-input"; var set = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value").set; set.call(i, "575"); i.dispatchEvent(new Event("input", { bubbles: true })); i.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", code: "Enter", keyCode: 13, which: 13, bubbles: true })); return "ok"; })()');
  await sleep(900);
  const probe = await cdp.eval('window.__qa.alignProbe()');
  note('align probe', probe);
  check('align: the goto box puts cursor 1 exactly on the clk edge (575)',
    !!probe && probe.cursor1 === 575, probe && probe.cursor1);
  check('align: the probe runs at devicePixelRatio 1',
    !!probe && Math.abs(probe.dpr - 1) <= 1e-6, probe && probe.dpr);
  check('align: the cursor column is found by the cursor colour',
    !!probe && probe.cursorCol >= 0 && probe.cursorCols === 1, probe && { cursorCol: probe.cursorCol, cursorCols: probe.cursorCols });
  check('align: the case is discriminating (the raw edge is off the cursor column)',
    !!probe && Math.abs(probe.xfrac - 0.5) > 0.1, probe && probe.xfrac);
  check('align: the cursor sits on the crisp column of the change',
    !!probe && Math.abs(probe.crisp - (probe.cursorCol + 0.5)) < 1e-6, probe && { crisp: probe.crisp, cursorCol: probe.cursorCol });
  const massAt = (dx) => { const entry = probe && probe.masses.find((pair) => pair[0] === dx); return entry ? entry[1] : 0; };
  const left = massAt(-1);
  const right = massAt(1);
  check('align: there is 1-bit edge ink on both sides of the cursor', left > 0.5 && right > 0.5, { left: left, right: right, masses: probe && probe.masses });
  check('align: the 1-bit edge ink is symmetric around the cursor column',
    Math.abs(left - right) <= 0.2 * Math.max(left, right, 1),
    { left: left, right: right, balance: +(Math.min(left, right) / Math.max(left, right, 1)).toFixed(3) });
  check('align: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-renderer-align.png');
};

// --- Wave 6: elaboration in the UI, elaboration diagnostics, TXT export -----
// The elaborated design (no dump open) drives the sidebar module tree. The tree
// rows are already expanded for a design hierarchy, so no clicking is needed;
// the seam carries the logical names and the row `title` attributes carry the
// full elaborated paths.
scenarios.elaboration = async (cdp) => {
  const tree = async () => {
    const seam = await cdp.eval('window.__qa.sidebar()');
    const titles = await cdp.eval('window.__qa.qa(".tree-row").map(function(r){ return r.getAttribute("title"); })');
    const s = await state(cdp);
    return { seam: seam, titles: titles, modules: s.modules, tabs: s.tabs };
  };
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'generate.sv')]);
  await waitTabs(cdp, 1);
  await sleep(2500);
  let t = await tree();
  note('generate tree', t);
  check('elaboration: a source alone opens no dump', t.tabs.length === 1, t.tabs);
  check('elaboration: the generated hierarchy is in the sidebar',
    t.titles.includes('gen_top.g[0].u_leaf') && t.titles.includes('gen_top.g[3].u_leaf'), t.titles);
  check('elaboration: every generate-for element is a row',
    ['gen_top.g[0].u_leaf', 'gen_top.g[1].u_leaf', 'gen_top.g[2].u_leaf', 'gen_top.g[3].u_leaf'].every((path) => t.titles.includes(path)), t.titles);
  check('elaboration: the seam reports the four leaf rows',
    !!t.seam && t.seam.modules.length === 4 && t.seam.modules.every((name) => name === 'u_leaf'), t.seam && t.seam.modules);
  check('elaboration: the seam counts the generate scopes too',
    !!t.seam && t.seam.modules_total === 5, t.seam && t.seam.modules_total);
  // the other fixture is a different top: start from a clean page so the
  // scenario does not depend on which root the elaborator picks first
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'arrayed.sv')]);
  await waitTabs(cdp, 1);
  await sleep(2500);
  t = await tree();
  note('arrayed tree', t);
  check('elaboration: arrayed instances expand into element rows',
    t.titles.includes('arrayed_top.u[0]') && t.titles.includes('arrayed_top.u[1]') && t.titles.includes('arrayed_top.v[3]'), t.titles);
  check('elaboration: both arrays contribute their elements',
    t.titles.filter((path) => /^arrayed_top\.u\[\d\]$/.test(path)).length === 2
      && t.titles.filter((path) => /^arrayed_top\.v\[\d\]$/.test(path)).length === 4, t.titles);
  check('elaboration: the seam counts the top plus six elements',
    !!t.seam && t.seam.modules_total === 7, t.seam && t.seam.modules_total);
  check('elaboration: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-elaboration.png');
};

// --- Wave 6: elaboration diagnostics reach the editor and the top bar -------
// link_errors.sv has unresolved names in link_top, which the elaborator picks
// as the top when no dump is open. (The port-width mismatch in width_top is
// not surfaced: only the picked top is elaborated - documented in README.)
scenarios.diagnostics = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'link_errors.sv')]);
  await waitTabs(cdp, 1);
  await sleep(3000);
  const raw = await cdp.eval('window.__waveInsightLsp && window.__waveInsightLsp.diagnostics ? window.__waveInsightLsp.diagnostics() : null');
  const diagnostics = raw ? JSON.parse(raw) : null;
  note('elaboration diagnostics', diagnostics);
  const files = diagnostics && diagnostics.files ? Object.keys(diagnostics.files) : [];
  const entry = files.length ? diagnostics.files[files[0]] : null;
  const items = entry && Array.isArray(entry.items) ? entry.items : [];
  check('diagnostics: the elaborator reports unresolved names',
    !!entry && entry.count >= 2 && items.filter((item) => item.code === 'unresolved-name').length >= 2, entry);
  check('diagnostics: both missing names are reported',
    items.some((item) => /missing_load/.test(item.message || '')) && items.some((item) => /missing_driver/.test(item.message || '')), items.map((item) => item.message));
  check('diagnostics: the diagnostics point at the source lines',
    items.every((item) => typeof item.line === 'number' && item.line >= 0), items.map((item) => item.line));
  const stats = await cdp.eval('window.__waveInsightLsp ? window.__waveInsightLsp.stats() : null');
  const statsObj = stats ? JSON.parse(stats) : null;
  note('lsp stats', statsObj);
  check('diagnostics: the server elaborated link_top as the top',
    !!statsObj && statsObj.top === 'link_top', statsObj && statsObj.top);
  const markers = await cdp.eval('window.monaco && window.monaco.editor ? window.monaco.editor.getModelMarkers({}).length : -1');
  note('monaco markers', markers);
  check('diagnostics: the editor receives markers for the elaborated file', markers >= 2, markers);
  const chip = await cdp.eval('window.__qa.text(".problems-tag")');
  note('problems chip', chip);
  check('diagnostics: the top bar shows the problem count', /^[1-9]\d* problem/.test(chip || ''), chip);
  check('diagnostics: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-diagnostics.png');
};

// --- Wave 6: TXT export is <time> <value> lines, no header -------------------
scenarios.exportTxt = async (cdp) => {
  await boot(cdp);
  const downloads = path.join(QA, 'artifacts', 'downloads');
  fs.rmSync(downloads, { recursive: true, force: true });
  fs.mkdirSync(downloads, { recursive: true });
  await cdp.send('Page.setDownloadBehavior', { behavior: 'allow', downloadPath: downloads });
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  const added = await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(800);
  check('exportTxt: a row is on the wave', added === 'ok', added);
  const waitForFile = async (ext) => {
    for (let i = 0; i < 50; i++) {
      await sleep(300);
      const names = fs.existsSync(downloads) ? fs.readdirSync(downloads).filter((n) => n.endsWith(ext)) : [];
      for (const name of names) {
        const full = path.join(downloads, name);
        try { const stat = fs.statSync(full); if (stat.size > 0) return { name: name, text: fs.readFileSync(full, 'utf8') }; } catch (error) { /* still downloading */ }
      }
    }
    return null;
  };
  // the CSV is the reference: same row, same values, one header line
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(500);
  const csvClicked = await cdp.eval('window.__qa.clickText(".modal .btn", "CSV (row)")');
  const csv = await waitForFile('.csv');
  note('csv export', csv && { name: csv.name, bytes: csv.text.length });
  check('exportTxt: the csv export produced a file', csvClicked === 'ok' && !!csv, csv && csv.name);
  const csvLines = csv ? csv.text.split('\n').filter((line) => line.length > 0) : [];
  check('exportTxt: the csv has the header and data', csvLines.length > 3 && csvLines[0] === 'time,value', csvLines.slice(0, 2));
  // the TXT export of the same row
  await cdp.eval('window.__qa.click(".wave-more", 1)');
  await sleep(500);
  const txtClicked = await cdp.eval('window.__qa.clickText(".modal .btn", "TXT (row)")');
  const txt = await waitForFile('.txt');
  note('txt export', txt && { name: txt.name, bytes: txt.text.length, first: txt.text.split('\n').slice(0, 3) });
  check('exportTxt: the settings dialog offers a TXT export', txtClicked === 'ok', txtClicked);
  check('exportTxt: the export produced a txt file', !!txt, txt && txt.name);
  const txtLines = txt ? txt.text.split('\n').filter((line) => line.length > 0) : [];
  check('exportTxt: the txt has no header', txtLines.length > 3 && /^\d+ \S+$/.test(txtLines[0]), txtLines.slice(0, 2));
  check('exportTxt: the txt has the same number of lines as the csv data',
    !!csv && txtLines.length === csvLines.length - 1, { txt: txtLines.length, csv: csvLines.length });
  const csvData = csvLines.slice(1).map((line) => { const comma = line.indexOf(','); return [line.slice(0, comma), line.slice(comma + 1)]; });
  const txtData = txtLines.map((line) => { const space = line.indexOf(' '); return [line.slice(0, space), line.slice(space + 1)]; });
  check('exportTxt: every line is <time> <value> with the csv values',
    csvData.length === txtData.length && csvData.every((pair, i) => pair[0] === txtData[i][0] && pair[1] === txtData[i][1]),
    { csv: csvData.slice(0, 3), txt: txtData.slice(0, 3) });
  const s = await state(cdp);
  note('export status', s.status);
  check('exportTxt: the export reports success in the status line', /export/i.test(s.status || ''), s.status);
  check('exportTxt: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-export-txt.png');
};

// --- Wave 8: corrupt dumps fail honestly, and the app keeps working ---------
// Wave 7 hardened the parsers: a corrupt FST used to abort the process with a
// multi-terabyte allocation. This is the browser-level regression proof: each
// corrupt file must produce a readable error (naming the file and what to do),
// no page exception, and the app must still open a valid dump afterwards.
scenarios.corruption = async (cdp) => {
  await boot(cdp);
  const cases = [
    { file: 'truncated.fst', what: 'a truncated FST' },
    { file: 'flipped.fst', what: 'an FST with a flipped length field' },
    { file: 'truncated.vcd', what: 'a truncated VCD' },
  ];
  for (const item of cases) {
    await cdp.upload([path.join(FIXTURES, item.file)]);
    // the failed open is reported once the reader gives up; the size guard and
    // the parse happen before the error status lands
    await sleep(2500);
    const s = await state(cdp);
    const tone = await cdp.eval('window.__qa.statusTone()');
    const errs = await exceptions(cdp);
    note('corrupt ' + item.file, { status: s.status, tone: tone, exceptions: errs });
    check('corruption: ' + item.what + ' is refused, not crashed', errs.length === 0, errs);
    check('corruption: ' + item.what + ' names the file in the error',
      (s.status || '').includes(item.file), s.status);
    check('corruption: ' + item.what + ' says it could not be opened',
      /could not be opened/.test(s.status || ''), s.status);
    check('corruption: ' + item.what + ' tells the user what to do',
      /intact VCD\/FST\/GHW dump/.test(s.status || ''), s.status);
    check('corruption: ' + item.what + ' is an error status, not a note', tone === 'error', tone);
  }
  // a VCD that declares the same id twice with conflicting widths used to trap
  // the wasm parser; the structural validation must turn it into the actionable
  // refusal instead of a panic (fixture: the 298 B fe-auditor repro, committed
  // as qa/fixtures/min_width_mismatch.vcd)
  await cdp.upload([path.join(FIXTURES, 'min_width_mismatch.vcd')]);
  await sleep(2500);
  const mismatch = await state(cdp);
  const mismatchTone = await cdp.eval('window.__qa.statusTone()');
  const mismatchExceptions = await exceptions(cdp);
  note('width mismatch', { status: mismatch.status, tone: mismatchTone, exceptions: mismatchExceptions });
  check('corruption: a conflicting-width VCD is refused, not a wasm panic',
    mismatchExceptions.length === 0, mismatchExceptions);
  check('corruption: the conflicting-width VCD is named in an error status',
    mismatchTone === 'error' && (mismatch.status || '').includes('min_width_mismatch.vcd'),
    { status: mismatch.status, tone: mismatchTone });
  check('corruption: the conflicting-width VCD error is actionable',
    /could not be opened|malformed|invalid/i.test(mismatch.status || '')
      && /intact VCD\/FST\/GHW dump|malformed|re-export/i.test(mismatch.status || ''),
    mismatch.status);
  // the app must still be usable: the corrupt attempts left no broken state
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  const loaded = await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(700);
  const added = await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(800);
  const s = await state(cdp);
  note('after the corrupt dumps', { status: s.status, rows: s.waveNames, added: added });
  check('corruption: a valid dump still loads after the corrupt ones', !!loaded, s.status);
  check('corruption: a row can still be added to the wave', added === 'ok' && (s.waveNames || []).length > 0, s.waveNames);
  check('corruption: no exceptions during the recovery', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-corruption.png');
};

// --- Wave 8: several dumps at once, one tab strip ---------------------------
// The strip only exists with two or more dumps. Every assertion is DOM/state
// based (class names, aria attributes, text, __qa.state().waveNames), never
// style based, so the CSS can change without breaking the scenario.
scenarios.multiDump = async (cdp) => {
  const strip = () => cdp.eval(`(function(){
    const box = document.querySelector('.dump-tabs');
    const names = Array.prototype.map.call(document.querySelectorAll('.dump-tab-name'), function(b){ return (b.innerText || '').trim(); });
    const selected = Array.prototype.map.call(document.querySelectorAll('.dump-tab-name'), function(b){ return b.getAttribute('aria-selected'); });
    const active = document.querySelector('.dump-tab.active .dump-tab-name');
    const closes = Array.prototype.map.call(document.querySelectorAll('.dump-tab-close'), function(b){ return b.getAttribute('aria-label'); });
    return {
      present: !!box,
      count: document.querySelectorAll('.dump-tab').length,
      names: names,
      selected: selected,
      active: active ? (active.innerText || '').trim() : null,
      title: box ? box.getAttribute('title') : null,
      role: box ? box.getAttribute('role') : null,
      ariaLabel: box ? box.getAttribute('aria-label') : null,
      closes: closes,
    };
  })()`);
  const rows = async () => (await state(cdp)).waveNames;
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(600);
  let t = await strip();
  note('after the first dump', t);
  check('multiDump: one dump shows a single-tab strip', t.present && t.count === 1, t);
  check('multiDump: the single tab is the open dump and is active',
    JSON.stringify(t.names) === JSON.stringify(['wave.vcd']) && t.active === 'wave.vcd', t);
  check('multiDump: the single tab is selected for screen readers',
    JSON.stringify(t.selected) === JSON.stringify(['true']), t.selected);
  check('multiDump: the single-tab strip title counts the dump',
    /^1 dumps open, \d+ MB of dump files$/.test(t.title || ''), t.title);
  check('multiDump: the single tab has a labelled close button',
    JSON.stringify(t.closes) === JSON.stringify(['Close wave.vcd']), t.closes);
  await cdp.upload([path.join(FIXTURES, 'mismatch.vcd')]);
  await waitStatus(cdp, '2 dumps open', 25000);
  await sleep(700);
  t = await strip();
  note('after the second dump', t);
  check('multiDump: the strip appears with two dumps', t.present && t.count === 2, t);
  check('multiDump: the strip is a tablist labelled for screen readers',
    t.role === 'tablist' && t.ariaLabel === 'Open dumps', { role: t.role, label: t.ariaLabel });
  check('multiDump: the tabs are the two file names',
    JSON.stringify(t.names) === JSON.stringify(['wave.vcd', 'mismatch.vcd']), t.names);
  check('multiDump: the new dump is the active one', t.active === 'mismatch.vcd', t.active);
  check('multiDump: aria-selected follows the active tab',
    JSON.stringify(t.selected) === JSON.stringify(['false', 'true']), t.selected);
  check('multiDump: the strip title reports the dump count and size',
    /^2 dumps open, \d+ MB of dump files$/.test(t.title || ''), t.title);
  check('multiDump: the close buttons are labelled with the file names',
    JSON.stringify(t.closes) === JSON.stringify(['Close wave.vcd', 'Close mismatch.vcd']), t.closes);
  check('multiDump: the second dump starts with its own empty row list', (await rows()).length === 0, await rows());
  await cdp.shot('qa-multidump.png');
  // one row per dump: the row lists are per dump, not shared
  const addedSecond = await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(700);
  const secondRows = await rows();
  note('second dump rows', { added: addedSecond, rows: secondRows });
  check('multiDump: a row can be added to the second dump',
    addedSecond === 'ok' && secondRows.length === 1 && /pc/.test(secondRows[0]), secondRows);
  await cdp.eval('window.__qa.clickText(".dump-tab-name", "wave.vcd")');
  await sleep(700);
  t = await strip();
  const firstRowsBefore = await rows();
  note('after switching to wave.vcd', { active: t.active, rows: firstRowsBefore });
  check('multiDump: clicking a tab activates it', t.active === 'wave.vcd', t.active);
  check('multiDump: the row list follows the active dump (first is empty)',
    firstRowsBefore.length === 0, firstRowsBefore);
  const addedFirst = await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(700);
  const firstRows = await rows();
  check('multiDump: a row can be added to the first dump',
    addedFirst === 'ok' && firstRows.length === 1 && /pc/.test(firstRows[0]), firstRows);
  await cdp.eval('window.__qa.clickText(".dump-tab-name", "mismatch.vcd")');
  await sleep(700);
  const secondRowsAgain = await rows();
  note('back on the second dump', secondRowsAgain);
  check('multiDump: switching back restores the other dump rows',
    secondRowsAgain.length === 1 && /pc/.test(secondRowsAgain[0]), secondRowsAgain);
  check('multiDump: the two dumps keep different row paths',
    firstRows[0] !== secondRowsAgain[0], { first: firstRows[0], second: secondRowsAgain[0] });
  // re-opening the same file focuses the existing tab instead of duplicating it
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await sleep(3000);
  t = await strip();
  const focusedRows = await rows();
  note('after re-opening wave.vcd', { tabs: t, rows: focusedRows });
  check('multiDump: re-opening the same file does not add a tab', t.count === 2, t.count);
  check('multiDump: re-opening focuses the existing tab', t.active === 'wave.vcd', t.active);
  check('multiDump: the focused dump kept its rows',
    focusedRows.length === 1 && focusedRows[0] === firstRows[0], focusedRows);
  // closing the active tab hands over to the neighbour with its own rows.
  // NOTE: the strip only renders with >= 2 dumps (app.rs:517) and both close
  // paths live in the strip (close button + Ctrl+W keydown), so closing the
  // *last* dump is not reachable from the UI today; that gap is reported to
  // the owner instead of being papered over here.
  await cdp.eval('window.__qa.click(".dump-tab.active .dump-tab-close", 0)');
  await sleep(800);
  t = await strip();
  const afterClose = await rows();
  const statusAfterClose = (await state(cdp)).status;
  note('after closing the active tab', { tabs: t, rows: afterClose, status: statusAfterClose });
  check('multiDump: the strip stays with one dump left', t.present && t.count === 1, t);
  check('multiDump: the neighbour becomes the only active tab',
    t.active === 'mismatch.vcd' && JSON.stringify(t.selected) === JSON.stringify(['true']), t);
  check('multiDump: the remaining dump keeps its rows',
    afterClose.length === 1 && afterClose[0] === secondRowsAgain[0], afterClose);
  // the last dump can be closed: the strip disappears and the empty state returns
  await cdp.eval('window.__qa.click(".dump-tab-close", 0)');
  await sleep(900);
  t = await strip();
  const hint = await cdp.eval('window.__qa.text(".start-hint")');
  const emptyRows = await rows();
  note('after closing the last dump', { tabs: t, hint: hint, rows: emptyRows });
  check('multiDump: closing the last dump removes the strip', !t.present, t);
  check('multiDump: the start hint is back with no dump open',
    !!hint && /Nothing loaded yet|Drop a waveform/i.test(hint), hint);
  check('multiDump: the empty state has no rows', emptyRows.length === 0, emptyRows);
  // and the app is usable again: reopening brings the strip and the remembered rows back
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 25000);
  await sleep(900);
  t = await strip();
  const reopened = await rows();
  note('after re-opening wave.vcd', { tabs: t, rows: reopened });
  check('multiDump: re-opening after the empty state brings the single-tab strip back',
    t.present && t.count === 1 && t.active === 'wave.vcd', t);
  check('multiDump: the remembered rows come back with the reopened dump',
    JSON.stringify(reopened) === JSON.stringify(['top.pc[7:0]']), reopened);
  // Ctrl+W with the focus inside the strip closes the active dump (keyboard path)
  const sent = await cdp.eval(`(function(){
    const box = document.querySelector('.dump-tabs');
    if (!box) return 'no-strip';
    box.dispatchEvent(new KeyboardEvent('keydown', { key: 'w', ctrlKey: true, bubbles: true, cancelable: true }));
    return 'sent';
  })()`);
  await sleep(900);
  t = await strip();
  const hintAfterKeys = await cdp.eval('window.__qa.text(".start-hint")');
  note('after Ctrl+W on the strip', { sent: sent, tabs: t, hint: hintAfterKeys });
  check('multiDump: Ctrl+W inside the strip closes the active dump', sent === 'sent' && !t.present, t);
  check('multiDump: the empty state is back after Ctrl+W', !!hintAfterKeys, hintAfterKeys);
  check('multiDump: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
};

// --- Wave 9: driver bit ranges in the editor hover --------------------------
// Wave 8 made the per-bit driver ranges visible in the LSP hover
// ("- driven here: q[3:0], q[7:4]"). This is the browser-level proof that the
// editor receives it: bit_ranges.sv has two disjoint slices driving `q`, and
// `y` is driven as a whole signal, so it must not show a driven-here line.
// No dump is needed: without one the elaborator picks `bit_ranges` as the top
// (asserted below, so a change in that pick is visible instead of silent).
scenarios.bitRanges = async (cdp) => {
  const hover = async (line, column) => {
    const raw = await cdp.eval('window.__qa.provider("hover", "bit_ranges.sv", ' + line + ', ' + column + ')');
    if (!raw || raw === 'null' || raw === 'no-hook' || String(raw).startsWith('no-model')) return { raw: raw };
    try {
      const parsed = JSON.parse(raw);
      const contents = parsed && parsed.contents ? parsed.contents : [];
      const value = contents.map((item) => item.value || '').join('\n');
      return { value: value, range: parsed.range };
    } catch (error) {
      return { raw: String(raw) };
    }
  };
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'bit_ranges.sv')]);
  await waitTabs(cdp, 1);
  await sleep(3000);
  const stats = await cdp.eval('window.__waveInsightLsp ? window.__waveInsightLsp.stats() : null');
  const statsObj = stats ? JSON.parse(stats) : null;
  note('lsp stats', statsObj);
  check('bitRanges: the elaborator picks bit_ranges as the top without a dump',
    !!statsObj && statsObj.top === 'bit_ranges', statsObj && statsObj.top);
  // `q` is driven in two disjoint slices: the hover on its usage must list both
  const usage = await hover(14, 12);
  note('hover on q[3:0]', usage);
  check('bitRanges: the hover on a driven slice names the elaborated signal',
    !!usage.value && usage.value.includes('bit_ranges.q'), usage.value);
  check('bitRanges: the hover lists the driver bit ranges',
    !!usage.value && usage.value.includes('driven here') && usage.value.includes('[3:0]') && usage.value.includes('[7:4]'), usage.value);
  check('bitRanges: the hover counts both drivers',
    !!usage.value && /drivers: 2/.test(usage.value), usage.value);
  // the declaration position reports the same ranges (the hover is position-based,
  // not model-based)
  const declaration = await hover(11, 25);
  note('hover on the q declaration', declaration);
  check('bitRanges: the declaration hover carries the same ranges',
    !!declaration.value && declaration.value.includes('driven here') && declaration.value.includes('[3:0]') && declaration.value.includes('[7:4]'), declaration.value);
  // `y` is driven as a whole signal: no bit-range line must appear
  const whole = await hover(16, 12);
  note('hover on y', whole);
  check('bitRanges: a whole-signal hover has no driven-here line',
    !!whole.value && whole.value.includes('bit_ranges.y') && !whole.value.includes('driven here'), whole.value);
  check('bitRanges: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-bit-ranges.png');
};

// --- Wave 10: Monaco completion in the editor ---------------------------------
// The completion provider is the 6th registered provider; the __qa.provider
// helper's `completion` branch calls its 4-argument form (monaco passes the
// trigger context). The server returns its whole visible set and filters by the
// text before the caret: plain names give the enclosing module's ports/signals/
// parameters/genvars/instances plus every module name, `instance.` gives that
// module's members only, an unknown qualifier and a position before the first
// module give an empty list.
scenarios.completion = async (cdp) => {
  const complete = async (file, line, column) => {
    const raw = await cdp.eval('window.__qa.provider("completion", ' + JSON.stringify(file) + ', ' + line + ', ' + column + ')');
    if (!raw || raw === 'no-hook' || String(raw).startsWith('no-model')) return { error: raw };
    try {
      const parsed = JSON.parse(raw);
      return { suggestions: parsed && parsed.suggestions ? parsed.suggestions : [] };
    } catch (error) {
      return { error: String(raw) };
    }
  };
  const labels = (result) => (result.suggestions || []).map((item) => item.label);
  const kindOf = (result, label) => {
    const item = (result.suggestions || []).find((suggestion) => suggestion.label === label);
    return item ? item.kind : null;
  };
  // append a comment line to top.sv through monaco; the editor's change event
  // syncs the buffer to the store and the LSP, which is what the server reads
  const appendLine = async (text) => cdp.eval(`(function(){
    const hook = window.__waveInsightProviders;
    const uri = hook.documentUri('top.sv');
    const model = monaco.editor.getModels().find(function(m){ return m.uri.toString() === uri; });
    if (!model) return { error: 'no-model' };
    const last = model.getLineCount();
    const end = model.getLineMaxColumn(last);
    model.applyEdits([{ range: { startLineNumber: last, startColumn: end, endLineNumber: last, endColumn: end },
      text: String.fromCharCode(10) + ${JSON.stringify(text)} }]);
    const line = model.getLineCount();
    return { line: line, column: model.getLineMaxColumn(line) };
  })()`);
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv')]);
  await waitTabs(cdp, 2);
  await sleep(2500);
  // the editor only forwards changes of the model it currently shows to the
  // store and the LSP, so the buffer edits below must happen while top.sv is
  // the active tab (a background model's applyEdits would not reach the server)
  await cdp.eval('window.__qa.clickText(".tab .tab-name", "top.sv")');
  await sleep(800);
  // the prefix test below reads the *server's* document text, so wait until both
  // uploaded files are open in the LSP: the editor-to-server sync is asynchronous
  // and a completion call before the didOpen answers with an empty prefix
  await waitFor(cdp, 'window.__waveInsightLsp && JSON.parse(window.__waveInsightLsp.stats()).documents >= 2', 20000);
  await sleep(600);
  const hook = await cdp.eval('window.__waveInsightProviders ? { completion: !!window.__waveInsightProviders.completion, triggers: window.__waveInsightProviders.completion ? Array.from(window.__waveInsightProviders.completion.triggerCharacters || []) : null } : null');
  note('completion hook', hook);
  check('completion: the completion provider is published with its trigger characters',
    !!hook && hook.completion === true && (hook.triggers || []).includes('.'), hook);
  // plain mode: the enclosing module's members plus every module name
  const plain = await complete('top.sv', 12, 1);
  note('completion plain', { count: labels(plain).length, labels: labels(plain) });
  check('completion: a plain position lists the module scope',
    !!plain.suggestions && labels(plain).includes('clk') && labels(plain).includes('u_cpu') && labels(plain).includes('cpu'), labels(plain));
  check('completion: the item kinds map to monaco (Variable/Field/Module/Reference)',
    kindOf(plain, 'clk') === 4 && kindOf(plain, 'u_cpu') === 21 && kindOf(plain, 'cpu') === 8, {
      clk: kindOf(plain, 'clk'), u_cpu: kindOf(plain, 'u_cpu'), cpu: kindOf(plain, 'cpu') });
  const params = await complete('cpu.sv', 12, 5);
  note('completion parameters', { WIDTH: kindOf(params, 'WIDTH'), next_pc: kindOf(params, 'next_pc') });
  check('completion: parameters are Field items and signals are Variable items',
    kindOf(params, 'WIDTH') === 3 && kindOf(params, 'next_pc') === 4, {
      WIDTH: kindOf(params, 'WIDTH'), next_pc: kindOf(params, 'next_pc') });
  // a partial prefix filters case-insensitively: `.cl` in the u_cpu port list
  const filtered = await complete('top.sv', 14, 12);
  note('completion cl', labels(filtered));
  check('completion: a prefix narrows the list to the matching label',
    JSON.stringify(labels(filtered)) === JSON.stringify(['clk']), labels(filtered));
  // member access: `u_cpu.` returns only that module's members
  const spot = await appendLine('// u_cpu.');
  note('completion edit spot', spot);
  await sleep(1500);
  const members = await complete('top.sv', spot.line, spot.column);
  note('completion u_cpu.', { count: labels(members).length, labels: labels(members) });
  check('completion: instance access lists that module members only',
    ['clk', 'reset', 'instr', 'pc', 'data_out', 'next_pc', 'pc_reg', 'active', 'STEP'].every((label) => labels(members).includes(label)), labels(members));
  const instrItem = (members.suggestions || []).find((item) => item.label === 'instr');
  check('completion: member items carry a kind and a detail',
    kindOf(members, 'clk') === 4 && kindOf(members, 'STEP') === 3
      && !!instrItem && typeof instrItem.detail === 'string' && instrItem.detail.includes('WIDTH')
      && (members.suggestions || []).every((item) => item.insertText === item.label), {
        clk: kindOf(members, 'clk'), STEP: kindOf(members, 'STEP'), instr: instrItem && instrItem.detail });
  // an unknown qualifier is an empty list, not an error
  const unknownSpot = await appendLine('// no_such_instance.');
  await sleep(1500);
  const unknown = await complete('top.sv', unknownSpot.line, unknownSpot.column);
  note('completion unknown qualifier', labels(unknown));
  check('completion: an unknown qualifier returns no suggestions',
    !!unknown.suggestions && unknown.suggestions.length === 0, labels(unknown));
  // before the first module of a file nothing is visible
  const before = await complete('cpu.sv', 1, 1);
  note('completion before the module', labels(before));
  check('completion: a position before the first module returns no suggestions',
    !!before.suggestions && before.suggestions.length === 0, labels(before));
  check('completion: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-completion.png');
};

// --- Wave 10: driver/load bit ranges in the inspector -------------------------
// The Drivers/Loads rows of a sliced reference render
// <span class="inspector-bits" title="q[3:0]">q[3:0]</span> between the name and
// the size; a whole-signal reference adds nothing, so the common row is
// unchanged.
scenarios.inspectorBits = async (cdp) => {
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'bit_ranges.sv')]);
  await waitTabs(cdp, 1);
  await sleep(2500);
  const added = await addSignal(cdp, 'q');
  await sleep(800);
  await cdp.eval('window.__qa.clickText(".dock-tab", "Drivers / loads")');
  await sleep(800);
  let s = await state(cdp);
  note('inspector q', { head: s.inspectorHead, tab: s.dockTab });
  const bits = await cdp.eval(`(function(){
    const rows = Array.prototype.slice.call(document.querySelectorAll('.inspector-panel .panel-body .sig-row'));
    const sliced = rows.filter(function(row){ return !!row.querySelector('.inspector-bits'); }).map(function(row){
      const name = row.querySelector('.sig-name');
      const bit = row.querySelector('.inspector-bits');
      const size = row.querySelector('.sig-size');
      return {
        text: (bit.innerText || '').trim(),
        title: bit.getAttribute('title'),
        afterName: !!name && name.nextElementSibling === bit,
        beforeSize: !!size && bit.nextElementSibling === size,
      };
    });
    return {
      all: Array.prototype.map.call(document.querySelectorAll('.inspector-panel .inspector-bits'), function(e){ return (e.innerText || '').trim(); }),
      sliced: sliced,
    };
  })()`);
  note('inspector bits', bits);
  check('inspectorBits: the sliced drivers are labelled with their ranges',
    added && JSON.stringify(bits.all) === JSON.stringify(['q[3:0]', 'q[7:4]']), bits.all);
  check('inspectorBits: the range sits between the name and the size',
    bits.sliced.length === 2 && bits.sliced.every((row) => row.afterName && row.beforeSize), bits.sliced);
  check('inspectorBits: the range title repeats the label',
    bits.sliced.length === 2 && bits.sliced.every((row) => row.title === row.text), bits.sliced);
  check('inspectorBits: the panel shows the selected signal',
    (s.inspectorHead || '').includes('bit_ranges.q'), s.inspectorHead);
  // a whole-signal driver adds no range label
  const addedY = await addSignal(cdp, 'y');
  await sleep(800);
  const bitsY = await cdp.eval('window.__qa.qa(".inspector-panel .inspector-bits").map(function(e){ return (e.innerText || "").trim(); })');
  const sY = await state(cdp);
  note('inspector y', { head: sY.inspectorHead, bits: bitsY });
  check('inspectorBits: a whole-signal driver has no range label',
    addedY && bitsY.length === 0, bitsY);
  check('inspectorBits: the panel switched to the whole signal',
    (sY.inspectorHead || '').includes('bit_ranges.y'), sY.inspectorHead);
  check('inspectorBits: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-inspector-bits.png');
};

// --- Wave 11: split-view polish -------------------------------------------------
// The split pane has its own separator (keyboard + drag, clamped to 25..75 %),
// its own wheel mapping while unlinked (Wave 18: a plain wheel pans its time
// axis, Shift+wheel scrolls its rows, Ctrl+wheel zooms), follows the left window
// again when linked, survives a window resize, and the ratio is persisted in
// the layout. Every assertion is DOM/state/canvas based, never style based.
scenarios.splitView = async (cdp) => {
  const hashOf = (selector) => cdp.eval('(function(){ const c = document.querySelector(' + JSON.stringify(selector) + '); if (!c) return null; const ctx = c.getContext("2d"); if (!ctx) return null; try { const d = ctx.getImageData(0,0,c.width,c.height).data; let h = 2166136261 >>> 0; for (let i=0;i<d.length;i+=4){ h ^= (d[i]|(d[i+1]<<8)|(d[i+2]<<16)|(d[i+3]<<24))>>>0; h = Math.imul(h,16777619)>>>0; } return h; } catch(e){ return "no-pixels:"+String(e); } })()');
  const splitState = () => cdp.eval(`(function(){
    const toolbar = document.querySelector('.split-toolbar');
    const view = document.querySelector('.split-view');
    const toggle = document.querySelector('.split-btn[aria-label="Split view"]');
    const linked = document.querySelector('.split-btn[aria-label="Linked time axis"]');
    const close = document.querySelector('.split-btn.split-close');
    const sep = document.querySelector('.split-separator');
    const pane = document.querySelector('.split-pane');
    const name = pane ? pane.querySelector('.split-pane-name') : null;
    const mode = pane ? pane.querySelector('.split-pane-mode') : null;
    const left = view ? view.querySelector('.wave-pane') : null;
    const leftRect = left ? left.getBoundingClientRect() : null;
    const canvas = document.querySelector('canvas.split-canvas');
    const canvasRect = canvas ? canvas.getBoundingClientRect() : null;
    const surface = document.querySelector('#wave-surface');
    return {
      toolbar: !!toolbar,
      view: !!view,
      toolbarRole: toolbar ? toolbar.getAttribute('role') : null,
      toolbarLabel: toolbar ? toolbar.getAttribute('aria-label') : null,
      pressed: toggle ? toggle.getAttribute('aria-pressed') : null,
      linked: linked ? linked.getAttribute('aria-pressed') : null,
      close: !!close,
      choices: Array.prototype.map.call(document.querySelectorAll('.split-btn.split-right-choice'), function(b){ return { label: b.getAttribute('aria-label'), active: b.classList.contains('active') }; }),
      sepRole: sep ? sep.getAttribute('role') : null,
      sepOrientation: sep ? sep.getAttribute('aria-orientation') : null,
      sepLabel: sep ? sep.getAttribute('aria-label') : null,
      sepMin: sep ? sep.getAttribute('aria-valuemin') : null,
      sepMax: sep ? sep.getAttribute('aria-valuemax') : null,
      sepNow: sep ? sep.getAttribute('aria-valuenow') : null,
      sepTabIndex: sep ? sep.getAttribute('tabindex') : null,
      style: view ? (view.getAttribute('style') || '') : null,
      leftW: leftRect ? Math.round(leftRect.width) : null,
      shield: !!document.querySelector('.split-shield'),
      paneName: name ? (name.textContent || '').trim() : null,
      paneMode: mode ? (mode.textContent || '').trim() : null,
      canvas: !!canvas,
      canvasWidth: canvas ? canvas.width : null,
      canvasHeight: canvas ? canvas.height : null,
      canvasClientW: canvas ? Math.round(canvasRect.width) : null,
      canvasClientH: canvas ? Math.round(canvasRect.height) : null,
      surface: !!surface,
      surfaceInSplit: !!(surface && surface.closest('.split-view')),
    };
  })()`);
  const clickBtn = (label) => cdp.eval(`(function(){
    const b = Array.prototype.find.call(document.querySelectorAll('.split-btn'), function(x){ return x.getAttribute('aria-label') === ${JSON.stringify('LABEL')}; });
    if (!b) return false;
    b.click();
    return true;
  })()`.replace('LABEL', label));
  const keyOnSeparator = (key) => cdp.eval(`(function(){
    const sep = document.querySelector('.split-separator');
    if (!sep) return false;
    sep.dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify('KEY')}, bubbles: true, cancelable: true }));
    return true;
  })()`.replace('KEY', key));
  // ctrl+wheel zooms, a plain wheel pans that pane's time axis, shift+wheel
  // scrolls its rows (Wave 18 Verdi mapping; the boolean form keeps the old
  // ctrl-only call sites working).
  const wheelAt = async (x, y, deltaY, options) => {
    const opts = typeof options === 'boolean' ? { ctrl: options } : (options || {});
    await wheel(cdp, x, y, deltaY, opts);
  };
  const centerOf = async (selector) => {
    const rect = await cdp.eval('window.__qa.rect(' + JSON.stringify(selector) + ')');
    return rect ? { x: Math.round(rect.x + rect.w / 2), y: Math.round(rect.y + rect.h / 2) } : null;
  };
  await boot(cdp);
  // the right pane shows the *other* dump: give it enough rows to scroll
  await cdp.upload([path.join(FIXTURES, 'many_signals.vcd')]);
  await waitStatus(cdp, 'waveform loaded', 40000);
  await sleep(800);
  const addedRows = await cdp.eval('(function(){ let n = 0; for (let i = 0; i < 20; i++) { if (window.__qa.clickSignal("sig_" + String(i).padStart(4, "0")) === "ok") n++; } return n; })()');
  await sleep(1500);
  await cdp.upload([path.join(FIXTURES, 'wave.vcd')]);
  await waitStatus(cdp, '2 dumps open', 25000);
  await sleep(800);
  const addedLeft = await cdp.eval('window.__qa.clickSignal("pc")');
  await sleep(700);
  let st = await splitState();
  note('two dumps', { addedRows: addedRows, addedLeft: addedLeft, toolbar: st.toolbar, pressed: st.pressed });
  check('splitView: the toolbar appears with two dumps',
    addedRows === 20 && st.toolbar && st.toolbarRole === 'toolbar' && st.toolbarLabel === 'Split view', st);
  check('splitView: the split starts closed', !st.view && st.pressed === 'false', st);
  // open
  await clickBtn('Split view');
  await sleep(1500);
  st = await splitState();
  note('split open', st);
  check('splitView: the split opens with the other dump on the right',
    st.view && st.pressed === 'true' && st.paneName === 'many_signals.vcd', { pressed: st.pressed, name: st.paneName });
  check('splitView: the right pane is read only', st.paneMode === 'read only', st.paneMode);
  check('splitView: the right choice lists only the non-active dump',
    st.choices.length === 1 && st.choices[0].label === 'Show many_signals.vcd on the right' && st.choices[0].active === true, st.choices);
  check('splitView: the left pane stays interactive inside the split',
    st.surface && st.surfaceInSplit && st.canvas, { surface: st.surface, inSplit: st.surfaceInSplit, rightCanvas: st.canvas });
  // (a) separator: attributes and keyboard
  note('separator', { role: st.sepRole, orientation: st.sepOrientation, label: st.sepLabel, min: st.sepMin, max: st.sepMax, now: st.sepNow, tab: st.sepTabIndex, style: st.style });
  check('splitView: the separator is an accessible vertical divider clamped to 25..75',
    st.sepRole === 'separator' && st.sepOrientation === 'vertical' && st.sepLabel === 'Split divider'
      && st.sepMin === '25' && st.sepMax === '75' && st.sepNow === '50' && st.sepTabIndex === '0',
    { role: st.sepRole, orientation: st.sepOrientation, label: st.sepLabel, min: st.sepMin, max: st.sepMax, now: st.sepNow });
  const widthAt50 = st.leftW;
  await keyOnSeparator('ArrowRight');
  await sleep(600);
  st = await splitState();
  note('after ArrowRight', { now: st.sepNow, leftW: st.leftW, style: st.style });
  check('splitView: ArrowRight moves the divider by 2 % and grows the left pane',
    st.sepNow === '52' && /--split-ratio:0\.520/.test(st.style || '') && st.leftW > widthAt50, { now: st.sepNow, leftW: st.leftW, before: widthAt50 });
  await keyOnSeparator('Home');
  await sleep(600);
  st = await splitState();
  check('splitView: Home jumps the divider to its minimum',
    st.sepNow === '25' && /--split-ratio:0\.250/.test(st.style || ''), { now: st.sepNow, style: st.style });
  await keyOnSeparator('End');
  await sleep(600);
  st = await splitState();
  check('splitView: End jumps the divider to its maximum',
    st.sepNow === '75' && /--split-ratio:0\.750/.test(st.style || ''), { now: st.sepNow, style: st.style });
  // (b) pointer drag: shield while dragging, live style, commit on release
  const viewRect = await cdp.eval('window.__qa.rect(".split-view")');
  const sepRect = await cdp.eval('window.__qa.rect(".split-separator")');
  const dragX = Math.round(viewRect.x + viewRect.w * 0.35);
  const dragY = Math.round(sepRect.y + sepRect.h / 2);
  await cdp.mouse('mousePressed', Math.round(sepRect.x + sepRect.w / 2), dragY, { buttons: 1 });
  await sleep(300);
  const during = await splitState();
  await cdp.mouse('mouseMoved', dragX, dragY, { buttons: 1 });
  await sleep(300);
  const moved = await splitState();
  await cdp.mouse('mouseReleased', dragX, dragY, { buttons: 0 });
  await sleep(800);
  const afterDrag = await splitState();
  note('separator drag', { during: { shield: during.shield, now: during.sepNow }, moved: { shield: moved.shield, style: moved.style }, after: { now: afterDrag.sepNow, style: afterDrag.style, leftW: afterDrag.leftW } });
  check('splitView: dragging the separator shows the shield and previews the ratio',
    during.shield && moved.shield && /--split-ratio:0\.3[0-9][0-9]/.test(moved.style || ''), { during: during.shield, moved: moved.shield, style: moved.style });
  check('splitView: releasing the separator commits the new ratio',
    !afterDrag.shield && afterDrag.sepNow === '35' && /--split-ratio:0\.350/.test(afterDrag.style || '')
      && Math.abs((afterDrag.leftW || 0) - Math.round(viewRect.w * 0.35)) <= 2,
    { now: afterDrag.sepNow, style: afterDrag.style, leftW: afterDrag.leftW, expected: Math.round(viewRect.w * 0.35) });
  // (c) unlinked: the right pane has its own window; the left gestures do not move it
  await clickBtn('Linked time axis');
  await sleep(800);
  st = await splitState();
  check('splitView: the linked toggle turns the link off', st.linked === 'false', st.linked);
  const leftPoint = await centerOf('#wave-surface');
  const rightPoint = await centerOf('canvas.split-canvas');
  let leftHash = await hashOf('#wave-surface');
  let rightHash = await hashOf('canvas.split-canvas');
  await wheelAt(leftPoint.x, leftPoint.y, -240, true);
  await sleep(800);
  const leftZoomed = await hashOf('#wave-surface');
  const rightAfterLeftZoom = await hashOf('canvas.split-canvas');
  note('unlinked: left zoom', { leftBefore: leftHash, leftAfter: leftZoomed, rightBefore: rightHash, rightAfter: rightAfterLeftZoom });
  check('splitView: a left zoom changes the left pane only while unlinked',
    leftZoomed !== leftHash && rightAfterLeftZoom === rightHash, { left: [leftHash, leftZoomed], right: [rightHash, rightAfterLeftZoom] });
  leftHash = leftZoomed;
  rightHash = rightAfterLeftZoom;
  await wheelAt(rightPoint.x, rightPoint.y, -240, true);
  await sleep(800);
  const rightZoomed = await hashOf('canvas.split-canvas');
  const leftAfterRightZoom = await hashOf('#wave-surface');
  note('unlinked: right zoom', { rightBefore: rightHash, rightAfter: rightZoomed, leftBefore: leftHash, leftAfter: leftAfterRightZoom });
  check('splitView: a right ctrl+wheel zooms the right pane only',
    rightZoomed !== rightHash && leftAfterRightZoom === leftHash, { right: [rightHash, rightZoomed], left: [leftHash, leftAfterRightZoom] });
  // (d) a window resize must keep the right canvas backing store in sync (the
  // verifier's second split-view bug: no ResizeObserver on the split canvas)
  st = await splitState();
  const dpr = await cdp.eval('window.devicePixelRatio || 1');
  note('resize before', { canvasWidth: st.canvasWidth, canvasHeight: st.canvasHeight, clientW: st.canvasClientW, clientH: st.canvasClientH, dpr: dpr });
  check('splitView: the right canvas backing store matches its css box before the resize',
    Math.abs(st.canvasWidth - Math.round(st.canvasClientW * dpr)) <= 1 && Math.abs(st.canvasHeight - Math.round(st.canvasClientH * dpr)) <= 1,
    { canvas: [st.canvasWidth, st.canvasHeight], css: [st.canvasClientW, st.canvasClientH], dpr: dpr });
  const beforeResizeHash = await hashOf('canvas.split-canvas');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1100, height: 600, deviceScaleFactor: 1, mobile: false });
  await sleep(1500);
  st = await splitState();
  const afterResizeHash = await hashOf('canvas.split-canvas');
  note('resize after', { canvasWidth: st.canvasWidth, canvasHeight: st.canvasHeight, clientW: st.canvasClientW, clientH: st.canvasClientH, hash: [beforeResizeHash, afterResizeHash] });
  check('splitView: a window resize updates the right canvas backing store',
    Math.abs(st.canvasWidth - Math.round(st.canvasClientW * dpr)) <= 1 && Math.abs(st.canvasHeight - Math.round(st.canvasClientH * dpr)) <= 1
      && st.canvasWidth < 1100,
    { canvas: [st.canvasWidth, st.canvasHeight], css: [st.canvasClientW, st.canvasClientH], dpr: dpr });
  check('splitView: the right canvas repaints at the new size', afterResizeHash !== beforeResizeHash, { before: beforeResizeHash, after: afterResizeHash });
  // (e) unlinked right pane: a plain wheel PANS that pane's own time axis and
  // Shift+wheel scrolls its rows (Wave 18: it used to be the other way round).
  // The pane publishes its last paint on .split-pane[data-frame], which is
  // written in view() while the paint happens in rendered(), so it is exactly
  // one paint behind: flush it with two window resizes before reading (the
  // integration probe did the same). The smaller window makes 20 rows overflow
  // the pane, so the row scroll is observable.
  await flushSplitPane(cdp, 1100, 600);
  const smallRight = await centerOf('canvas.split-canvas');
  let pane = await cdp.eval('window.__qa.splitPaneFrame()');
  const leftHashBefore = await hashOf('#wave-surface');
  const rightHashBefore = await hashOf('canvas.split-canvas');
  note('unlinked: pane before the wheel gestures', pane);
  check('splitView: the right pane publishes its frame',
    pane.raw.length > 0 && typeof pane.start === 'number' && typeof pane.scroll === 'number', pane);
  // plain wheel: the pane's own time axis moves forward, the rows do not scroll
  await wheelAt(smallRight.x, smallRight.y, 240, false);
  await sleep(600);
  await flushSplitPane(cdp, 1100, 600);
  const panePanned = await cdp.eval('window.__qa.splitPaneFrame()');
  note('unlinked: after a plain wheel', panePanned);
  check('splitView: a plain wheel pans the right pane time axis',
    panePanned.start > pane.start && panePanned.size === pane.size, { before: pane, after: panePanned });
  check('splitView: a plain wheel does not scroll the right pane rows',
    panePanned.scroll === pane.scroll, { before: pane.scroll, after: panePanned.scroll });
  // plain wheel backwards moves the start back (the pan is symmetric)
  await wheelAt(smallRight.x, smallRight.y, -240, false);
  await sleep(600);
  await flushSplitPane(cdp, 1100, 600);
  const paneBack = await cdp.eval('window.__qa.splitPaneFrame()');
  note('unlinked: after a plain wheel back', paneBack);
  check('splitView: a plain wheel pans the right pane back',
    paneBack.start < panePanned.start, { forward: panePanned.start, back: paneBack.start });
  // shift + wheel: rows scroll, the pane's view does not move. A small notch
  // first, so the later clamp pass has room to prove the clamp.
  await wheelAt(smallRight.x, smallRight.y, 60, { shift: true });
  await sleep(600);
  await flushSplitPane(cdp, 1100, 600);
  const paneRows = await cdp.eval('window.__qa.splitPaneFrame()');
  note('unlinked: after shift+wheel', paneRows);
  check('splitView: shift+wheel scrolls the right pane rows',
    paneRows.scroll > paneBack.scroll, { before: paneBack.scroll, after: paneRows.scroll });
  check('splitView: the row scroll leaves the right pane view alone',
    paneRows.start === paneBack.start && paneRows.size === paneBack.size, { view: [paneBack.start, paneRows.start], size: [paneBack.size, paneRows.size] });
  // ...and the row scroll clamps at the end of the list
  for (let i = 0; i < 10; i++) { await wheelAt(smallRight.x, smallRight.y, 400, { shift: true }); }
  await sleep(800);
  await flushSplitPane(cdp, 1100, 600);
  const paneClamped = await cdp.eval('window.__qa.splitPaneFrame()');
  await wheelAt(smallRight.x, smallRight.y, 400, { shift: true });
  await sleep(700);
  await flushSplitPane(cdp, 1100, 600);
  const paneClampedAgain = await cdp.eval('window.__qa.splitPaneFrame()');
  note('unlinked: row scroll clamp', { clamped: paneClamped.scroll, clampedAgain: paneClampedAgain.scroll });
  check('splitView: the row scroll clamps at the end of the list',
    paneClamped.scroll === paneClampedAgain.scroll && paneClamped.scroll > paneRows.scroll,
    { first: paneRows.scroll, clamped: paneClamped.scroll, clampedAgain: paneClampedAgain.scroll });
  // the right-pane gestures never touch the interactive left pane
  const leftHashAfter = await hashOf('#wave-surface');
  const rightHashAfter = await hashOf('canvas.split-canvas');
  note('unlinked: pane hashes', { left: [leftHashBefore, leftHashAfter], right: [rightHashBefore, rightHashAfter] });
  check('splitView: the right pane gestures leave the left pane untouched',
    leftHashAfter === leftHashBefore && rightHashAfter !== rightHashBefore,
    { left: [leftHashBefore, leftHashAfter], right: [rightHashBefore, rightHashAfter] });
  // (f) back to the normal window for the linked re-sync test
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await sleep(1200);
  // (g) linked again: the right pane follows the left window
  await clickBtn('Linked time axis');
  await sleep(900);
  st = await splitState();
  check('splitView: the linked toggle turns the link back on', st.linked === 'true', st.linked);
  const linkedRightBefore = await hashOf('canvas.split-canvas');
  const linkedLeftBefore = await hashOf('#wave-surface');
  const bigLeft = await centerOf('#wave-surface');
  await wheelAt(bigLeft.x, bigLeft.y, -240, true);
  await sleep(900);
  const linkedRightAfter = await hashOf('canvas.split-canvas');
  const linkedLeftAfter = await hashOf('#wave-surface');
  note('relinked: left zoom', { left: [linkedLeftBefore, linkedLeftAfter], right: [linkedRightBefore, linkedRightAfter] });
  check('splitView: with the link back on the right pane follows the left window',
    linkedLeftAfter !== linkedLeftBefore && linkedRightAfter !== linkedRightBefore, { left: [linkedLeftBefore, linkedLeftAfter], right: [linkedRightBefore, linkedRightAfter] });
  // (h) the ratio is persisted in the layout storage
  await keyOnSeparator('Home');
  await sleep(700);
  const layout = await cdp.eval('window.localStorage.getItem("wave-insight.layout.v1")');
  let parsedLayout = null;
  try { parsedLayout = JSON.parse(layout); } catch (error) { parsedLayout = null; }
  note('layout storage', parsedLayout);
  check('splitView: the divider ratio is persisted in the layout storage',
    !!parsedLayout && Math.abs((parsedLayout.split_ratio || 0) - 0.25) < 0.001, parsedLayout);
  // (h) close restores the single-pane DOM
  const closed = await clickBtn('Close split view');
  await sleep(900);
  st = await splitState();
  note('split closed', { view: st.view, toolbar: st.toolbar, surface: st.surface, inSplit: st.surfaceInSplit, sep: st.sepRole });
  check('splitView: closing removes the split view', closed && !st.view && !st.sepRole, st);
  check('splitView: the toolbar stays after closing', st.toolbar && st.pressed === 'false', st);
  check('splitView: the plain wave pane is back', st.surface && !st.surfaceInSplit && !st.canvas, st);
  check('splitView: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-split-view.png');
};


// --- Wave 12: large dumps load (or refuse), never crash ------------------------
// The generated dump is deterministic (qa/fixtures/generators/gen-large.js): a
// realistic shape with thousands of signals and many time steps. The default
// size keeps the matrix fast; bigger sizes are opt-in with QA_LARGE_MB=348,480.
// A load must end in 'waveform loaded' or the actionable refusal, never a crash
// or a blank page. The loader publishes its memory-first accounting on every
// redraw as window.__waveDebug.memory = { wasmBytes, fileBytes, decodedBytes,
// budget, dumps, unloaded, released, overBudget }: the decoded cache has to
// stay inside its budget (visible rows are never evicted, so overBudget is the
// documented soft flag), the wasm bytes inside the 1280 MB policy cap, closing
// a dump must return its bytes, and a split view counts both dumps.
// NOTE: the wide shape (200k signals / 16 MB) loads, but adding a row currently
// panics the wasm (fe-auditor is diagnosing) - this scenario only asserts the
// load for it until the fix lands; qa/README records the open bug.
scenarios.largeDump = async (cdp) => {
  const gen = require('./fixtures/generators/gen-large.js');
  const dir = path.join(QA, 'artifacts', 'large');
  fs.mkdirSync(dir, { recursive: true });
  const ensure = async (name, bytes, options) => {
    const file = path.join(dir, name);
    if (!fs.existsSync(file) || fs.statSync(file).size < bytes) {
      await gen.generate(file, bytes, options);
    }
    return { file: file, size: fs.statSync(file).size };
  };
  // wait for a terminal state: loaded, refused, or a hard timeout (a crash ends
  // the CDP connection and fails the harness, which is the regression signal)
  const loadAndWait = async (file, timeoutSeconds) => {
    await cdp.upload([file]);
    const started = Date.now();
    let status = '';
    for (let i = 0; i < timeoutSeconds; i++) {
      await sleep(1000);
      status = (await state(cdp)).status || '';
      if (/waveform loaded|cannot open|too large|more than the \d+ MB|trim the dump/i.test(status)) break;
    }
    return { status: status, ms: Date.now() - started };
  };
  const loaded = (status) => /waveform loaded/.test(status || '');
  const refused = (status) => /cannot open|too large|more than the \d+ MB|trim the dump/i.test(status || '');
  const tabsInfo = () => cdp.eval('(function(){ const box = document.querySelector(".dump-tabs"); const names = Array.prototype.map.call(document.querySelectorAll(".dump-tab-name"), function(b){ return (b.innerText || "").trim(); }); return { present: !!box, names: names, title: box ? box.getAttribute("title") : null }; })()');
  const memoryOf = () => cdp.eval('(function(){ const memory = window.__waveDebug ? window.__waveDebug.memory : null; if (!memory) return null; return { wasmBytes: memory.wasmBytes, fileBytes: memory.fileBytes, decodedBytes: memory.decodedBytes, budget: memory.budget, dumps: memory.dumps, unloaded: memory.unloaded, released: memory.released, overBudget: memory.overBudget }; })()');
  const numeric = (value) => typeof value === 'number' && Number.isFinite(value);
  const decodedOk = (memory) => !!memory && (memory.decodedBytes <= memory.budget + 8 * 1024 * 1024 || memory.overBudget === true);
  // the generated fixtures must be well formed: unique ids, one width per id
  // (the old two-char id scheme collided above 3969 signals and produced
  // malformed dumps that used to trap the parser)
  const wellFormed = (file) => {
    const text = fs.readFileSync(file, 'utf8');
    const widths = new Map();
    let signals = 0;
    let conflicts = 0;
    for (const line of text.split('\n')) {
      const match = /^\s*\$var wire (\d+) (\S+) /.exec(line);
      if (match) {
        signals += 1;
        const previous = widths.get(match[2]);
        if (previous !== undefined && previous !== match[1]) conflicts += 1;
        widths.set(match[2], match[1]);
      }
      if (line.startsWith('$enddefinitions')) break;
    }
    return { signals: signals, unique: widths.size, conflicts: conflicts };
  };
  const defaultBytes = 30 * 1024 * 1024;
  const first = await ensure('large-a-30mb.vcd', defaultBytes);
  const second = await ensure('large-b-30mb.vcd', defaultBytes, { signals: 1200, changedPerStep: 300 });
  const wide = await ensure('large-wide-200k.vcd', 16 * 1024 * 1024, { signals: 200000, changedPerStep: 200 });
  const firstShape = wellFormed(first.file);
  const wideShape = wellFormed(wide.file);
  note('generated shapes', { first: firstShape, wide: wideShape });
  check('largeDump: the generated fixtures are well formed (unique ids, one width per id)',
    firstShape.signals > 0 && firstShape.unique === firstShape.signals && firstShape.conflicts === 0
      && wideShape.signals === 200000 && wideShape.unique === wideShape.signals && wideShape.conflicts === 0,
    { first: firstShape, wide: wideShape });
  const sparse = path.join(dir, 'large-513mb.vcd');
  if (!fs.existsSync(sparse) || fs.statSync(sparse).size < 513 * 1024 * 1024) {
    // a valid VCD header so the format sniff succeeds, then a huge zero body:
    // the 512 MB per-file guard must refuse it before the body is parsed
    const header = ['$date qa oversized fixture $end', '$version qa $end', '$timescale 1ns $end',
      '$scope module top $end', '$var wire 1 ! big $end', '$upscope $end', '$enddefinitions $end'].join('\n') + '\n';
    fs.writeFileSync(sparse, header);
    fs.truncateSync(sparse, 513 * 1024 * 1024);
  }
  await boot(cdp);
  const firstLoad = await loadAndWait(first.file, 120);
  const firstState = await state(cdp);
  note('30 MB load', { ms: firstLoad.ms, status: firstLoad.status, tabs: firstState.tabs });
  check('largeDump: a 30 MB dump ends in a load or an actionable refusal',
    loaded(firstLoad.status) || refused(firstLoad.status), firstLoad.status);
  check('largeDump: the load never leaves the app blank',
    await cdp.eval('!!document.querySelector(".app")') === true && (await exceptions(cdp)).length === 0,
    { status: firstLoad.status, exceptions: await exceptions(cdp) });
  check('largeDump: the refusal path, when taken, names the limit and what to do',
    !refused(firstLoad.status) || (/\d+ MB/.test(firstLoad.status) && /trim the dump/i.test(firstLoad.status)), firstLoad.status);
  if (loaded(firstLoad.status)) {
    const memoryMb = Number((firstLoad.status.match(/wasm memory (\d+) MB/) || [])[1]);
    note('30 MB memory readout', memoryMb);
    check('largeDump: the load reports its wasm memory',
      Number.isFinite(memoryMb) && memoryMb > 0, firstLoad.status);
    check('largeDump: the memory readout is inside the documented 512 MB tab budget',
      memoryMb > 0 && memoryMb <= 512, memoryMb);
    const memoryHook = await memoryOf();
    note('30 MB memory hook', memoryHook);
    check('largeDump: the memory hook is published with numeric accounting',
      !!memoryHook && ['wasmBytes', 'fileBytes', 'decodedBytes', 'budget', 'dumps', 'unloaded', 'released'].every((field) => numeric(memoryHook[field])), memoryHook);
    check('largeDump: the hook accounts for the loaded dump',
      !!memoryHook && memoryHook.dumps === 1 && Math.abs(memoryHook.fileBytes - first.size) <= 1 && memoryHook.budget === 67108864, memoryHook);
    check('largeDump: the wasm memory stays inside the 1280 MB policy cap',
      !!memoryHook && memoryHook.wasmBytes <= 1280 * 1024 * 1024
        && Math.abs(Math.round(memoryHook.wasmBytes / (1024 * 1024)) - memoryMb) <= 2,
      { wasmBytes: memoryHook && memoryHook.wasmBytes, statusMb: memoryMb });
    check('largeDump: the decode cache stays inside its budget (or flags overBudget)',
      decodedOk(memoryHook), memoryHook);
    // the dump is usable: rows can be added and the wave scrolls
    const added = await cdp.eval('(function(){ const names = window.__qa.qa(".sidebar .sig-row .sig-name").slice(0, 3).map(function(n){ return n.innerText.trim().split("[")[0]; }); let n = 0; for (const name of names) { if (window.__qa.clickSignal(name) === "ok") n++; } return n; })()');
    await sleep(1500);
    const afterRows = await state(cdp);
    note('30 MB rows', { clicked: added, rows: afterRows.waveNames.length, status: afterRows.status });
    check('largeDump: rows can be added from a large dump',
      added > 0 && afterRows.waveNames.length > 0, { clicked: added, rows: afterRows.waveNames });
    const memoryAfterRows = await memoryOf();
    note('memory after rows', memoryAfterRows);
    check('largeDump: adding rows keeps the decode cache bounded',
      decodedOk(memoryAfterRows), memoryAfterRows);
    const surface = await cdp.eval('window.__qa.rect("#wave-surface")');
    if (surface) {
      // scroll the rows with Shift+wheel (Wave 18: a plain wheel pans the time
      // axis now), then Ctrl+wheel zoom at the pointer
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: Math.round(surface.x + surface.w / 2), y: Math.round(surface.y + surface.h / 2), deltaX: 0, deltaY: 400, modifiers: 8 });
      await sleep(500);
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: Math.round(surface.x + surface.w / 2), y: Math.round(surface.y + surface.h / 2), deltaX: 0, deltaY: -240, modifiers: 2 });
      await sleep(700);
    }
    check('largeDump: scrolling and zooming a large dump raises no exceptions',
      (await exceptions(cdp)).length === 0, await exceptions(cdp));
    // a second dump: both are counted in the strip's total bytes
    const secondLoad = await loadAndWait(second.file, 120);
    await sleep(700);
    const twoTabs = await tabsInfo();
    const expectedMb = Math.round((first.size + second.size) / (1024 * 1024));
    note('two dumps', { status: secondLoad.status, tabs: twoTabs, expectedMb: expectedMb });
    check('largeDump: a second large dump opens beside the first',
      loaded(secondLoad.status) && twoTabs.names.length === 2, twoTabs);
    check('largeDump: the strip counts both dumps against the byte budget',
      !!twoTabs.title && new RegExp('^2 dumps open, ' + expectedMb + ' MB of dump files$').test(twoTabs.title),
      { title: twoTabs.title, expectedMb: expectedMb });
    const memoryTwo = await memoryOf();
    note('memory with two dumps', memoryTwo);
    check('largeDump: the hook counts both dumps',
      !!memoryTwo && memoryTwo.dumps === 2 && Math.abs(memoryTwo.fileBytes - (first.size + second.size)) <= 2, memoryTwo);
    // closing the second dump returns its bytes to the budget
    await cdp.eval('window.__qa.click(".dump-tab-close", 1)');
    await sleep(800);
    const oneTab = await tabsInfo();
    const oneMb = Math.round(first.size / (1024 * 1024));
    note('after closing the second dump', { tabs: oneTab, expectedMb: oneMb });
    check('largeDump: closing a dump returns its bytes to the budget',
      oneTab.present && oneTab.names.length === 1 && !!oneTab.title
        && new RegExp('^1 dumps open, ' + oneMb + ' MB of dump files$').test(oneTab.title),
      { title: oneTab.title, expectedMb: oneMb });
    const memoryOne = await memoryOf();
    note('memory after closing one dump', memoryOne);
    check('largeDump: the hook returns the closed dump bytes',
      !!memoryOne && memoryOne.dumps === 1 && Math.abs(memoryOne.fileBytes - first.size) <= 1, memoryOne);
    // the split view counts both dumps again
    await loadAndWait(second.file, 120);
    await sleep(700);
    await cdp.eval('(function(){ const b = Array.prototype.find.call(document.querySelectorAll(".split-btn"), function(x){ return x.getAttribute("aria-label") === "Split view"; }); if (b) b.click(); })()');
    await sleep(1200);
    const splitTabs = await tabsInfo();
    const splitPane = await cdp.eval('(function(){ const pane = document.querySelector(".split-pane"); const name = pane ? pane.querySelector(".split-pane-name") : null; return { open: !!document.querySelector(".split-view"), name: name ? (name.textContent || "").trim() : null }; })()');
    note('split over two large dumps', { tabs: splitTabs, pane: splitPane });
    check('largeDump: the split view counts both dumps against the byte budget',
      !!splitTabs.title && new RegExp('^2 dumps open, ' + expectedMb + ' MB of dump files$').test(splitTabs.title), splitTabs.title);
    check('largeDump: the split right pane shows the other large dump',
      splitPane.open && splitPane.name === 'large-a-30mb.vcd', splitPane);
    const memorySplit = await memoryOf();
    note('memory with the split open', memorySplit);
    check('largeDump: the split counts both dumps in the hook',
      !!memorySplit && memorySplit.dumps === 2, memorySplit);
    await cdp.eval('(function(){ const b = Array.prototype.find.call(document.querySelectorAll(".split-btn"), function(x){ return x.getAttribute("aria-label") === "Close split view"; }); if (b) b.click(); })()');
    await sleep(700);
    // closing every dump releases its bytes (the second load made b the active
    // dump, so close the first tab until the strip is gone)
    for (let attempt = 0; attempt < 4; attempt++) {
      const stillOpen = await tabsInfo();
      if (!stillOpen.present) break;
      await cdp.eval('window.__qa.click(".dump-tab-close", 0)');
      await sleep(700);
    }
    const closedTabs = await tabsInfo();
    const memoryClosed = await memoryOf();
    note('after closing every dump', { tabs: closedTabs, memory: memoryClosed });
    check('largeDump: closing the last dump releases its bytes',
      !!memoryClosed && memoryClosed.dumps === 0 && memoryClosed.fileBytes === 0 && !closedTabs.present
        && memoryClosed.decodedBytes === 0,
      { memory: memoryClosed, tabs: closedTabs });
    check('largeDump: the start hint is back after closing every dump',
      await cdp.eval('!!document.querySelector(".start-hint")') === true, closedTabs);
  }
  // the cheap refusal path: a sparse file above the 512 MB per-file guard
  await boot(cdp);
  await cdp.upload([sparse]);
  await sleep(3000);
  const refusalState = await state(cdp);
  const refusalTone = await cdp.eval('window.__qa.statusTone()');
  note('513 MB refusal', { status: refusalState.status, tone: refusalTone, tabs: refusalState.tabs });
  check('largeDump: an oversized dump is refused with the documented 512 MB message',
    /512 MB/.test(refusalState.status || '') && /trim the dump/i.test(refusalState.status || ''), refusalState.status);
  check('largeDump: the refusal is an error and opens no tab',
    refusalTone === 'error' && refusalState.tabs.length === 0, { tone: refusalTone, tabs: refusalState.tabs });
  check('largeDump: the app is still alive after the refusal',
    await cdp.eval('!!document.querySelector(".app")') === true && (await exceptions(cdp)).length === 0, await exceptions(cdp));
  // the sparse guard fixture is big and gitignored: drop it again
  try { fs.rmSync(sparse, { force: true }); } catch (error) { /* best effort */ }
  // the wide shape: loads and the first row adds (the generator now writes
  // unique ids, so this is a well-formed 200k-signal dump)
  await boot(cdp);
  const wideLoad = await loadAndWait(wide.file, 120);
  const wideState = await state(cdp);
  const wideClicked = await cdp.eval('(function(){ const row = document.querySelector(".sidebar .sig-row"); if (!row) return null; row.click(); return true; })()');
  await sleep(1500);
  const wideAfter = await state(cdp);
  const wideExceptions = await exceptions(cdp);
  note('wide 200k load', { ms: wideLoad.ms, status: wideLoad.status, rows: wideState.waveNames.length, clicked: wideClicked, afterRows: wideAfter.waveNames.length, exceptions: wideExceptions });
  check('largeDump: the wide 200k-signal shape loads without a crash',
    loaded(wideLoad.status) && await cdp.eval('!!document.querySelector(".app")') === true
      && wideExceptions.length === 0, wideLoad.status);
  check('largeDump: the first row of the wide shape adds without a panic',
    wideClicked === true && wideAfter.waveNames.length > 0 && wideExceptions.length === 0,
    { clicked: wideClicked, rows: wideAfter.waveNames, exceptions: wideExceptions });
  // opt-in: bigger sizes are manual (QA_LARGE_MB=348,480) and must end the same way
  const extra = String(process.env.QA_LARGE_MB || '').split(',').map((value) => Number(value.trim())).filter((value) => value > 0 && value !== 30);
  for (const mb of extra) {
    const file = await ensure('large-' + mb + 'mb.vcd', mb * 1024 * 1024, { signals: 60, changedPerStep: 60 });
    await boot(cdp);
    const load = await loadAndWait(file.file, 180);
    const optMemory = loaded(load.status) ? await memoryOf() : null;
    note('opt-in ' + mb + ' MB', { ms: load.ms, status: load.status, memory: optMemory });
    check('largeDump: a ' + mb + ' MB dump loads or is refused, never crashes',
      (loaded(load.status) || refused(load.status)) && await cdp.eval('!!document.querySelector(".app")') === true,
      load.status);
    check('largeDump: a ' + mb + ' MB load stays inside the memory policy',
      !loaded(load.status) || (!!optMemory && optMemory.wasmBytes <= 1280 * 1024 * 1024 && decodedOk(optMemory)),
      { status: load.status, memory: optMemory });
    // the opt-in dumps are hundreds of MB and gitignored: drop them again
    try { fs.rmSync(file.file, { force: true }); } catch (error) { /* best effort */ }
  }
  check('largeDump: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-large-dump.png');
};

// --- Wave 12/13: bulk add from the module tree ----------------------------------
// The Modules panel is a multi-select: plain click focuses a row and resets the
// selection to it, Ctrl/Cmd+click toggles, Shift+click selects the visible range.
// "Add selected (N)" (and Ctrl/Cmd+Enter) adds the deduped signals of the
// selection in ONE RowsAdded batch; every .tree-row also has a .tree-add button
// that adds its whole scope. The batch dedupes by path+bit and reports
// 'added N signals (M already in the wave)' / 'no new signals: M already in the
// wave'; above 2000 signals it refuses with 'too many signals to add at once'.
// Wave 13 adds the discoverability layer: the add button is always visible, a
// right-click / Shift+F10 row menu offers the same actions, a "Select all"
// button and a sticky bulk bar (a footer, so the rows never move) make the
// multi-select findable, and the Add-selected button goes 'over-cap' above 2000.
scenarios.bulkAdd = async (cdp) => {
  const treeState = () => cdp.eval(`(function(){
    const rows = Array.prototype.map.call(document.querySelectorAll('.tree-row'), function(row){ const rect = row.getBoundingClientRect(); const add = row.querySelector('.tree-add'); return { text: (row.querySelector('.tree-name') ? row.querySelector('.tree-name').innerText : row.innerText || '').trim(), title: row.getAttribute('title'), selected: row.getAttribute('aria-selected'), classes: row.className, add: !!add, addOpacity: add ? getComputedStyle(add).opacity : null, addLabel: add ? add.getAttribute('aria-label') : null, x: Math.round(rect.x + rect.width / 2), y: Math.round(rect.y + rect.height / 2), top: Math.round(rect.top) }; });
    const menu = document.querySelector('.row-menu');
    const items = Array.prototype.map.call(document.querySelectorAll('.row-menu-item'), function(item){ return { text: (item.innerText || '').trim(), role: item.getAttribute('role') }; });
    const bar = document.querySelector('.bulk-bar');
    const count = bar ? bar.querySelector('.bulk-count') : null;
    const selectAll = document.querySelector('.tree-select-all');
    const addSelected = document.querySelector('.tree-add-selected');
    return { rows: rows, menu: menu ? { role: menu.getAttribute('role'), label: menu.getAttribute('aria-label'), items: items } : null, bar: bar ? { role: bar.getAttribute('role'), label: bar.getAttribute('aria-label'), count: count ? (count.innerText || '').trim() : null, add: !!(bar.querySelector('.bulk-add')), clear: !!(bar.querySelector('.bulk-clear')), hint: (bar.querySelector('.bulk-hint') || {}).innerText || null } : null, selectAll: selectAll ? { text: (selectAll.innerText || '').trim(), label: selectAll.getAttribute('aria-label') } : null, addSelected: addSelected ? { text: (addSelected.innerText || '').trim(), label: addSelected.getAttribute('aria-label'), title: addSelected.getAttribute('title'), classes: addSelected.className, ariaDisabled: addSelected.getAttribute('aria-disabled'), disabled: addSelected.disabled } : null };
  })()`);
  const counts = async () => {
    const current = await state(cdp);
    return { rows: current.waveNames.length, status: current.status, render: await cdp.eval('window.__qa.dbg("render_count")') };
  };
  const plainClick = async (row) => { await cdp.clickAt(row.x, row.y); await sleep(500); };
  const ctrlClick = async (row) => {
    await cdp.mouse('mousePressed', row.x, row.y, { buttons: 1, modifiers: 2 });
    await cdp.mouse('mouseReleased', row.x, row.y, { buttons: 0, modifiers: 2 });
    await sleep(450);
  };
  const shiftClick = async (row) => {
    await cdp.mouse('mousePressed', row.x, row.y, { buttons: 1, modifiers: 8 });
    await cdp.mouse('mouseReleased', row.x, row.y, { buttons: 0, modifiers: 8 });
    await sleep(450);
  };
  const clickAddSelected = async () => { await cdp.eval('(function(){ const b = document.querySelector(".tree-add-selected"); if (b) b.click(); })()'); await sleep(1200); };
  const clickTreeAdd = async (path) => cdp.eval('(function(){ const row = Array.prototype.find.call(document.querySelectorAll(".tree-row"), function(r){ return (r.getAttribute("title") || "") === ' + JSON.stringify(path) + '; }); if (!row) return null; const b = row.querySelector(".tree-add"); if (!b) return null; b.click(); return true; })()');
  const byTitle = (state, title) => state.rows.find((row) => row.title === title);
  const rowContextMenu = (title) => cdp.eval('(function(){ const row = Array.prototype.find.call(document.querySelectorAll(".tree-row"), function(r){ return (r.getAttribute("title") || "") === ' + JSON.stringify(title) + '; }); if (!row) return null; row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, button: 2 })); return true; })()');
  const menuEscape = () => cdp.eval('(function(){ const menu = document.querySelector(".row-menu"); if (!menu) return false; menu.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true })); return true; })()');
  const rowShiftF10 = (title) => cdp.eval('(function(){ const row = Array.prototype.find.call(document.querySelectorAll(".tree-row"), function(r){ return (r.getAttribute("title") || "") === ' + JSON.stringify(title) + '; }); if (!row) return null; row.dispatchEvent(new KeyboardEvent("keydown", { key: "F10", shiftKey: true, bubbles: true, cancelable: true })); return true; })()');
  const menuItem = (needle) => cdp.eval('(function(){ const items = Array.prototype.slice.call(document.querySelectorAll(".row-menu-item")); const item = items.find(function(i){ return (i.innerText || "").trim().indexOf(' + JSON.stringify(needle) + ') === 0; }); if (!item) return null; item.click(); return true; })()');
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'top.sv'), path.join(FIXTURES, 'cpu.sv')]);
  await waitTabs(cdp, 2);
  await sleep(2500);
  let tree = await treeState();
  note('bulkAdd tree', tree);
  const rows = tree.rows;
  const firstRow = rows.find((row) => row.title === 'top.u_cpu');
  const secondRow = rows.find((row) => row.title === 'top.u_cpu16');
  check('bulkAdd: every tree row exposes its selection state and an always-visible add-scope button',
    rows.length >= 2 && rows.every((row) => (row.selected === 'true' || row.selected === 'false') && row.add === true
      && (row.addLabel || '').startsWith('Add all signals in ') && row.addOpacity === '1'), rows.map((row) => ({ title: row.title, add: row.add, opacity: row.addOpacity })));
  check('bulkAdd: the add-selected button starts empty and disabled',
    !!tree.addSelected && tree.addSelected.text === 'Add selected (0)' && tree.addSelected.label === 'Add selected signals'
      && tree.addSelected.title === 'Add 0 signals from the selected scopes' && tree.addSelected.disabled === true
      && tree.addSelected.ariaDisabled === 'false', tree.addSelected);
  check('bulkAdd: the Modules head offers Select all',
    !!tree.selectAll && tree.selectAll.text === 'Select all' && tree.selectAll.label === 'Select all visible modules', tree.selectAll);
  // the discoverability row menu: right-click, Shift+F10, Escape
  const opened = await rowContextMenu('top.u_cpu');
  await sleep(600);
  tree = await treeState();
  note('row menu', { opened: opened, menu: tree.menu });
  check('bulkAdd: right-click opens the row actions menu with the add and select entries',
    opened === true && !!tree.menu && tree.menu.role === 'menu' && tree.menu.label === 'Row actions'
      && tree.menu.items.some((item) => item.role === 'menuitem' && /^Add all signals in this scope \(\d+\)$/.test(item.text))
      && tree.menu.items.some((item) => item.text === 'Select for bulk add')
      && tree.menu.items.some((item) => /^Select all visible \(\d+\)$/.test(item.text)), tree.menu);
  const escaped = await menuEscape();
  await sleep(500);
  tree = await treeState();
  check('bulkAdd: Escape closes the row menu', escaped === true && !tree.menu, tree.menu);
  const shiftF10 = await rowShiftF10('top.u_cpu');
  await sleep(500);
  tree = await treeState();
  check('bulkAdd: Shift+F10 opens the row menu too', shiftF10 === true && !!tree.menu && tree.menu.role === 'menu', tree.menu);
  const beforeMenuAdd = await counts();
  const menuAdd = await menuItem('Add all signals in this scope');
  await sleep(1500);
  const afterMenuAdd = await counts();
  note('menu add all', { clicked: menuAdd, before: beforeMenuAdd, after: afterMenuAdd });
  check('bulkAdd: the menu add-all adds the whole scope in one batch',
    menuAdd === true && afterMenuAdd.rows === beforeMenuAdd.rows + 8
      && afterMenuAdd.status === 'added 8 signals (0 already in the wave)',
    { before: beforeMenuAdd, after: afterMenuAdd });
  // Select all fills the sticky bulk bar; the rows must not move (footer, not header)
  const rowTopBefore = (await treeState()).rows[0].top;
  const selectAll = await cdp.eval('(function(){ const b = document.querySelector(".tree-select-all"); if (b) b.click(); return !!b; })()');
  await sleep(800);
  tree = await treeState();
  const rowTopAfter = tree.rows[0].top;
  note('select all', { clicked: selectAll, bar: tree.bar, selected: tree.rows.filter((row) => row.selected === 'true').length, rowTopBefore: rowTopBefore, rowTopAfter: rowTopAfter });
  check('bulkAdd: Select all selects every visible row and opens the bulk bar',
    selectAll === true && tree.rows.filter((row) => row.selected === 'true').length === tree.rows.length
      && !!tree.bar && tree.bar.role === 'status' && tree.bar.label === 'Bulk selection'
      && tree.bar.count === '16 signals selected' && tree.bar.add === true && tree.bar.clear === true
      && /Ctrl\/Cmd-click/.test(tree.bar.hint || ''), tree.bar);
  check('bulkAdd: the bulk bar is a footer and does not move the rows',
    rowTopBefore === rowTopAfter, { before: rowTopBefore, after: rowTopAfter });
  // the bulk bar's own Add selected runs the batch (the first scope is already in)
  const beforeBarAdd = await counts();
  const barAdd = await cdp.eval('(function(){ const b = document.querySelector(".bulk-add"); if (b) b.click(); return !!b; })()');
  await sleep(1500);
  const afterBarAdd = await counts();
  note('bulk bar add', { clicked: barAdd, before: beforeBarAdd, after: afterBarAdd });
  check('bulkAdd: the bulk bar Add selected adds the missing scope and reports the rest',
    barAdd === true && afterBarAdd.rows === beforeBarAdd.rows + 8
      && afterBarAdd.status === 'added 8 signals (8 already in the wave)'
      && afterBarAdd.render - beforeBarAdd.render <= 2,
    { before: beforeBarAdd, after: afterBarAdd });
  const barClear = await cdp.eval('(function(){ const b = document.querySelector(".bulk-clear"); if (b) b.click(); return !!b; })()');
  await sleep(600);
  tree = await treeState();
  check('bulkAdd: Clear hides the bulk bar and empties the selection',
    barClear === true && !tree.bar && tree.rows.every((row) => row.selected === 'false'), { bar: tree.bar, rows: tree.rows.map((row) => row.selected) });
  // the rest of the interaction contract: click, ctrl+click, Ctrl+Enter, shift range
  await plainClick(firstRow);
  tree = await treeState();
  check('bulkAdd: a plain click focuses one row and resets the selection',
    tree.rows.filter((row) => row.selected === 'true').length === 1
      && (byTitle(tree, 'top.u_cpu') || {}).selected === 'true'
      && tree.addSelected.text === 'Add selected (8)' && tree.addSelected.disabled === false,
    { rows: tree.rows.map((row) => row.selected), button: tree.addSelected });
  await ctrlClick(secondRow);
  tree = await treeState();
  check('bulkAdd: ctrl+click toggles a second row into the selection',
    tree.rows.filter((row) => row.selected === 'true').length === 2 && tree.addSelected.text === 'Add selected (16)',
    { rows: tree.rows.map((row) => row.selected), button: tree.addSelected });
  const ctrlEnter = await cdp.eval('(function(){ window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", ctrlKey: true, bubbles: true, cancelable: true })); return true; })()');
  await sleep(1200);
  const afterKey = await counts();
  check('bulkAdd: Ctrl+Enter runs the same batch as the button',
    ctrlEnter === true && afterKey.status === 'no new signals: 16 already in the wave', afterKey);
  await plainClick(firstRow);
  await shiftClick(secondRow);
  tree = await treeState();
  check('bulkAdd: shift+click selects the range from the anchor',
    tree.rows.filter((row) => row.selected === 'true').length === 2 && tree.addSelected.text === 'Add selected (16)', tree.rows.map((row) => row.selected));
  const treeAddAgain = await clickTreeAdd('top.u_cpu16');
  await sleep(1200);
  const afterTreeAdd = await counts();
  check('bulkAdd: a scope add of signals already in the wave is deduped',
    treeAddAgain === true && afterTreeAdd.rows === 16 && afterTreeAdd.status === 'no new signals: 8 already in the wave', afterTreeAdd);
  // the cap: 3000 signals selected -> over-cap button + actionable refusal, no rows
  await boot(cdp);
  await cdp.upload([path.join(FIXTURES, 'wide_scope.sv')]);
  await waitTabs(cdp, 1);
  await sleep(2000);
  await cdp.eval('(function(){ const b = document.querySelector(".tree-select-all"); if (b) b.click(); })()');
  await sleep(800);
  tree = await treeState();
  note('cap selected', tree);
  check('bulkAdd: a selection above the 2000-signal cap shows it on the button and the bar',
    !!tree.bar && tree.bar.count === '3000 signals selected'
      && !!tree.addSelected && (tree.addSelected.classes || '').includes('over-cap')
      && tree.addSelected.ariaDisabled === 'true'
      && /Too many signals \(3000\)/.test(tree.addSelected.title || ''),
    { bar: tree.bar, button: tree.addSelected });
  const beforeCap = await counts();
  const capClicked = await cdp.eval('(function(){ const b = document.querySelector(".tree-add-selected"); if (!b) return false; b.click(); return true; })()');
  await sleep(1200);
  const afterCap = await counts();
  note('cap result', { clicked: capClicked, before: beforeCap, after: afterCap });
  check('bulkAdd: an over-cap Add selected is refused with the limit and the way out',
    capClicked === true && /^too many signals to add at once: \d+ \(limit 2000\) — select fewer modules$/.test(afterCap.status || '')
      && /3000/.test(afterCap.status || ''), afterCap.status);
  check('bulkAdd: the refused batch adds no rows', afterCap.rows === 0, afterCap.rows);
  check('bulkAdd: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-bulk-add.png');
};

// --- Wave 13: dense dumps decode in a bounded window ----------------------------
// The dense shape is what blew the desktop app up: a clock that changes on every
// time step, so the full history is huge while the file is moderate. The
// generator makes a deterministic dense dump at run time (a few MB default;
// QA_DENSE_STEPS=2000000 opts into a bigger one). The FST reports cannot be
// reproduced in this image (no vcd2fst/gtkwave, iverilog/vvp are broken here,
// verilator 4.024 predates --trace-fst), so this VCD is an explicit proxy for the
// full-history decode path - see qa/README. QA_LARGE_FST=<path> adds the same
// contract for a real FST once one is available.
// Pre-fix numbers on the v0.16.0 build: a 10M-change clock row decoded to
// decodedBytes 240 MB with overBudget true and grew the wasm by ~0.5 GB per dense
// row; the bounded-window decode must keep a visible dense row to its window,
// keep the overBudget flag quiet at rest and not accumulate on zoom cycles.
scenarios.denseDump = async (cdp) => {
  const gen = require('./fixtures/generators/gen-dense.js');
  const dir = path.join(QA, 'artifacts', 'dense');
  fs.mkdirSync(dir, { recursive: true });
  const steps = Number(process.env.QA_DENSE_STEPS || '') > 0 ? Number(process.env.QA_DENSE_STEPS) : 200000;
  const file = path.join(dir, 'dense-' + steps + '.vcd');
  if (!fs.existsSync(file) || fs.statSync(file).size < steps * 6) {
    await gen.generateDense(file, { steps: steps, signals: 1 });
  }
  const size = fs.statSync(file).size;
  // A visible dense row should decode only its window. The global decode budget
  // is 64 MB and the documented soft bound is budget + 8 MB, but a windowed
  // decode is far tighter: the ~200k-change history that used to be decoded
  // whole is several MB, while the window is a few hundred changes. 2 MB is the
  // regression bound that a full-history decode cannot meet.
  const WINDOWED_BOUND = 2 * 1024 * 1024;
  const POLICY_CAP = 1280 * 1024 * 1024;
  const mem = () => cdp.eval('(function(){ const memory = window.__waveDebug ? window.__waveDebug.memory : null; if (!memory) return null; return { wasmBytes: memory.wasmBytes, fileBytes: memory.fileBytes, decodedBytes: memory.decodedBytes, budget: memory.budget, dumps: memory.dumps, overBudget: memory.overBudget }; })()');
  // wait until the decode settles: three identical memory samples in a row
  const settle = async (maxMs) => {
    const started = Date.now();
    let previous = null;
    let stable = 0;
    let samples = 0;
    while (Date.now() - started < maxMs) {
      await sleep(300);
      const current = await mem();
      samples += 1;
      if (current && previous && current.wasmBytes === previous.wasmBytes && current.decodedBytes === previous.decodedBytes) {
        stable += 1;
        if (stable >= 3) return current;
      } else {
        stable = 0;
      }
      previous = current;
    }
    return previous;
  };
  await boot(cdp);
  const started = Date.now();
  await cdp.upload([file]);
  let status = '';
  for (let i = 0; i < 120; i++) {
    await sleep(1000);
    status = (await state(cdp)).status || '';
    if (/waveform loaded|cannot open|too large|trim the dump|more than the \d+ MB/i.test(status)) break;
  }
  const loadMs = Date.now() - started;
  const loaded = /waveform loaded/.test(status);
  note('dense dump load', { steps: steps, bytes: size, ms: loadMs, status: status });
  check('denseDump: a dense dump loads (or is refused with the actionable message)',
    loaded || /cannot open|too large|trim the dump|more than the \d+ MB/i.test(status), status);
  check('denseDump: the load never leaves the app blank',
    await cdp.eval('!!document.querySelector(".app")') === true && (await exceptions(cdp)).length === 0,
    { status: status, exceptions: await exceptions(cdp) });
  if (loaded) {
    const atLoad = await mem();
    note('dense load memory', atLoad);
    check('denseDump: the memory hook is published with numeric accounting',
      !!atLoad && typeof atLoad.wasmBytes === 'number' && typeof atLoad.decodedBytes === 'number'
        && typeof atLoad.budget === 'number' && typeof atLoad.overBudget === 'boolean', atLoad);
    check('denseDump: loading a dense dump stays inside the wasm policy cap',
      !!atLoad && atLoad.wasmBytes <= POLICY_CAP, atLoad && atLoad.wasmBytes);
    const added = await cdp.eval('window.__qa.clickSignal("clk")');
    const rest = await settle(6000);
    const afterRows = await state(cdp);
    note('dense row at rest', { added: added, rows: (afterRows.waveNames || []).length, status: afterRows.status, memory: rest });
    // a transient overBudget during the decode pass is allowed: only the settled
    // state is asserted
    check('denseDump: the dense row paints',
      added === 'ok' && (afterRows.waveNames || []).length > 0, { added: added, rows: afterRows.waveNames });
    check('denseDump: at rest the decoded history stays inside the documented budget',
      !!rest && rest.decodedBytes <= rest.budget + 8 * 1024 * 1024, rest);
    check('denseDump: at rest the decode does not flag overBudget',
      !!rest && rest.overBudget === false, rest && rest.overBudget);
    check('denseDump: a visible dense row decodes only a bounded window',
      !!rest && rest.decodedBytes <= WINDOWED_BOUND, { ...rest, bound: WINDOWED_BOUND });
    check('denseDump: the wasm memory stays inside the policy cap after the decode',
      !!rest && rest.wasmBytes <= POLICY_CAP, rest && rest.wasmBytes);
    // zoom in and out three times: the window is re-decoded, never accumulated
    const surface = await cdp.eval('window.__qa.rect("#wave-surface")');
    const cycles = [];
    if (surface) {
      const x = Math.round(surface.x + surface.w / 2);
      const y = Math.round(surface.y + surface.h / 2);
      for (let cycle = 0; cycle < 3; cycle++) {
        await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: x, y: y, deltaX: 0, deltaY: -240, modifiers: 2 });
        await sleep(500);
        await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: x, y: y, deltaX: 0, deltaY: 240, modifiers: 2 });
        await sleep(500);
        cycles.push(await settle(3000));
      }
    }
    const cycleSamples = cycles.filter((sample) => !!sample);
    const peakDecoded = cycleSamples.reduce((best, sample) => (sample.decodedBytes > best.decodedBytes ? sample : best), { decodedBytes: 0, wasmBytes: 0 });
    const monotone = cycleSamples.every((sample, index) => index === 0 || sample.wasmBytes <= cycleSamples[index - 1].wasmBytes + 8 * 1024 * 1024);
    note('dense zoom cycles', { samples: cycleSamples.length, peakDecoded: peakDecoded, monotone: monotone, cycles: cycleSamples.map((sample) => ({ wasm: sample.wasmBytes, decoded: sample.decodedBytes })) });
    check('denseDump: zooming keeps the decoded history inside the windowed bound',
      cycleSamples.length === 3 && peakDecoded.decodedBytes <= WINDOWED_BOUND, peakDecoded);
    check('denseDump: zooming does not grow the wasm memory monotonically',
      cycleSamples.length === 3 && monotone, cycleSamples.map((sample) => sample.wasmBytes));
    // a second dense row must not double the decoded history
    const secondAdded = await cdp.eval('window.__qa.clickSignal("data_000")');
    const secondRest = await settle(6000);
    const secondRows = await state(cdp);
    note('dense second row at rest', { added: secondAdded, rows: (secondRows.waveNames || []).length, memory: secondRest });
    check('denseDump: a second dense row does not double the decoded history',
      secondAdded === 'ok' && !!secondRest && secondRest.decodedBytes <= WINDOWED_BOUND
        && secondRest.wasmBytes <= POLICY_CAP, secondRest);
    check('denseDump: the dense decode raises no exceptions',
      (await exceptions(cdp)).length === 0, await exceptions(cdp));
  }
  // opt-in real FST (the subagent may hand one over): same contract, QA_LARGE_FST=<path>
  const largeFst = String(process.env.QA_LARGE_FST || '').trim();
  if (largeFst && fs.existsSync(largeFst)) {
    const fstSize = fs.statSync(largeFst).size;
    await boot(cdp);
    await cdp.upload([largeFst]);
    let fstStatus = '';
    for (let i = 0; i < 180; i++) {
      await sleep(1000);
      fstStatus = (await state(cdp)).status || '';
      if (/waveform loaded|cannot open|too large|trim the dump|more than the \d+ MB/i.test(fstStatus)) break;
    }
    const fstLoaded = /waveform loaded/.test(fstStatus);
    note('large FST load', { bytes: fstSize, status: fstStatus });
    check('denseDump: a large FST loads or is refused, never crashes',
      (fstLoaded || /cannot open|too large|trim the dump|more than the \d+ MB/i.test(fstStatus))
        && await cdp.eval('!!document.querySelector(".app")') === true, fstStatus);
    if (fstLoaded) {
      const fstLoadMemory = await mem();
      check('denseDump: a large FST loads inside the wasm policy cap',
        !!fstLoadMemory && fstLoadMemory.wasmBytes <= POLICY_CAP, fstLoadMemory && fstLoadMemory.wasmBytes);
      const firstSignal = await cdp.eval('(function(){ const name = document.querySelector(".sidebar .sig-row .sig-name"); if (!name) return null; const text = (name.innerText || "").trim().split("[")[0]; name.parentElement.click(); return text; })()');
      const fstRest = await settle(8000);
      note('large FST first row', { signal: firstSignal, memory: fstRest });
      check('denseDump: the large FST stays inside the memory policy',
        !!fstRest && fstRest.decodedBytes <= fstRest.budget + 8 * 1024 * 1024
          && fstRest.wasmBytes <= POLICY_CAP, fstRest);
      // the user's likely path: bulk add a whole scope (many dense rows) and
      // scroll through the new rows - the decoded history must stay bounded
      await cdp.eval('(function(){ const row = Array.prototype.find.call(document.querySelectorAll(".tree-row"), function(r){ return (r.getAttribute("title") || "") === "top"; }); if (row) row.click(); })()');
      await sleep(800);
      const bulkTitle = await cdp.eval('(function(){ const row = Array.prototype.find.call(document.querySelectorAll(".tree-row"), function(r){ return /^top\\.u\\d+$/.test(r.getAttribute("title") || ""); }); return row ? row.getAttribute("title") : null; })()');
      const bulkClicked = bulkTitle ? await cdp.eval('(function(){ const row = Array.prototype.find.call(document.querySelectorAll(".tree-row"), function(r){ return (r.getAttribute("title") || "") === ' + JSON.stringify(bulkTitle) + '; }); if (!row) return null; const button = row.querySelector(".tree-add"); if (!button) return null; button.click(); return true; })()') : null;
      const fstBulkRest = await settle(10000);
      const fstRows = await state(cdp);
      note('large FST bulk add', { scope: bulkTitle, clicked: bulkClicked, rows: (fstRows.waveNames || []).length, status: fstRows.status, memory: fstBulkRest });
      check('denseDump: a bulk add of a large-FST scope stays inside the memory policy',
        bulkClicked === true && (fstRows.waveNames || []).length > 0 && !!fstBulkRest
          && fstBulkRest.decodedBytes <= fstBulkRest.budget + 8 * 1024 * 1024
          && fstBulkRest.overBudget === false && fstBulkRest.wasmBytes <= POLICY_CAP,
        { clicked: bulkClicked, rows: fstRows.waveNames, memory: fstBulkRest });
      const fstSurface = await cdp.eval('window.__qa.rect("#wave-surface")');
      if (fstSurface) {
        const fx = Math.round(fstSurface.x + fstSurface.w / 2);
        const fy = Math.round(fstSurface.y + fstSurface.h / 2);
        // scroll through the rows with Shift+wheel (Wave 18: the plain wheel
        // pans the time axis instead)
        for (let notch = 0; notch < 24; notch++) {
          await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: fx, y: fy, deltaX: 0, deltaY: 800, modifiers: 8 });
          await sleep(250);
        }
      }
      const fstScrolled = await settle(6000);
      note('large FST after scrolling', fstScrolled);
      check('denseDump: scrolling the large-FST rows keeps the decoded history inside the policy',
        !!fstScrolled && fstScrolled.decodedBytes <= fstScrolled.budget + 8 * 1024 * 1024
          && fstScrolled.wasmBytes <= POLICY_CAP, fstScrolled);
      check('denseDump: the large FST bulk add raises no exceptions',
        (await exceptions(cdp)).length === 0, await exceptions(cdp));
    }
  }
  check('denseDump: no exceptions', (await exceptions(cdp)).length === 0, await exceptions(cdp));
  await cdp.shot('qa-dense-dump.png');
};

(async () => {
  const wanted = process.argv.slice(2);
  const names = wanted.length ? wanted : Object.keys(scenarios);
  const cdp = await attach();
  await cdp.send('Runtime.enable');
  await cdp.send('Page.enable');
  await cdp.send('Log.enable');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  for (const name of names) {
    console.log('\n=== scenario: ' + name + ' ===');
    try {
      const renderer = await probeRenderer(cdp);
      console.log('renderer: ' + renderer);
      await scenarios[name](cdp);
    } catch (error) {
      check(name + ': scenario threw', false, String(error && error.message).slice(0, 300));
    }
    const logs = await errorLogs(cdp);
    if (logs.length) note('error logs', logs.slice(0, 5));
    const errs = await exceptions(cdp);
    if (errs.length) note('exceptions', errs.slice(0, 5));
  }
  const failed = results.filter((r) => !r.ok);
  const pendingItems = results.filter((r) => r.pending);
  const retiredItems = results.filter((r) => r.retired);
  const deferredItems = results.filter((r) => r.deferred);
  const passed = results.filter((r) => r.ok && !r.pending && !r.retired && !r.deferred);
  console.log('\n=== summary: ' + passed.length + ' passed, ' + failed.length + ' failed, '
    + pendingItems.length + ' pending (R2 layout' + (LAYOUT_REQUIRED ? ', required)' : ', run with QA_LAYOUT=1 to enforce)')
    + ', ' + deferredItems.length + ' deferred (R2 step 4)'
    + ', ' + retiredItems.length + ' retired (canvas rewrite) ===');
  if (pendingItems.length) {
    console.log('--- pending (R2 layout rules, not implemented yet) ---');
    for (const item of pendingItems) console.log('PENDING ' + item.name);
  }
  if (deferredItems.length) {
    console.log('--- deferred (R2 step 4, expected red under -Layout) ---');
    for (const item of deferredItems) console.log('DEFERRED ' + item.name);
  }
  if (retiredItems.length) {
    console.log('--- retired (covered elsewhere) ---');
    for (const item of retiredItems) console.log('RETIRED ' + item.name + ' :: ' + item.detail);
  }
  for (const f of failed) console.log('FAIL ' + f.name + ' :: ' + String(JSON.stringify(f.detail)).slice(0, 300));
  // A red matrix must fail the run. This used to exit 0 unconditionally, which
  // made `node qa.js` useless as a gate.
  process.exit(failed.length ? 1 : 0);
})().catch((e) => { console.error('harness failed: ' + (e && e.stack)); process.exit(1); });
