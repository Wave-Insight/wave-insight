// Host plugin body: wire the native bridge, the viewer backend, the session
// projection and the eight tools onto one Cordis fiber.
//
// Activation never fails because an optional piece is missing: the tools work
// without the embedded viewer, and the viewer is reported through the projection
// so the panel can explain itself instead of rendering blank.
import { resolveAssets } from './assets.js'
import { Bridge } from './bridge.js'
import { acquireWebBackend } from './daemon.js'
import { createProjection, projectionReader } from './projection.js'
import { SessionViews } from './sessions.js'
import { createTools } from './tools.js'

export const inject = ['tools', 'sessionProjections', 'systemPrompt']

/** Config fields, and the only two the row may set. */
const CONFIG_FIELDS = ['helperPath', 'backendPath']

/**
 * Row config as a Standard Schema, which is what Cordis validates against.
 *
 * It is written by hand rather than with `zod` on purpose: this package is
 * linked into a profile, so `import 'zod'` resolves to whatever copy is on the
 * parent path (3.x in the Desktop profile) rather than the 4.x copy declared in
 * package.json. The Loader calls `Config['~standard'].validate(config)` and a
 * 3.x schema has no `~standard`, which would fail activation for every row.
 *
 * `undefined` is accepted and normalized to `{}`: the Loader passes
 * `entry.options.config`, which is absent for a row that declares no `config`.
 */
export const Config = {
  '~standard': {
    version: 1,
    vendor: 'wave-insight',
    /** @param {unknown} value */
    validate(value) {
      if (value === undefined || value === null) return { value: {} }
      if (typeof value !== 'object' || Array.isArray(value)) {
        return { issues: [{ message: 'wave-insight: config must be an object', path: [] }] }
      }
      /** @type {Record<string, string>} */
      const config = {}
      for (const field of CONFIG_FIELDS) {
        const entry = /** @type {Record<string, unknown>} */ (value)[field]
        if (entry === undefined || entry === null) continue
        if (typeof entry !== 'string' || entry.trim().length === 0) {
          return { issues: [{ message: `wave-insight: "${field}" must be a non-empty path string`, path: [field] }] }
        }
        config[field] = entry
      }
      return { value: config }
    },
  },
}

const PROMPT_SECTION = [
  'Wave Insight tools open and drive a waveform viewer in the DSH right sidebar.',
  'Start with load_waveform(path); find_signals/add_signals/remove_signals/set_time_range/set_cursors/get_changes/list_waveform act on that dump, and the panel follows every call.',
  'The design tools read the Verilog/SystemVerilog behind the dump: find_sources lists the companion RTL next to it, list_modules reports the module definitions and ports, find_declaration locates a hierarchical signal and the spans that drive and read it, and get_diagnostics reports what did not parse or resolve.',
].join(' ')

/** @param {any} ctx @param {string} message */
function warn(ctx, message) {
  const logger = ctx?.logger
  if (typeof logger?.warn === 'function') {
    logger.warn(message)
    return
  }
  console.warn(message)
}

/**
 * @param {any} ctx
 * @param {{ helperPath?: string, backendPath?: string }} [config]
 */
export async function apply(ctx, config) {
  const assets = resolveAssets(config ?? {})
  const viewer = acquireWebBackend({ asset: assets.backend, uiDir: assets.uiDir, platform: assets.platform })
  // Registered before anything that can await, so an unload during startup
  // still stops the child instead of orphaning it.
  ctx.effect(() => () => viewer.release(), 'wave-insight: viewer backend')

  const bridge = new Bridge(assets.helper, { platform: assets.platform })
  const live = () => ({ url: viewer.daemon.clientUrl, error: viewer.daemon.error })

  // The projection stamps the viewer URL into its initial state, and the shell
  // reads that state before any tool runs. Waiting for the listener here (it is
  // normally up in well under a second) keeps a resumed session's panel from
  // starting with a null URL and no way to learn the live one.
  await viewer.daemon.ready(3000)

  // Whatever `register()` resolves to is not always the handle: the 0.2.1
  // registry returns the Cordis effect disposer, whose only reader is
  // `registry.stateOf(session, key)`. Normalizing here keeps the session cache
  // calling one shape on every Host this package supports.
  /** @type {{ stateOf: (session: unknown) => any } | null} */
  let projection = null
  try {
    const registration = await ctx.sessionProjections.register(createProjection({ live }))
    projection = projectionReader(registration, ctx.sessionProjections)
    if (!projection) throw new Error(`register() resolved to ${typeof registration} without a stateOf(session) reader`)
  } catch (error) {
    warn(ctx, `wave-insight: the session projection could not be registered, so the side panel will not follow tool calls: ${error instanceof Error ? error.message : String(error)}`)
  }

  const sessions = new SessionViews({ projection, live })
  ctx.effect(() => () => sessions.clear(), 'wave-insight: session cache')
  ctx.effect(
    () => ctx.on('session/disposed', (session) => sessions.evict(String(session?.id))),
    'wave-insight: session eviction',
  )

  const tools = createTools({ bridge, sessions, daemon: viewer.daemon })
  ctx.effect(() => {
    const disposers = tools.map((definition) => ctx.tools.register(definition))
    return () => {
      for (const dispose of disposers) dispose()
    }
  }, 'wave-insight: tools')

  if (ctx.systemPrompt?.section) {
    ctx.effect(
      () =>
        ctx.systemPrompt.section({
          name: 'wave-insight',
          order: 50,
          text: PROMPT_SECTION,
        }),
      'wave-insight: prompt',
    )
  }

  // Report a dead viewer once it is known to be dead, without blocking load.
  void viewer.daemon.start().then((url) => {
    if (!url) {
      warn(ctx, `wave-insight: the embedded waveform viewer is unavailable: ${viewer.daemon.error ?? 'unknown reason'}`)
    }
  })
}
