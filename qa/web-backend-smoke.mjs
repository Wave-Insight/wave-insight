// Loopback backend test: starts the built binary on an ephemeral port, checks
// the trust rules around /invoke, exercises every command the Web front end
// sends, and verifies the static UI is served with usable headers.
//
//   node qa/web-backend-smoke.mjs
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import http from 'node:http'
import { existsSync } from 'node:fs'
import path from 'node:path'
import test, { after } from 'node:test'
import { fileURLToPath } from 'node:url'
import { resolveAssets } from '../dsh-plugin/host/assets.js'

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
const exe = process.platform === 'win32' ? '.exe' : ''
// The bundled name and the WAVE_WEB_BACKEND override come from the plugin's
// own resolution, so a renamed platform row cannot leave this test pointing at
// a path that no longer exists. A local `cargo build --release` still wins, so
// the test runs against the source that was just compiled.
const bundled = resolveAssets().backend
const candidates = [
  path.join(root, 'target', 'release', `wave-insight-web-backend${exe}`),
  ...bundled.candidates,
]
const binary = candidates.find((candidate) => existsSync(candidate))
const uiDir = path.join(root, 'dsh-plugin', 'ui')
const dump = path.join(root, 'qa', 'fixtures', 'wave.vcd')

/** @type {{ child: import('node:child_process').ChildProcess, port: number, token: string }} */
let server

after(() => {
  server?.child?.kill()
})

/** One HTTP request against the backend. */
function request(options) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: '127.0.0.1',
        port: server.port,
        method: options.method ?? 'POST',
        path: options.path ?? '/invoke',
        headers: options.headers ?? {},
      },
      (response) => {
        const chunks = []
        response.on('data', (chunk) => chunks.push(chunk))
        response.on('end', () => {
          const body = Buffer.concat(chunks).toString('utf8')
          let json
          try {
            json = JSON.parse(body)
          } catch {
            json = undefined
          }
          resolve({ status: response.statusCode, headers: response.headers, body, json })
        })
      },
    )
    req.on('error', reject)
    if (options.body !== undefined) req.write(JSON.stringify(options.body))
    req.end()
  })
}

/** Call /invoke with the process token and a correct Host header. */
async function invoke(cmd, args = {}, overrides = {}) {
  const response = await request({
    path: '/invoke',
    headers: {
      'content-type': 'application/json',
      'x-wave-token': server.token,
      ...overrides,
    },
    body: { cmd, args },
  })
  return response
}

test('start the backend', async () => {
  assert.ok(binary, `no backend binary found; tried:\n${candidates.join('\n')}\nBuild it with: cargo build -p wave-insight-web-backend --release`)
  server = { child: null, port: 0, token: '' }
  server.child = spawn(binary, ['--ui-dir', uiDir, '--port', '0'], { stdio: ['ignore', 'pipe', 'pipe'] })
  server.port = await new Promise((resolve, reject) => {
    let text = ''
    const timer = setTimeout(() => reject(new Error('backend did not report a port')), 10_000)
    server.child.stdout.on('data', (chunk) => {
      text += chunk.toString()
      const match = /LISTENING\s+(\d+)(?:\s+(\S+))?/.exec(text)
      if (!match) return
      clearTimeout(timer)
      server.token = match[2] ?? ''
      resolve(Number(match[1]))
    })
    server.child.once('error', reject)
    server.child.once('exit', (code) => reject(new Error(`backend exited: ${code}`)))
  })
  assert.ok(server.port > 0)
  assert.match(server.token, /^[0-9a-f]{32}$/, 'the backend prints a random token next to the port')
})

test('health answers without a token and allows the DSH page origin', async () => {
  const response = await request({ method: 'GET', path: '/health' })
  assert.equal(response.status, 200)
  assert.equal(response.json?.ok, true)
  assert.equal(response.headers['access-control-allow-origin'], '*')
  assert.equal(response.headers['cache-control'], 'no-store')
})

test('invoke rejects a missing token, a foreign origin and a foreign host', async () => {
  const missing = await request({
    headers: { 'content-type': 'application/json' },
    body: { cmd: 'ping', args: {} },
  })
  assert.equal(missing.status, 403)

  const foreignOrigin = await invoke('ping', {}, { origin: 'http://evil.example' })
  assert.equal(foreignOrigin.status, 403)

  const foreignHost = await invoke('ping', {}, { host: 'evil.example' })
  assert.equal(foreignHost.status, 403)

  const allowed = await invoke('ping')
  assert.equal(allowed.status, 200)
  assert.equal(allowed.json?.ok, true)
})

