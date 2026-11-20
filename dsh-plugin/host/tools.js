// The eight model-facing tools.
//
// Every tool returns a canonical `{ summary, state }` value: `summary` is the
// text the model reads, `state` is the whole panel view after the call (null
// when the call only reads). `output.presentationMeta` copies that view onto
// the persisted tool result, which is what the `waveInsight` projection folds —
// so the panel follows the tools without a second channel.
import path from 'node:path'
import { WaveInsightError, cancellationOf, waveError } from './errors.js'
import { META_KEY, META_VERSION, emptyView } from './projection.js'

/** Hard row cap for the panel; also the cap `add_signals` reports. */
export const ROW_LIMIT = 64
/** Signals shown automatically after a load, so the panel is never empty. */
const ROWS_ON_LOAD = 20
/** Value pairs rendered to the model per signal. */
const MODEL_CHANGES = 24
/** Upper bound on decoded changes per signal for one `get_changes` call. */
const CHANGE_CAP = 64
const FIND_LIMIT_DEFAULT = 50
const FIND_LIMIT_MAX = 200
const FIND_RENDER_MAX = 30
/** How long a load waits for the viewer backend before answering anyway. */
const VIEWER_WAIT_MS = 5_000
/** Hard cap on the modules one design answer lists. */
export const DESIGN_MODULE_LIMIT = 400

const OUTPUT_SCHEMA = {
  type: 'object',
  required: ['summary', 'state'],
  properties: {
    summary: { type: 'string' },
    state: {},
    values: { type: 'array' },
  },
}

/**
 * @param {string} value
 * @param {string} label
 * @returns {string}
 */
function requiredString(value, label) {
  if (typeof value !== 'string' || value.trim().length === 0) {
    throw waveError('INVALID_ARGS', `"${label}" must be a non-empty string`)
  }
  return value
}

/**
 * @param {unknown} value
 * @param {string} label
 * @returns {number}
 */
function requiredInt(value, label) {
  const number = typeof value === 'string' ? Number(value) : value
  if (typeof number !== 'number' || !Number.isFinite(number)) {
    throw waveError('INVALID_ARGS', `"${label}" must be a finite number`)
  }
  return Math.trunc(number)
}

/**
 * @param {unknown} value
 * @param {string} label
 * @param {{ fallback: number, min: number, max: number }} limit
 * @returns {number}
 */
function boundedInt(value, label, limit) {
  if (value === undefined || value === null) return limit.fallback
  const number = requiredInt(value, label)
  if (number < limit.min || number > limit.max) {
    throw waveError('INVALID_ARGS', `"${label}" must be between ${limit.min} and ${limit.max}`)
  }
  return number
}

/**
 * @param {unknown} value
 * @param {string} label
 * @returns {string[]}
 */
function stringList(value, label) {
  if (!Array.isArray(value)) throw waveError('INVALID_ARGS', `"${label}" must be an array of paths`)
  /** @type {string[]} */
  const list = []
  for (const item of value) {
    if (typeof item !== 'string' || item.trim().length === 0) {
      throw waveError('INVALID_ARGS', `"${label}" must contain non-empty strings`)
    }
    const trimmed = item.trim()
    if (!list.includes(trimmed)) list.push(trimmed)
  }
  if (list.length === 0) throw waveError('INVALID_ARGS', `"${label}" must contain at least one path`)
  return list
}

/**
 * The sources a design tool should parse: an explicit `paths` list, the `dump`
 * argument, or the dump the panel already shows. The helper refuses an empty
 * request, so the error says which of the three to give.
 * @param {{ paths?: unknown, dump?: unknown }} args
 * @param {ReturnType<typeof emptyView>} view
 * @returns {{ paths?: string[], dump?: string }}
 */
