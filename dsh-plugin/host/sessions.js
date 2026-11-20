// Per-session working view.
//
// The durable truth is the session log (folded into the `waveInsight`
// projection); this map is only the latest decoded snapshot, so a tool call
// does not fold the log again. It is rebuilt from the projection when a session
// first touches a tool after a Host restart, and dropped when the session is
// disposed.
import { emptyView } from './projection.js'
import { waveError } from './errors.js'

/** @type {Set<string>} */
const warned = new Set()

/**
 * Warn about a wiring failure once per process. The restore path runs on every
 * tool call of every session, so an unconditional warning would bury the log.
 * @param {string} message
 */
function warnOnce(message) {
  if (warned.has(message)) return
  warned.add(message)
  console.warn(message)
}

/** Copy the parts of a view that a tool may replace later; never alias the projection value. */
function cloneView(view) {
  return {
    dump: view.dump ? { ...view.dump, timeRange: view.dump.timeRange ? { ...view.dump.timeRange } : null } : null,
    rows: [...(view.rows ?? [])],
    visible: view.visible ? { ...view.visible } : null,
    cursors: { one: view.cursors?.one ?? null, two: view.cursors?.two ?? null },
    status: view.status ?? emptyView().status,
    ui: { url: view.ui?.url ?? null, error: view.ui?.error ?? null },
  }
}

export class SessionViews {
  /** @type {Map<string, ReturnType<typeof emptyView>>} */
  #cache = new Map()

  /**
   * @param {{
   *   projection: { stateOf: (session: unknown) => any } | null,
   *   live: () => { url: string | null, error: string | null },
   * }} options
   * The projection is a reader built by `projectionReader()`: whatever the
   * registry's `register()` resolved to, normalized to `stateOf(session)`.
   */
  constructor(options) {
    this.projection = options.projection
    this.live = options.live
  }

  get size() {
    return this.#cache.size
  }

  /**
   * The executing session's key. Tool calls always carry an Agent bound to a
   * Session; an agentless call is a wiring mistake rather than a reason to
   * share one view across every session.
   * @param {{ agent?: { id?: unknown, session?: { id?: unknown } } }} exec
   * @returns {string}
   */
  keyOf(exec) {
    const sessionId = exec?.agent?.session?.id
    if (sessionId !== undefined && sessionId !== null) return String(sessionId)
    const agentId = exec?.agent?.id
    if (agentId !== undefined && agentId !== null) return `agent:${String(agentId)}`
    throw waveError('NO_SESSION', 'this tool needs an agent session; nothing was dispatched to the waveform viewer')
  }

  /**
   * Current view for a session, restored from the projection on first use.
   * @param {{ agent?: any }} exec
   */
  get(exec) {
    const key = this.keyOf(exec)
    const cached = this.#cache.get(key)
    if (cached) {
      const stamped = this.#withLive(cached)
      if (stamped !== cached) this.#cache.set(key, stamped)
      return stamped
    }
    const restored = this.#restore(exec) ?? emptyView()
    const stamped = this.#withLive(restored)
    this.#cache.set(key, stamped)
    return stamped
  }

  /**
   * Replace the session's view with the next state a tool computed.
   * @param {{ agent?: any }} exec
   * @param {ReturnType<typeof emptyView>} next
   */
  update(exec, next) {
    const key = this.keyOf(exec)
    const stamped = this.#withLive({ ...emptyView(), ...next })
    this.#cache.set(key, stamped)
    return stamped
  }

  /** @param {string} sessionId */
  evict(sessionId) {
    this.#cache.delete(String(sessionId))
  }

  clear() {
    this.#cache.clear()
  }

  /** @param {ReturnType<typeof emptyView>} view */
  #withLive(view) {
    const ui = this.live()
    if (view.ui?.url === ui.url && view.ui?.error === ui.error) return view
    return { ...view, ui }
  }

  /**
   * The durable view this session's log folded to, or null when it never
   * loaded a dump.
   *
   * A projection the wiring could not build is a permanent condition, not a
   * transient one: saying so once is what keeps "the tools cannot see the dump
   * the panel is showing" from being rediscovered by every resumed session.
   * @param {{ agent?: any }} exec
   */
  #restore(exec) {
    const session = exec?.agent?.session
    if (!session) return null
    if (typeof this.projection?.stateOf !== 'function') {
      warnOnce(
        'wave-insight: no session projection reader, so a resumed session starts with an empty view and the tools answer NO_WAVEFORM until load_waveform runs again',
      )
      return null
    }
    const durable = this.projection.stateOf(session)
    if (!durable?.dump?.path) return null
    return cloneView(durable)
  }
}
