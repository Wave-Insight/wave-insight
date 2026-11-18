// The web-remote shim must serialize the args the wasm app passes it.
//
// serde_wasm_bindgen::to_value turns a serde_json object into a JS Map, and
// JSON.stringify(map) is "{}" — so every open_dump arrived at the backend
// without its path and the embedded viewer never loaded a dump.
//
//   node qa/dsh-invoke-shim.test.mjs
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
const source = fs.readFileSync(path.join(root, 'dsh-plugin', 'ui', 'dsh-invoke.js'), 'utf8')

/** Boot the shim in a fake page and return the handles the test drives. */
function loadShim({ search = '?t=tok123', referrer = 'http://127.0.0.1:49877/' } = {}) {
  /** @type {Array<{ url: string, headers: Record<string, string>, body: string }>} */
  const requests = []
  /** @type {Array<any>} */
  const posted = []
  /** @type {Array<any>} */
  const dispatched = []
  const messageListeners = []

  const parent = { postMessage: (data, targetOrigin) => posted.push({ data, targetOrigin }) }
  const window = {
    location: { search },
    parent,
    addEventListener: (type, handler) => {
      if (type === 'message') messageListeners.push(handler)
    },
    dispatchEvent: (event) => {
      dispatched.push(event)
      return true
    },
  }
  const document = {
    referrer,
    readyState: 'loading',
    addEventListener() {},
    body: null,
  }
  const fetch = async (url, init) => {
    requests.push({ url, headers: init.headers, body: init.body })
    return { ok: true, json: async () => ({ ok: true, result: { id: 7 } }) }
  }
  class CustomEvent {
    constructor(type, init) {
      this.type = type
      this.detail = init?.detail
    }
  }

  const run = new Function(
    'window',
    'document',
    'location',
    'fetch',
    'URLSearchParams',
    'URL',
    'CustomEvent',
    'setTimeout',
    'clearTimeout',
    source,
  )
  run(window, document, window.location, fetch, URLSearchParams, URL, CustomEvent, setTimeout, clearTimeout)

  return { window, parent, requests, posted, dispatched, messageListeners }
}

test('a Map argument survives the JSON boundary', async () => {
  const shim = loadShim()
  const args = new Map([['path', 'F:\\projects\\Rust\\wave-insight\\test.vcd']])
  await shim.window.__WAVE_INVOKE__('open_dump', args)

  assert.equal(shim.requests.length, 1)
  const body = JSON.parse(shim.requests[0].body)
  assert.equal(body.cmd, 'open_dump')
  assert.deepEqual(body.args, { path: 'F:\\projects\\Rust\\wave-insight\\test.vcd' })
})

test('nested maps, arrays and the token header are all preserved', async () => {
  const shim = loadShim()
  const args = new Map([
    ['id', 3],
    ['signals', new Map([[0, new Map([['path', 'TOP.clk']])]])],
  ])
  await shim.window.__WAVE_INVOKE__('dump_window', args)

  const request = shim.requests[0]
  assert.equal(request.headers['x-wave-token'], 'tok123')
  assert.deepEqual(JSON.parse(request.body).args, {
    id: 3,
    signals: { 0: { path: 'TOP.clk' } },
  })
})

test('the args are tracked for dump cleanup on pagehide', async () => {
  const shim = loadShim()
  await shim.window.__WAVE_INVOKE__('open_dump', new Map([['path', 'C:\\a.vcd']]))
  assert.equal(JSON.parse(shim.requests[0].body).args.path, 'C:\\a.vcd')
})

test('only the framing window can drive the viewer', () => {
  const shim = loadShim()
  const [handler] = shim.messageListeners
  assert.equal(typeof handler, 'function')

  // A message from some other window is ignored.
  handler({ source: {}, origin: 'http://127.0.0.1:49877', data: { type: 'wave-dsh', command: { kind: 'add_signals' } } })
  assert.equal(shim.dispatched.length, 0)

  // A message from the parent, from the expected origin, is forwarded.
  handler({
    source: shim.window.parent,
    origin: 'http://127.0.0.1:49877',
    data: { type: 'wave-dsh', command: { kind: 'add_signals', signals: ['TOP.clk'] } },
  })
  assert.equal(shim.dispatched.length, 1)
  assert.equal(shim.dispatched[0].type, 'wave-web-command')
  assert.equal(shim.dispatched[0].detail.kind, 'add_signals')

  // A message from the parent but another origin is ignored.
  handler({
    source: shim.window.parent,
    origin: 'http://evil.example',
    data: { type: 'wave-dsh', command: { kind: 'add_signals' } },
  })
  assert.equal(shim.dispatched.length, 1)
})