function sourceRequest(args, view) {
  if (args?.paths !== undefined && args?.paths !== null) {
    return { paths: stringList(args.paths, 'paths') }
  }
  if (typeof args?.dump === 'string' && args.dump.trim().length > 0) {
    return { dump: path.resolve(args.dump.trim()) }
  }
  const loaded = view?.dump?.path
  if (loaded) return { dump: loaded }
  throw waveError(
    'NO_SOURCES',
    'pass "paths" (.v/.sv files) or "dump" (a waveform to find its companion RTL next to), or load a waveform first',
  )
}
/** @param {ReturnType<typeof emptyView>} view */
function requireDump(view) {
  if (!view?.dump?.path) {
    throw waveError('NO_WAVEFORM', 'no waveform is loaded yet; call load_waveform with a .vcd, .fst or .ghw path first')
  }
  return view.dump
}

/** @param {number} value @param {{start:number,end:number}|null} range */
function clampTime(value, range) {
  if (!range) return value
  return Math.min(Math.max(value, range.start), range.end)
}

/** @param {Array<{signal:string,changes:Array<{time:number,value:string}>,returned:number,capped:boolean,truncated:boolean}>} values */
function renderValues(values) {
  return values
    .map((item) => {
      const pairs = item.changes.map((change) => `${change.time}=${change.value}`).join(' ')
      const tail = item.capped
        ? ` … capped at ${item.returned}`
        : item.truncated
          ? ` … showing ${item.changes.length} of ${item.returned}`
          : ''
      return `${item.signal}: ${pairs || '(no changes in range)'}${tail}`
    })
    .join('\n')
}

/**
 * Register one tool definition.
 * @param {{ name: string, description: string, parameters: object, run: (args: any, exec: any) => Promise<any> }} definition
 */
function defineTool({ name, description, parameters, run }) {
  return {
    name,
    description,
    parameters,
    output: {
      schema: OUTPUT_SCHEMA,
      render: (_args, value) => {
        const blocks = [{ type: 'text', text: String(value?.summary ?? '') }]
        if (Array.isArray(value?.values) && value.values.length > 0) {
          blocks.push({ type: 'text', text: renderValues(value.values) })
        }
        return blocks
      },
      presentationMeta: (_args, value) => ({
        // The viewer URL is Host-process local: it carries a per-process token
        // and a port that dies with this process, so it is never persisted. The
        // projection stamps the live URL when it folds or serves this event.
        [META_KEY]: {
          version: META_VERSION,
          state: value?.state ? { ...value.state, ui: { url: null, error: null } } : null,
        },
      }),
    },
    async execute(args, exec) {
      try {
        return await run(args ?? {}, exec)
      } catch (error) {
        // Cancellation keeps its own identity so the executor still records
        // ABORTED instead of a failed tool.
        if (exec?.signal?.aborted) throw cancellationOf(exec.signal)
        if (error instanceof Error && error.name === 'AbortError') throw error
        if (error instanceof WaveInsightError) throw error
        throw waveError('TOOL_FAILED', error instanceof Error ? error.message : String(error), { cause: error })
      }
    },
  }
}

/**
 * @param {{
 *   bridge: import('./bridge.js').Bridge,
 *   sessions: import('./sessions.js').SessionViews,
 *   daemon: import('./daemon.js').WebBackend,
 * }} deps
 */
