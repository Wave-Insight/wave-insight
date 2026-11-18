// Smoke test for the loopback Web backend used by the DSH plugin UI.
// Starts the built binary on an ephemeral port, exercises the /invoke commands
// the front-end remote layer sends, then stops it.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
const binary = path.join(root, 'target', 'release', process.platform === 'win32'
  ? 'wave-insight-web-backend.exe'
  : 'wave-insight-web-backend')
const uiDir = path.join(root, 'front-end', 'dist')
const dump = path.join(root, 'test.vcd')

const child = spawn(binary, ['--ui-dir', uiDir, '--port', '0'], {
  stdio: ['ignore', 'pipe', 'pipe'],
})

try {
  const port = await new Promise((resolve, reject) => {
    let text = ''
    const timer = setTimeout(() => reject(new Error('backend did not report a port')), 10_000)
    child.stdout.on('data', (chunk) => {
      text += chunk.toString()
      const match = text.match(/LISTENING (\d+)/)
      if (match) { clearTimeout(timer); resolve(Number(match[1])) }
    })
    child.once('error', reject)
    child.once('exit', (code) => reject(new Error(`backend exited: ${code}`)))
  })

  const base = `http://127.0.0.1:${port}`
  const invoke = async (cmd, args = {}) => {
    const response = await fetch(`${base}/invoke`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ cmd, args }),
    })
    assert.equal(response.status, 200)
    return response.json()
  }

  const index = await fetch(`${base}/`)
  assert.equal(index.status, 200, 'the UI directory is served')

  const ping = await invoke('ping')
  assert.equal(ping.ok, true)

  const open = await invoke('open_dump', { path: dump })
  assert.equal(open.ok, true, open.error)
  assert.ok(open.result.id >= 0)
  const id = open.result.id

  const hierarchy = await invoke('dump_hierarchy', { id, offset: 0, limit: 5 })
  assert.equal(hierarchy.ok, true, hierarchy.error)
  assert.ok(hierarchy.result.rows.length > 0)

  const window = await invoke('dump_window', {
    id, signals: [0], start: 0, end: 100, maxChangesPerSignal: 10, show: 'hex',
  })
  assert.equal(window.ok, true, window.error)
  assert.ok(Array.isArray(window.result))

  const value = await invoke('dump_value_at', { id, signal: 0, time: 100, show: 'hex' })
  assert.equal(value.ok, true, value.error)

  const closed = await invoke('close_dump', { id })
  assert.equal(closed.ok, true, closed.error)
  console.log('ok: static UI, ping, open_dump, dump_hierarchy, dump_window, dump_value_at, close_dump')
} finally {
  child.kill()
}
