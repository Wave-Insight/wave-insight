// Host half of the wave-insight DSH bundle.
//
// Tools own one plain-JSON view of the waveform session. Every mutating tool
// attaches that view to its `tool/result` meta; the `waveInsight` projection
// folds those results and the framework streams the view to the Client panel.
import { execFileSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import { z } from 'zod'

export const inject = ['tools', 'sessionProjections', 'systemPrompt']

const stateSchema = z.object({
  dump: z.any().nullable(),
  rows: z.array(z.string()),
  visible: z.any().nullable(),
  cursors: z.any(),
  waves: z.record(z.string(), z.any()),
  status: z.string(),
  inheritedEventCount: z.number().optional(),
})

const emptyView = () => ({
  dump: null,
  rows: [],
  visible: null,
  cursors: { one: null, two: null },
  waves: {},
  status: 'No waveform loaded. Call load_waveform with a .vcd, .fst or .ghw path.',
})

// Per-session working state. The durable state lives in the session log; this
// map is just the latest decoded snapshot used to answer the next tool call.
const working = new Map()
const known = new Map()
let projectionHandle = null

function keyFor(exec) {
  return String(exec?.agent?.session?.id ?? exec?.agent?.sessionId ?? 'default')
}

function stateFor(key, exec) {
  let state = working.get(key)
  if (!state) {
    state = { ...emptyView(), path: null }
    // A resumed session keeps its waveform in the durable projection; rebuild
    // the working copy so tools do not claim nothing was loaded.
    const durable = exec?.agent?.session && projectionHandle?.stateOf?.(exec.agent.session)
    if (durable?.dump?.path) {
      state.path = durable.dump.path
      state.dump = durable.dump
      state.rows = [...(durable.rows ?? [])]
      state.visible = durable.visible
      state.cursors = { ...(durable.cursors ?? { one: null, two: null }) }
      state.waves = { ...(durable.waves ?? {}) }
      state.status = durable.status ?? 'waveform restored from session'
    }
    working.set(key, state)
    known.set(key, new Set(state.rows))
  }
  return state
}

function ensureKnown(state, key, config) {
  if (!state.path || (known.get(key)?.size ?? 0) > 0) return
  const found = callHelper(config, { cmd: 'find', path: state.path, query: '', limit: 10_000 })
  known.set(key, new Set(found.matches.map((match) => match.path)))
}

function helperPath(config) {
  if (config?.helperPath) return config.helperPath
  if (process.env.WAVE_DSH_BRIDGE) return process.env.WAVE_DSH_BRIDGE
  const bundled = path.join(
    path.dirname(fileURLToPath(import.meta.url)),
    'bin',
    process.platform === 'win32' ? 'dsh-bridge.exe' : 'dsh-bridge',
  )
  return bundled
}

function callHelper(config, request) {
  const helper = helperPath(config)
  const stdout = execFileSync(helper, {
    input: `${JSON.stringify(request)}\n`,
    encoding: 'utf8',
    timeout: 60_000,
    maxBuffer: 64 * 1024 * 1024,
  })
  const line = stdout.split(/\r?\n/).find((part) => part.trim().length > 0)
  if (!line) throw new Error('wave-insight helper returned no response')
  const response = JSON.parse(line)
  if (!response.ok) throw new Error(response.error || 'wave-insight helper error')
  return response
}

function refreshWaves(state, config) {
  if (!state.path || state.rows.length === 0 || !state.visible) {
    state.waves = {}
    return
  }
  const start = Math.floor(state.visible.start || 0)
  const end = Math.floor(state.visible.end || start)
  const response = callHelper(config, {
    cmd: 'wave',
    path: state.path,
    signals: state.rows,
    start,
    end,
    max: 256,
  })
  const waves = {}
  state.rows.forEach((row, index) => {
    waves[row] = response.waves[index]?.changes ?? []
  })
  state.waves = waves
}

function viewOf(state) {
  return {
    dump: state.dump,
    rows: [...state.rows],
    visible: state.visible,
    cursors: { ...state.cursors },
    waves: { ...state.waves },
    status: state.status,
  }
}

function result(summary, state, key) {
  return { summary, state: state ? viewOf(state) : null, key }
}

function defineTool({ name, description, parameters, run }) {
  return {
    name,
    description,
    parameters,
    output: {
      schema: { type: 'object' },
      render: (_args, value) => [{ type: 'text', text: value.summary }],
      presentationMeta: (_args, value) =>
        value.state
          ? { __waveInsight: true, state: value.state }
          : { __waveInsight: false },
    },
    async execute(args, exec) {
      const key = keyFor(exec)
      try {
        return await run(args, exec, key)
      } catch (error) {
        const state = stateFor(key, exec)
        const message = error instanceof Error ? error.message : String(error)
        state.status = `error: ${message}`
        return result(`Error: ${message}`, state, key)
      }
    },
  }
}

export async function apply(ctx, config) {
  projectionHandle = await ctx.sessionProjections.register({
    key: 'waveInsight',
    stateSchema,
    init: (_header, inheritedEventCount) => ({
      ...emptyView(),
      inheritedEventCount,
    }),
    apply: (state, event) => {
      if (event.seq !== undefined && event.seq < (state.inheritedEventCount ?? 0)) return state
      if (event.type !== 'tool/result' || event.data?.message?.isError === true) return state
      const meta = event.data?.meta
      if (!meta || meta.__waveInsight !== true || !meta.state) return state
      return { ...state, ...meta.state, inheritedEventCount: state.inheritedEventCount }
    },
    wire: { viewSchema: stateSchema, view: (state) => state, summary: false },
    stateVersion: 1,
  })

  const tools = [
    defineTool({
      name: 'load_waveform',
      description:
        'Open a VCD, FST or GHW dump and show it in the Wave Insight side panel. Call this before the other waveform tools unless the user names an already loaded dump.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          path: {
            type: 'string',
            description: 'Absolute path of the waveform file to open.',
          },
        },
        required: ['path'],
      },
      run: (args, exec, key) => {
        const info = callHelper(config, { cmd: 'open', path: args.path, limit: 10_000 })
        const state = stateFor(key, exec)
        const rows = info.signals.slice(0, 20).map((signal) => signal.path)
        state.path = info.path
        state.dump = {
          name: info.name,
          path: info.path,
          format: info.format,
          timescale: info.timescale,
          time_range: info.time_range,
          signal_count: info.signal_count,
          signals_truncated: info.signals_truncated,
        }
        state.rows = rows
        state.visible = info.time_range ?? { start: 0, end: 1 }
        state.cursors = { one: null, two: null }
        known.set(key, new Set(info.signals.map((signal) => signal.path)))
        refreshWaves(state, config)
        state.status = `loaded ${info.name} (${info.format})`
        return result(
          `${info.name}: ${info.signal_count} signals, range ${state.visible.start}..${state.visible.end}. Showing ${rows.length} rows in the side panel.`,
          state,
          key,
        )
      },
    }),
    defineTool({
      name: 'find_signals',
      description: 'Search signal paths in the loaded dump. Returns full hierarchical paths usable by add_signals.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          query: { type: 'string', description: 'Case-insensitive substring of the signal path.' },
          limit: { type: 'number', description: 'Maximum matches to return (default 100, max 500).' },
        },
        required: ['query'],
      },
      run: (args, exec, key) => {
        const state = stateFor(key, exec)
        if (!state.path) throw new Error('load a waveform first')
        const found = callHelper(config, {
          cmd: 'find',
          path: state.path,
          query: args.query,
          limit: args.limit ?? 100,
        })
        const shown = found.matches.slice(0, 25)
        return result(
          shown.length
            ? `matches: ${shown.map((match) => `${match.path} (${match.width} bit)`).join(', ')}`
            : 'no matching signals',
          null,
          key,
        )
      },
    }),
    defineTool({
      name: 'add_signals',
      description: 'Add signal paths to the wave shown in the side panel. Deduplicates and keeps at most 40 rows.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          signals: {
            type: 'array',
            description: 'Full signal paths, e.g. top.pc.',
            items: { type: 'string' },
          },
        },
        required: ['signals'],
      },
      run: (args, exec, key) => {
        const state = stateFor(key, exec)
        if (!state.path) throw new Error('load a waveform first')
        ensureKnown(state, key, config)
        const paths = known.get(key) ?? new Set()
        const wanted = [...new Set(args.signals.map((signal) => String(signal)))]
        const unknown = wanted.filter((signal) => !paths.has(signal))
        const fresh = wanted.filter((signal) => paths.has(signal) && !state.rows.includes(signal))
        state.rows = [...state.rows, ...fresh].slice(0, 40)
        refreshWaves(state, config)
        state.status = `added ${fresh.length} signal(s)`
        return result(
          `added ${fresh.length}, already on wave ${wanted.length - fresh.length - unknown.length}, unknown ${unknown.length}${unknown.length ? `: ${unknown.join(', ')}` : ''}`,
          state,
          key,
        )
      },
    }),
    defineTool({
      name: 'remove_signals',
      description: 'Remove signal paths from the wave shown in the side panel.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          signals: { type: 'array', items: { type: 'string' } },
        },
        required: ['signals'],
      },
      run: (args, exec, key) => {
        const state = stateFor(key, exec)
        const remove = new Set(args.signals.map(String))
        const before = state.rows.length
        state.rows = state.rows.filter((row) => !remove.has(row))
        refreshWaves(state, config)
        state.status = `removed ${before - state.rows.length} signal(s)`
        return result(`removed ${before - state.rows.length} row(s)`, state, key)
      },
    }),
    defineTool({
      name: 'set_time_range',
      description: 'Set the visible time window of the side panel. Values are clamped to the loaded dump.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          start: { type: 'number', description: 'First time step to show.' },
          end: { type: 'number', description: 'Last time step to show.' },
        },
        required: ['start', 'end'],
      },
      run: (args, exec, key) => {
        const state = stateFor(key, exec)
        if (!state.path) throw new Error('load a waveform first')
        const full = state.dump?.time_range ?? { start: 0, end: Number.MAX_SAFE_INTEGER }
        const requested = { start: Math.floor(args.start), end: Math.floor(args.end) }
        const start = Math.max(full.start, Math.min(requested.start, full.end))
        const end = Math.max(start, Math.min(requested.end, full.end))
        state.visible = { start, end }
        refreshWaves(state, config)
        state.status = `range ${start}..${end}`
        return result(`showing ${start}..${end} (clamped to ${full.start}..${full.end})`, state, key)
      },
    }),
    defineTool({
      name: 'set_cursors',
      description: 'Place or clear the two cursors in the side panel (time steps).',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          one: { type: 'number', description: 'Time step for cursor 1.' },
          two: { type: 'number', description: 'Time step for cursor 2.' },
          clear: { type: 'boolean', description: 'Clear both cursors when true.' },
        },
      },
      run: (args, exec, key) => {
        const state = stateFor(key, exec)
        if (args.clear) state.cursors = { one: null, two: null }
        else {
          if (typeof args.one === 'number') state.cursors.one = Math.floor(args.one)
          if (typeof args.two === 'number') state.cursors.two = Math.floor(args.two)
        }
        state.status = `cursors ${state.cursors.one ?? '-'} / ${state.cursors.two ?? '-'}`
        return result(state.status, state, key)
      },
    }),
    defineTool({
      name: 'get_changes',
      description: 'Read value changes for signals in a time window and draw them in the side panel.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          signals: { type: 'array', items: { type: 'string' } },
          start: { type: 'number' },
          end: { type: 'number' },
          max: { type: 'number', description: 'Maximum changes per signal (default 256, max 4096).' },
        },
        required: ['signals'],
      },
      run: (args, exec, key) => {
        const state = stateFor(key, exec)
        if (!state.path) throw new Error('load a waveform first')
        const start = Math.floor(args.start ?? state.visible?.start ?? 0)
        const end = Math.floor(args.end ?? state.visible?.end ?? start)
        const response = callHelper(config, {
          cmd: 'wave',
          path: state.path,
          signals: args.signals,
          start,
          end,
          max: args.max ?? 256,
        })
        response.waves.forEach((wave, index) => {
          const path = args.signals[index]
          if (!state.rows.includes(path) && state.rows.length < 40) state.rows.push(path)
          state.waves = { ...state.waves, [path]: wave.changes }
        })
        const preview = response.waves
          .map((wave, index) => `${args.signals[index]}: ${wave.changes.length} changes`)
          .join(', ')
        state.status = `read ${start}..${end}`
        return result(preview, state, key)
      },
    }),
    defineTool({
      name: 'list_waveform',
      description: 'Report the waveform currently shown in the side panel: dump, rows, visible range and cursors.',
      parameters: { type: 'object', additionalProperties: false, properties: {} },
      run: (_args, exec, key) => {
        const state = stateFor(key, exec)
        return result(
          state.dump
            ? `${state.dump.name} (${state.dump.format}), ${state.rows.length} rows, range ${state.visible?.start}..${state.visible?.end}, cursors ${state.cursors.one ?? '-'}/${state.cursors.two ?? '-'}`
            : emptyView().status,
          state,
          key,
        )
      },
    }),
  ]

  ctx.effect(() => {
    const disposers = tools.map((definition) => ctx.tools.register(definition))
    return () => disposers.forEach((dispose) => dispose())
  }, 'wave-insight: tools')

  if (ctx.systemPrompt?.section) {
    ctx.effect(
      () =>
        ctx.systemPrompt.section({
          name: 'wave-insight',
          order: 50,
          text:
            'Wave Insight tools show a live waveform in the DSH side panel. Start with load_waveform(path); the panel follows every later waveform tool call.',
        }),
      'wave-insight: prompt',
    )
  }
}