export function createTools({ bridge, sessions, daemon }) {
  return [
    defineTool({
      name: 'load_waveform',
      description:
        'Open a VCD, FST, GHW or FSDB dump with the native memory-mapped reader and show it in the Wave Insight side panel. Call this before any other waveform tool. Replaces any previously loaded dump and resets the shown rows and cursors.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          path: {
            type: 'string',
            description: 'Absolute path of the waveform file (.vcd, .fst, .ghw or .fsdb). An FSDB trace is converted once and cached by the native reader.',
          },
        },
        required: ['path'],
      },
      run: async (args, exec) => {
        const target = requiredString(args.path, 'path')
        const info = await bridge.open(target, { limit: ROWS_ON_LOAD, signal: exec.signal })
        // A loaded dump is the only signal that opens the panel, so give the
        // viewer a short chance to report its port before answering.
        await daemon.ready(VIEWER_WAIT_MS)
        const rows = info.signals.slice(0, ROWS_ON_LOAD)
        const range = info.timeRange
        const state = sessions.update(exec, {
          dump: {
            name: info.name || path.basename(target),
            path: info.path || target,
            format: info.format,
            timescale: info.timescale,
            timeRange: range ? { ...range } : null,
            signalCount: info.signalCount,
          },
          rows,
          visible: range ? { ...range } : null,
          cursors: { one: null, two: null },
          status: `loaded ${info.name || path.basename(target)} (${info.format})`,
        })
        const count = `${info.signalCount} signal${info.signalCount === 1 ? '' : 's'}`
        const span = range ? `, range ${range.start}..${range.end}` : ', no time range reported'
        const cap = info.signalCount > rows.length ? `, showing the first ${rows.length}` : ''
        return {
          summary: `${state.dump.name}: ${count}${cap}${span}. ${rows.length} row${rows.length === 1 ? '' : 's'} shown in the Wave Insight panel.`,
          state,
        }
      },
    }),

    defineTool({
      name: 'find_signals',
      description: 'Search hierarchical signal paths in the loaded dump. Returns full paths usable by add_signals and get_changes.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          query: { type: 'string', description: 'Case-insensitive substring of the signal path.' },
          limit: { type: 'number', description: `Maximum matches to return (default ${FIND_LIMIT_DEFAULT}, max ${FIND_LIMIT_MAX}).` },
        },
        required: ['query'],
      },
      run: async (args, exec) => {
        const dump = requireDump(sessions.get(exec))
        const query = requiredString(args.query, 'query')
        const limit = boundedInt(args.limit, 'limit', { fallback: FIND_LIMIT_DEFAULT, min: 1, max: FIND_LIMIT_MAX })
        const matches = await bridge.find(dump.path, query, { limit, signal: exec.signal })
        const shown = matches.slice(0, FIND_RENDER_MAX)
        if (matches.length === 0) return { summary: `no signal path matches "${query}"`, state: null }
        const list = shown
          .map((match) => (match.width === null ? match.path : `${match.path} (${match.width} bit)`))
          .join(', ')
        const more = matches.length > shown.length ? ` … ${matches.length - shown.length} more` : ''
        const capped = matches.length >= limit ? ' (limit reached)' : ''
        return {
          summary: `${matches.length} signal${matches.length === 1 ? '' : 's'} match "${query}"${capped}: ${list}${more}`,
          state: null,
        }
      },
    }),

    defineTool({
      name: 'add_signals',
      description: `Add signal paths to the wave shown in the side panel. Deduplicates, validates every path, and keeps at most ${ROW_LIMIT} rows.`,
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
      run: async (args, exec) => {
        const view = sessions.get(exec)
        const dump = requireDump(view)
        const wanted = stringList(args.signals, 'signals')
        const { known, unknown } = await bridge.validate(dump.path, wanted, { signal: exec.signal })
        const already = known.filter((name) => view.rows.includes(name))
        const fresh = known.filter((name) => !view.rows.includes(name))
        const room = Math.max(0, ROW_LIMIT - view.rows.length)
        const placed = fresh.slice(0, room)
        const dropped = fresh.length - placed.length
        const state = sessions.update(exec, {
          ...view,
          rows: [...view.rows, ...placed],
          status: placed.length > 0 ? `added ${placed.length} signal(s)` : view.status,
        })
        const parts = [`added ${placed.length} row${placed.length === 1 ? '' : 's'}`]
        if (already.length > 0) parts.push(`${already.length} already shown`)
        if (unknown.length > 0) parts.push(`unknown ${unknown.length}: ${unknown.slice(0, 10).join(', ')}${unknown.length > 10 ? ' …' : ''}`)
        if (dropped > 0) parts.push(`${dropped} dropped (${ROW_LIMIT}-row limit)`)
        return { summary: parts.join(', '), state }
      },
    }),

    defineTool({
      name: 'remove_signals',
      description: 'Remove rows from the wave shown in the side panel. Paths that are not shown are reported, not treated as errors.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          signals: { type: 'array', items: { type: 'string' }, description: 'Signal paths to remove.' },
        },
        required: ['signals'],
      },
      run: async (args, exec) => {
        const view = sessions.get(exec)
        requireDump(view)
        const wanted = stringList(args.signals, 'signals')
        const remove = new Set(wanted)
        const removed = view.rows.filter((row) => remove.has(row))
        const absent = wanted.filter((name) => !view.rows.includes(name))
        const state = sessions.update(exec, {
          ...view,
          rows: view.rows.filter((row) => !remove.has(row)),
          status: `removed ${removed.length} row(s)`,
        })
        const parts = [`removed ${removed.length} row${removed.length === 1 ? '' : 's'}`]
        if (absent.length > 0) parts.push(`not shown: ${absent.slice(0, 10).join(', ')}${absent.length > 10 ? ' …' : ''}`)
        return { summary: parts.join(', '), state }
      },
    }),

    defineTool({
      name: 'set_time_range',
      description: 'Set the visible time window of the side panel. Values are integers in dump time steps, swapped when reversed, and clamped to the dump range.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          start: { type: 'number', description: 'First time step to show.' },
          end: { type: 'number', description: 'Last time step to show.' },
        },
        required: ['start', 'end'],
      },
      run: async (args, exec) => {
        const view = sessions.get(exec)
        const dump = requireDump(view)
        let start = requiredInt(args.start, 'start')
        let end = requiredInt(args.end, 'end')
        if (start > end) [start, end] = [end, start]
        let clamped = false
        const full = dump.timeRange
        if (full) {
          const nextStart = clampTime(start, full)
          const nextEnd = Math.max(nextStart, clampTime(end, full))
          clamped = nextStart !== start || nextEnd !== end
          start = nextStart
          end = nextEnd
        }
        const state = sessions.update(exec, { ...view, visible: { start, end }, status: `range ${start}..${end}` })
        const span = full ? `; dump range ${full.start}..${full.end}` : '; the dump reports no range'
        return { summary: `showing ${start}..${end}${clamped ? ' (clamped)' : ''}${span}`, state }
      },
    }),

    defineTool({
      name: 'set_cursors',
      description: 'Place or clear the two time cursors in the side panel. Values are integer time steps and are clamped to the dump range.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          one: { type: 'number', description: 'Time step for cursor 1; omit to leave it unchanged.' },
          two: { type: 'number', description: 'Time step for cursor 2; omit to leave it unchanged.' },
          clear: { type: 'boolean', description: 'Clear both cursors when true.' },
        },
      },
      run: async (args, exec) => {
        const view = sessions.get(exec)
        const dump = requireDump(view)
        /** @type {{one:number|null,two:number|null}} */
        let cursors
        if (args.clear === true) {
          cursors = { one: null, two: null }
        } else {
          const one = args.one === undefined ? view.cursors.one : clampTime(requiredInt(args.one, 'one'), dump.timeRange)
          const two = args.two === undefined ? view.cursors.two : clampTime(requiredInt(args.two, 'two'), dump.timeRange)
          cursors = { one, two }
        }
        const state = sessions.update(exec, { ...view, cursors, status: `cursors ${cursors.one ?? '-'} / ${cursors.two ?? '-'}` })
        return { summary: `cursors ${cursors.one ?? '-'} / ${cursors.two ?? '-'}`, state }
      },
    }),

    defineTool({
      name: 'get_changes',
      description: `Read value changes for signals in a time window. Returns up to ${MODEL_CHANGES} time/value pairs per signal to the model and adds the signals to the side panel.`,
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          signals: { type: 'array', items: { type: 'string' }, description: 'Signal paths to read.' },
          start: { type: 'number', description: 'First time step; defaults to the visible window start.' },
          end: { type: 'number', description: 'Last time step; defaults to the visible window end.' },
          max: { type: 'number', description: `Changes decoded per signal before the cap (default 32, max ${CHANGE_CAP}).` },
        },
        required: ['signals'],
      },
      run: async (args, exec) => {
        const view = sessions.get(exec)
        const dump = requireDump(view)
        const names = stringList(args.signals, 'signals')
        const window = view.visible ?? dump.timeRange
        let start = args.start === undefined ? (window?.start ?? 0) : requiredInt(args.start, 'start')
        let end = args.end === undefined ? (window?.end ?? start) : requiredInt(args.end, 'end')
        if (start > end) [start, end] = [end, start]
        const max = boundedInt(args.max, 'max', { fallback: 32, min: 1, max: CHANGE_CAP })
        const waves = await bridge.waves(dump.path, names, { start, end, max, signal: exec.signal })
        const values = names.map((name, index) => {
          const wave = waves[index]
          return {
            signal: name,
            returned: wave.changes.length,
            capped: wave.capped,
            truncated: wave.changes.length > MODEL_CHANGES,
            changes: wave.changes.slice(0, MODEL_CHANGES),
          }
        })
        const rows = [...view.rows]
        for (const name of names) {
          if (rows.length >= ROW_LIMIT) break
          if (!rows.includes(name)) rows.push(name)
        }
        const state = sessions.update(exec, { ...view, rows, status: `read ${start}..${end}` })
        const preview = values
          .map((item) => `${item.signal} ${item.returned}${item.capped ? '+' : ''} change(s)`)
          .join(', ')
        return { summary: `read ${start}..${end}: ${preview}`, state, values }
      },
    }),

    defineTool({
      name: 'list_waveform',
      description: 'Report the waveform shown in the side panel: dump, rows, visible range, cursors and whether the embedded viewer is available.',
      parameters: { type: 'object', additionalProperties: false, properties: {} },
      run: async (_args, exec) => {
        const view = sessions.get(exec)
        if (!view.dump) return { summary: emptyView().status, state: view }
        const range = view.visible ? `${view.visible.start}..${view.visible.end}` : 'not set'
        const viewer = view.ui?.url ? 'viewer ready' : `viewer unavailable${view.ui?.error ? `: ${view.ui.error}` : ''}`
        return {
          summary: `${view.dump.name} (${view.dump.format}), ${view.rows.length} row(s), range ${range}, cursors ${view.cursors.one ?? '-'}/${view.cursors.two ?? '-'}, ${viewer}`,
          state: view,
        }
      },
    }),

    defineTool({
      name: 'find_sources',
      description:
        'List the Verilog/SystemVerilog sources a simulator wrote next to the loaded dump (the SpinalHDL rtl/ layout, plus generated/ and verilog/ aliases, looked for in the dump directory and the two above it). Returns absolute paths you can pass to the design tools.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          path: {
            type: 'string',
            description: 'Waveform file to look next to. Defaults to the dump the panel is showing.',
          },
        },
      },
      run: async (args, exec) => {
        let target
        if (typeof args.path === 'string' && args.path.trim().length > 0) {
          target = { name: path.basename(args.path.trim()), path: path.resolve(args.path.trim()) }
        } else {
          target = requireDump(sessions.get(exec))
        }
        const paths = await bridge.sources(target.path, { signal: exec.signal })
        if (paths.length === 0) {
          return {
            summary: `no companion RTL next to ${target.name}: looked for rtl/, generated/ or verilog/ beside it and in the two directories above`,
            state: null,
          }
        }
        const shown = paths.slice(0, 20)
        const more = paths.length > shown.length ? ` … ${paths.length - shown.length} more` : ''
        return {
          summary: `${paths.length} source${paths.length === 1 ? '' : 's'} next to ${target.name}: ${shown.join(', ')}${more}`.replace(/\s+/g, ' '),
          state: null,
          values: paths,
        }
      },
    }),

    defineTool({
      name: 'list_modules',
      description:
        'Parse Verilog/SystemVerilog sources and report the module definitions: name, file and line, ports with their direction/kind/width, and the child instances. Use it to learn the design behind a waveform. Give `dump` to read the companion RTL next to the loaded dump, or `paths` for explicit files.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          paths: {
            type: 'array',
            items: { type: 'string' },
            description: 'Absolute .v/.sv paths to parse. Omit to use `dump`.',
          },
          dump: {
            type: 'string',
            description: 'Waveform path whose companion RTL should be parsed. Defaults to the loaded dump.',
          },
          limit: { type: 'number', description: `Maximum modules to list (default ${DESIGN_MODULE_LIMIT}).` },
        },
      },
      run: async (args, exec) => {
        const request = sourceRequest(args, sessions.get(exec))
        const limit = boundedInt(args.limit, 'limit', { fallback: DESIGN_MODULE_LIMIT, min: 1, max: DESIGN_MODULE_LIMIT })
        const found = await bridge.modules({ ...request, limit }, { signal: exec.signal })
        if (found.modules.length === 0) {
          return { summary: `no modules in ${found.files.length} source file(s)`, state: null }
        }
        const shown = found.modules.slice(0, 20)
        const lines = shown.map((module) => {
          const ports = module.ports
            .map((port) => `${port.direction} ${port.kind}${port.width && port.width > 1 ? ` [${port.width - 1}:0]` : ''} ${port.name}`)
            .join(', ')
          const instances = module.instances.map((instance) => `${instance.module} ${instance.name}`).join(', ')
          return `${module.name} (${path.basename(module.file)}:${module.line})${ports ? ` — ports: ${ports}` : ''}${instances ? ` — instances: ${instances}` : ''}`
        })
        const more = found.modules.length > shown.length ? ` … ${found.modules.length - shown.length} more` : ''
        const cap = found.truncated ? ` (capped at ${limit})` : ''
        return {
          summary: `${found.moduleCount} module${found.moduleCount === 1 ? '' : 's'} in ${found.files.length} file(s)${cap}: ${lines.join(' | ')}${more}`,
          state: null,
        }
      },
    }),

    defineTool({
      name: 'find_declaration',
      description:
        'Locate a hierarchical signal in the parsed sources: the file and line of its declaration, its module/instance, its kind and width, and every place that drives or reads it. Give `dump` to parse the companion RTL next to the loaded dump, or `paths` for explicit files; `top` names the module to elaborate.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          signal: { type: 'string', description: 'Hierarchical signal path, e.g. top.u_cpu.pc.' },
          top: { type: 'string', description: 'Module to elaborate as the design root. Omit when the sources define exactly one module.' },
          paths: { type: 'array', items: { type: 'string' }, description: 'Absolute .v/.sv paths to parse. Omit to use `dump`.' },
          dump: { type: 'string', description: 'Waveform path whose companion RTL should be parsed. Defaults to the loaded dump.' },
        },
        required: ['signal'],
      },
      run: async (args, exec) => {
        const signal = requiredString(args.signal, 'signal')
        const request = sourceRequest(args, sessions.get(exec))
        const top = typeof args.top === 'string' && args.top.trim().length > 0 ? args.top.trim() : undefined
        const found = await bridge.declare({ ...request, ...(top ? { top } : {}), signal }, { signal: exec.signal })
        if (!found.declared) {
          return {
            summary: `${signal} is not in the design elaborated as ${found.top || '(no top)'}${found.diagnostics.length > 0 ? `; ${found.diagnostics.length} diagnostic(s) explain what could not be resolved` : ''}`,
            state: null,
          }
        }
        const parts = [`${signal} is declared at ${found.location}`]
        const shape = [found.direction, found.kind].filter(Boolean).join(' ')
        parts.push(`${shape}${found.width && found.width > 1 ? ` [${found.width - 1}:0]` : ''} in ${found.module} (${found.instance})`)
        if (found.drivers.length > 0) parts.push(`driven at ${found.drivers.slice(0, 5).map((ref) => ref.location).join(', ')}`)
        if (found.loads.length > 0) parts.push(`loaded at ${found.loads.slice(0, 5).map((ref) => ref.location).join(', ')}`)
        return { summary: parts.join('; '), state: null }
      },
    }),

    defineTool({
      name: 'get_diagnostics',
      description:
        'Report what the parser and the elaborator could not resolve for a set of Verilog/SystemVerilog sources: syntax problems, unsupported constructs, unresolved modules and names. Give `dump` to read the companion RTL next to the loaded dump, or `paths` for explicit files.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          paths: { type: 'array', items: { type: 'string' }, description: 'Absolute .v/.sv paths to parse. Omit to use `dump`.' },
          dump: { type: 'string', description: 'Waveform path whose companion RTL should be parsed. Defaults to the loaded dump.' },
          top: { type: 'string', description: 'Module to elaborate, whose unresolved names are reported too.' },
        },
      },
      run: async (args, exec) => {
        const request = sourceRequest(args, sessions.get(exec))
        const top = typeof args.top === 'string' && args.top.trim().length > 0 ? args.top.trim() : undefined
        const found = await bridge.diagnostics({ ...request, ...(top ? { top } : {}) }, { signal: exec.signal })
        if (found.count === 0) {
          return { summary: `no diagnostics: ${found.modules} module(s) parsed cleanly${found.top ? ` and ${found.top} elaborated` : ''}`, state: null }
        }
        const shown = found.diagnostics.slice(0, 10)
        const lines = shown.map((entry) => `${entry.location ? `${entry.location} ` : ''}${entry.severity} ${entry.kind}: ${entry.message}`)
        const more = found.count > shown.length ? ` … ${found.count - shown.length} more` : ''
        return {
          summary: `${found.count} diagnostic${found.count === 1 ? '' : 's'}${found.truncated ? ' (capped)' : ''}: ${lines.join(' | ')}${more}`,
          state: null,
        }
      },
    }),
    defineTool({
      name: 'load_sources',
      description:
        'Open Verilog/SystemVerilog files in the side panel\'s source pane, so the panel shows the same design the design tools read. Give `paths` for explicit files, or nothing to open the companion RTL next to the loaded dump. Files already open are kept; the panel re-reads them on every replay.',
      parameters: {
        type: 'object',
        additionalProperties: false,
        properties: {
          paths: {
            type: 'array',
            items: { type: 'string' },
            description: 'Absolute .v/.sv paths to open. Omit to use the companion RTL next to the loaded dump.',
          },
        },
      },
      run: async (args, exec) => {
        const view = sessions.get(exec)
        /** @type {string[]} */
        let wanted
        if (args.paths !== undefined && args.paths !== null) {
          wanted = stringList(args.paths, 'paths')
        } else {
          const dump = requireDump(view)
          wanted = await bridge.sources(dump.path, { signal: exec.signal })
          if (wanted.length === 0) {
            return {
              summary: `no companion RTL next to ${dump.name}: looked for rtl/, generated/ or verilog/ beside it and in the two directories above`,
              state: null,
            }
          }
        }
        // Opening a source is the panel's job: the view carries the list and
        // the client replays it into the iframe, which reads the text through
        // the backend. Re-opening a file is harmless.
        const already = view.sources ?? []
        const added = wanted.filter((source) => !already.includes(source))
        const state = sessions.update(exec, { ...view, sources: [...already, ...added] })
        const parts = [`opened ${added.length} source${added.length === 1 ? '' : 's'}`]
        if (wanted.length > added.length) parts.push(`${wanted.length - added.length} already open`)
        const shown = added.slice(0, 5)
        if (shown.length > 0) parts.push(shown.map((source) => path.basename(source)).join(', '))
        return { summary: parts.join(', '), state }
      },
    }),  ]
}
