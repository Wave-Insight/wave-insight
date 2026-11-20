// The `waveInsight` session projection.
//
// Tools attach the whole view to their result meta; this fold turns the last
// non-failed one into the value the Client panel reads. State is a pure
// function of the session log, with one deliberate exception: the viewer URL
// belongs to the Host process, not to the session, so it is stripped from every
// folded value and re-stamped from the live backend. A resumed session
// therefore never iframes a port from a previous DSH run.
import { checkRange, createSchema, isInteger, isRecord, isStringOrNull } from './schema.js'

export const PROJECTION_KEY = 'waveInsight'

/**
 * Version of the tool-result meta payload. Bumped when the view shape changes
 * in a way an older Host could misread.
 */
export const META_VERSION = 1

/** Key under `tool/result.data.meta` and `tool/ptc-dispatch.data.meta`. */
export const META_KEY = PROJECTION_KEY

/** Bump when the state fields or fold semantics change (checkpoint invalidation). */
export const STATE_VERSION = 4

/** @param {unknown} value @returns {string | null} */
function checkDump(value) {
  if (value === null) return null
  if (!isRecord(value)) return 'dump must be an object or null'
  for (const field of ['name', 'path', 'format']) {
    if (typeof value[field] !== 'string') return `dump.${field} must be a string`
  }
  if (!isStringOrNull(value.timescale)) return 'dump.timescale must be a string or null'
  if (!isInteger(value.signalCount) || value.signalCount < 0) return 'dump.signalCount must be a non-negative integer'
  return checkRange(value.timeRange === undefined ? null : value.timeRange)
}

/** @param {unknown} value @returns {string | null} */
function checkView(value) {
  if (!isRecord(value)) return 'the view must be an object'
  const dump = checkDump(value.dump === undefined ? null : value.dump)
  if (dump) return dump
  if (!Array.isArray(value.rows) || value.rows.some((row) => typeof row !== 'string')) {
    return 'rows must be an array of strings'
  }
  const visible = checkRange(value.visible === undefined ? null : value.visible)
  if (visible) return visible
  if (!isRecord(value.cursors)) return 'cursors must be an object'
  for (const cursor of ['one', 'two']) {
    if (!isInteger(value.cursors[cursor]) && value.cursors[cursor] !== null) return `cursors.${cursor} must be an integer or null`
  }
  if (typeof value.status !== 'string') return 'status must be a string'
  if (!isRecord(value.ui)) return 'ui must be an object'
  for (const field of ['url', 'error']) {
    if (!isStringOrNull(value.ui[field])) return `ui.${field} must be a string or null`
  }
  return null
}

export const viewSchema = createSchema('waveInsight view', checkView)

/** State of a session that has not loaded a waveform yet. */
export function emptyView() {
  return {
    dump: null,
    rows: [],
    visible: null,
    cursors: { one: null, two: null },
    status: 'No waveform is loaded. Call load_waveform with a .vcd, .fst or .ghw path.',
    ui: { url: null, error: null },
  }
}

/**
 * Whether two views are equal in everything the panel renders. The registry
 * publishes only when the folded state reference changes, so an unchanged tool
 * result must return the previous reference.
 * @param {ReturnType<typeof emptyView>} left
 * @param {ReturnType<typeof emptyView>} right
 */
export function sameView(left, right) {
  if (left === right) return true
  if (!left || !right) return false
  if (left.status !== right.status) return false
  if (left.ui?.url !== right.ui?.url || left.ui?.error !== right.ui?.error) return false
  if (!sameDump(left.dump, right.dump)) return false
  if (!sameRange(left.visible, right.visible)) return false
  if ((left.cursors?.one ?? null) !== (right.cursors?.one ?? null)) return false
  if ((left.cursors?.two ?? null) !== (right.cursors?.two ?? null)) return false
  const leftRows = left.rows ?? []
  const rightRows = right.rows ?? []
  if (leftRows.length !== rightRows.length) return false
  return leftRows.every((row, index) => row === rightRows[index])
}

function sameDump(left, right) {
  if (left === right) return true
  if (!left || !right) return false
  return (
    left.name === right.name &&
    left.path === right.path &&
    left.format === right.format &&
    (left.timescale ?? null) === (right.timescale ?? null) &&
    (left.signalCount ?? 0) === (right.signalCount ?? 0) &&
    sameRange(left.timeRange, right.timeRange)
  )
}

function sameRange(left, right) {
  if (left === right) return true
  if (!left || !right) return false
  return left.start === right.start && left.end === right.end
}

/**
 * Normalize whatever `register()` handed back into a reader the session cache
 * can call: `reader.stateOf(session)`.
 *
 * The registry changed shape inside the range this plugin supports. 0.2.2 hands
 * back an awaitable handle whose `stateOf(session)` reads this unit; 0.2.1 hands
 * back the Cordis effect disposer, and the value has to be read back through
 * `registry.stateOf(session, key)` — which takes the key, not just the session.
 * Reading `stateOf` off the 0.2.1 value throws a TypeError, and a caller that
 * swallows it turns a permanent wiring bug into "this session has no dump".
 *
 * @param {unknown} registration what `ctx.sessionProjections.register()` resolved to
 * @param {{ stateOf?: (session: unknown, key: string) => unknown } | null | undefined} registry
 * @returns {{ stateOf: (session: unknown) => any } | null}
 */
export function projectionReader(registration, registry) {
  const handle = /** @type {{ stateOf?: unknown } | null | undefined} */ (registration)
  if (typeof handle?.stateOf === 'function') {
    return { stateOf: (session) => /** @type {any} */ (handle).stateOf(session) }
  }
  if (typeof registry?.stateOf === 'function') {
    return { stateOf: (session) => registry.stateOf(session, PROJECTION_KEY) }
  }
  return null
}

/**
 * Build the projection definition.
 * @param {{ live: () => { url: string | null, error: string | null } }} options
 */
export function createProjection({ live }) {
  const stampUi = (view) => ({ ...view, ui: live() })

  return {
    key: PROJECTION_KEY,
    stateSchema: viewSchema,
    stateVersion: STATE_VERSION,
    init: () => stampUi(emptyView()),
    /**
     * @param {ReturnType<typeof emptyView>} state
     * @param {{ type: string, seq?: number, data?: any }} event
     */
    apply: (state, event) => {
      const data = event.data
      /** @type {boolean} */
      let failed
      /** @type {unknown} */
      let meta
      if (event.type === 'tool/result') {
        failed = data?.message?.isError === true
        meta = data?.meta
      } else if (event.type === 'tool/ptc-dispatch') {
        // A sub-call inside `run_code` carries the same vocabulary directly on
        // the event data instead of nesting it under `message`.
        failed = data?.isError === true
        meta = data?.meta
      } else {
        return state
      }
      if (failed) return state
      const payload = meta?.[META_KEY]
      if (!payload || payload.version !== META_VERSION || !payload.state) return state
      // Host-local values never come from the log: spread the durable payload
      // first and stamp the live viewer last.
      const next = { ...emptyView(), ...payload.state, ui: live() }
      return sameView(next, state) ? state : next
    },
    wire: {
      viewSchema,
      view: (state) => state,
      // A per-session waveform view is not a list-row summary.
      summary: false,
    },
  }
}
