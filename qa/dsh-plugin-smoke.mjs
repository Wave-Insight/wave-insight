// Local smoke test for the Host half; not part of the installed bundle.
import * as plugin from '../dsh-plugin/index.js'

const tools = []
let projection
const ctx = {
  effect: (callback) => { callback(); return () => {} },
  tools: { register: (definition) => { tools.push(definition); return () => {} } },
  sessionProjections: { register: async (definition) => { projection = definition; return { dispose() {} } } },
  systemPrompt: { section: () => () => {} },
}

// No helperPath: exercise the helper bundled in dsh-plugin/bin/
await plugin.apply(ctx, {})

const exec = { agent: { session: { id: 'smoke' } } }
const call = async (name, args) => {
  const tool = tools.find((item) => item.name === name)
  if (!tool) throw new Error(`missing tool ${name}`)
  const value = await tool.execute(args, exec)
  return { value, meta: tool.output.presentationMeta(args, value) }
}

console.log('registered tools:', tools.map((tool) => tool.name).join(', '))

const loaded = await call('load_waveform', { path: 'F:/projects/Rust/wave-insight/test.vcd' })
console.log('load:', loaded.value.summary)
console.log('state:', loaded.meta.state.dump.name, loaded.meta.state.rows.length, 'rows', Object.keys(loaded.meta.state.waves).length, 'waves')

let state = projection.init({}, 0)
state = projection.apply(state, { type: 'tool/result', data: { message: { isError: false }, meta: loaded.meta } })
console.log('projected:', state.dump.name, state.rows.length, 'rows', Object.keys(state.waves).length, 'waves')

const added = await call('add_signals', { signals: ['TOP.clk'] })
console.log('add:', added.value.summary)
state = projection.apply(state, { type: 'tool/result', data: { message: { isError: false }, meta: added.meta } })
console.log('projected rows:', state.rows.join(','))

const ranged = await call('set_time_range', { start: 1000, end: 2000 })
console.log('range:', ranged.value.summary)
state = projection.apply(state, { type: 'tool/result', data: { message: { isError: false }, meta: ranged.meta } })
console.log('projected visible:', JSON.stringify(state.visible), 'clk changes:', state.waves['TOP.clk']?.length)

const cursor = await call('set_cursors', { one: 1200, two: 1800 })
state = projection.apply(state, { type: 'tool/result', data: { message: { isError: false }, meta: cursor.meta } })
console.log('projected cursors:', JSON.stringify(state.cursors))
