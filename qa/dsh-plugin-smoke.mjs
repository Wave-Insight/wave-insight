// Local smoke test for the Host half; not part of the installed bundle.
import assert from 'node:assert/strict'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import * as plugin from '../dsh-plugin/index.js'

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
const dump = path.join(root, 'qa', 'fixtures', 'wave.vcd')

const tools = []
const disposers = []
const sessionDisposed = []
let projection
const ctx = {
  effect: (callback) => {
    const dispose = callback()
    if (typeof dispose === 'function') disposers.push(dispose)
    return () => {}
  },
  tools: { register: (definition) => { tools.push(definition); return () => {} } },
  sessionProjections: { register: async (definition) => { projection = definition; return { dispose() {} } } },
  systemPrompt: { section: () => () => {} },
  on: (name, handler) => {
    if (name === 'session/disposed') sessionDisposed.push(handler)
    return () => {}
  },
}

try {
  // No helperPath/backendPath: exercise the binaries bundled in dsh-plugin/bin.
  await plugin.apply(ctx, {})
  assert.equal(tools.length, 8, 'eight tools registered')

  const exec = { agent: { session: { id: 'smoke' } } }
  const call = async (name, args) => {
    const tool = tools.find((item) => item.name === name)
    if (!tool) throw new Error(`missing tool ${name}`)
    const value = await tool.execute(args, exec)
    return { value, meta: tool.output.presentationMeta(args, value), tool }
  }
  const fold = (state, result, type = 'tool/result') =>
    projection.apply(state, type === 'tool/result'
      ? { type, data: { message: { isError: false }, meta: result.meta } }
      : { type, data: { isError: false, meta: result.meta } })

  const loaded = await call('load_waveform', { path: dump })
  console.log('load:', loaded.value.summary)
  assert.ok(loaded.meta.state.uiUrl, 'the embedded UI URL was published')
  assert.equal(loaded.meta.state.rows.length, 20)
  let state = projection.init({}, 0)
  state = fold(state, loaded)
  assert.equal(state.dump.name, 'wave.vcd')

  const added = await call('add_signals', { signals: ['top.clk'] })
  console.log('add:', added.value.summary)
  // PTC sub-calls carry isError/meta directly on data, not under data.message.
  state = fold(state, added, 'tool/ptc-dispatch')
  assert.ok(state.rows.includes('top.clk'), 'PTC dispatch folded into projection')

  const ranged = await call('set_time_range', { start: 100, end: 200 })
  console.log('range:', ranged.value.summary)
  state = fold(state, ranged)
  assert.deepEqual(state.visible, { start: 100, end: 200 })

  const changes = await call('get_changes', { signals: ['top.clk'] })
  assert.ok(changes.value.values.length === 1, 'get_changes returns values')
  assert.ok(changes.value.values[0].changes.length > 0, 'values carry time/value pairs')
  console.log('changes:', changes.value.summary, 'sample:', JSON.stringify(changes.value.values[0].changes[0]))

  const cursor = await call('set_cursors', { one: 1200, two: 1800 })
  state = fold(state, cursor)
  assert.deepEqual(state.cursors, { one: 1200, two: 1800 })
  console.log('projected cursors:', JSON.stringify(state.cursors))

  // session/disposed must evict the derived working snapshot.
  assert.ok(sessionDisposed.length === 1, 'session/disposed listener registered')
  sessionDisposed.forEach((handler) => handler({ id: 'smoke' }))
  const afterDispose = await call('list_waveform', {})
  assert.equal(afterDispose.value.state.dump, null, 'working cache was evicted')
  console.log('session cache evicted on dispose')

  const missing = tools.find((tool) => tool.name === 'load_waveform')
  await assert.rejects(
    () => missing.execute({ path: path.join(root, 'does-not-exist.vcd') }, exec),
    (error) => error.code === 'HELPER_ERROR',
    'failures are real isError results with a stable code',
  )
  console.log('ok: 8 tools, projection folds native + PTC, values returned, errors throw')
} finally {
  // Stop the backend child so the smoke test does not keep Node alive.
  disposers.forEach((dispose) => dispose())
}