test('open, hierarchy, window, value, find and close', async () => {
  const open = await invoke('open_dump', { path: dump })
  assert.equal(open.json?.ok, true, open.json?.error)
  const id = open.json.result.id
  assert.ok(id >= 0)
  assert.equal(open.json.result.name, 'wave.vcd')
  assert.ok(open.json.result.time_range)

  const hierarchy = await invoke('dump_hierarchy', { id, offset: 0, limit: 5 })
  assert.equal(hierarchy.json?.ok, true, hierarchy.json?.error)
  assert.ok(hierarchy.json.result.rows.length > 0)

  // The front end sends camelCase; the backend accepts both spellings.
  const window = await invoke('dump_window', {
    id,
    signals: [0],
    start: 0,
    end: 100,
    maxChangesPerSignal: 10,
    show: 'hex',
  })
  assert.equal(window.json?.ok, true, window.json?.error)
  assert.ok(Array.isArray(window.json.result))

  const value = await invoke('dump_value_at', { id, signal: 0, time: 100, show: 'hex' })
  assert.equal(value.json?.ok, true, value.json?.error)

  // The viewer asks for these on every adopt; the daemon answers empty shapes
  // instead of failing, so the panel's memory readout stays blank.
  const stats = await invoke('dump_memory_stats', { id })
  assert.equal(stats.json?.ok, true, stats.json?.error)
  assert.equal(stats.json.result, null)
  const memory = await invoke('process_memory')
  assert.equal(memory.json?.ok, true, memory.json?.error)
  assert.deepEqual(memory.json.result, { current_bytes: null, peak_bytes: null })

  // dump_find takes the ShowType unit-variant name, unlike the commands above.
  const find = await invoke('dump_find', { id, signal: 0, query: '1', from: 0, backwards: false, show: 'Bin' })
  assert.equal(find.json?.ok, true, find.json?.error)
  if (find.json.result) {
    assert.equal(typeof find.json.result.time, 'number')
    assert.equal(typeof find.json.result.value, 'string')
  }

  const closed = await invoke('close_dump', { id })
  assert.equal(closed.json?.ok, true, closed.json?.error)
  const reopened = await invoke('open_dump', { path: dump })
  assert.equal(reopened.json?.ok, true, reopened.json?.error)
  const all = await invoke('close_all_dumps')
  assert.equal(all.json?.ok, true)
  assert.equal(all.json.result, 1)
})

test('the built viewer is served with usable headers', async () => {
  const index = await request({ method: 'GET', path: '/' })
  assert.equal(index.status, 200)
  assert.match(index.headers['content-type'], /text\/html/)
  assert.equal(index.headers['cache-control'], 'no-cache')

  const assets = [...index.body.matchAll(/(?:src|href)="\/([^"]+)"/g)].map((match) => match[1])
  assert.ok(assets.length >= 3, 'index.html references its bundle, styles and shim')
  for (const asset of assets) {
    const response = await request({ method: 'GET', path: `/${asset}` })
    assert.equal(response.status, 200, `${asset} is served`)
    assert.equal(response.headers['x-content-type-options'], 'nosniff')
    if (asset.endsWith('.wasm')) {
      assert.equal(response.headers['content-type'], 'application/wasm')
      assert.match(response.headers['cache-control'], /immutable/)
    }
  }
})

test('traversal and unknown paths are refused', async () => {
  const traversal = await request({ method: 'GET', path: '/../src-tauri/src/dumps.rs' })
  assert.ok([400, 404].includes(traversal.status), `traversal was ${traversal.status}`)
  const encoded = await request({ method: 'GET', path: '/%2e%2e/src-tauri/src/dumps.rs' })
  assert.ok([400, 404].includes(encoded.status), `encoded traversal was ${encoded.status}`)
  const missing = await request({ method: 'GET', path: '/nope.js' })
  assert.equal(missing.status, 404)
  const unknown = await invoke('delete_everything')
  assert.equal(unknown.json?.ok, false)
  assert.match(unknown.json?.error ?? '', /unknown command/)
})
