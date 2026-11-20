// Host-half test suite. Runs without DSH: it supplies a fake Cordis context and
// a real in-memory projection, then drives the registered tools against the
// repository fixtures and the bundled native helper.
//
//   node qa/dsh-plugin-smoke.mjs
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import * as plugin from '../dsh-plugin/index.js'
import { PLATFORMS, bundledNames, resolveAssets } from '../dsh-plugin/host/assets.js'
import { WebBackend } from '../dsh-plugin/host/daemon.js'

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))
const dump = path.join(root, 'qa', 'fixtures', 'wave.vcd')

test('manifests are BOM-free and parse', () => {
  // A UTF-8 BOM makes the plugin manager report `Unexpected token` and drop the
  // bundle from the composition, so guard the files an editor or PowerShell
  // rewrite is most likely to save with one.
  for (const file of ['package.json', 'locale/en.json', 'locale/zh.json', 'cordis.patch.yml']) {
    const bytes = fs.readFileSync(path.join(root, 'dsh-plugin', file))
    assert.notEqual(bytes[0], 0xef, `${file} must not start with a UTF-8 BOM`)
    if (file.endsWith('.json')) JSON.parse(bytes.toString('utf8'))
  }
  const manifest = JSON.parse(fs.readFileSync(path.join(root, 'dsh-plugin', 'package.json'), 'utf8'))
  assert.ok(manifest.dsh?.bundle?.patch, 'the bundle patch is declared')
  assert.ok(manifest.dsh?.client, 'the client half is declared')
  assert.equal(manifest.exports['./client'], './client.js')
  assert.ok(manifest.files.includes('host/**/*'), 'the host modules ship')
  assert.match(
    fs.readFileSync(path.join(root, 'dsh-plugin', 'cordis.patch.yml'), 'utf8'),
    /id:\s*wave-insight-dsh/,
    'the row inserts the plugin',
  )
  const index = fs.readFileSync(path.join(root, 'dsh-plugin', 'ui', 'index.html'))
  assert.notEqual(index[0], 0xef, 'ui/index.html must not start with a BOM')
  assert.match(index.toString('utf8', 0, 40), /^<!DOCTYPE html>/i)
})

test('the packaged binaries and the platform matrix are the same list', () => {
  // Nothing under bin/ is committed, so the only thing keeping a released
  // tarball whole is that package.json "files" names exactly the rows of
  // PLATFORMS — a row nobody lists ships no binary, and a listed file nobody
  // builds ships nothing at all.
  const manifest = JSON.parse(fs.readFileSync(path.join(root, 'dsh-plugin', 'package.json'), 'utf8'))
  const declared = manifest.files
    .filter((entry) => entry.startsWith('bin/'))
    .map((entry) => entry.slice('bin/'.length))
  assert.deepEqual([...declared].sort(), [...bundledNames()].sort(), 'files lists every bundled binary, and only those')

  // A released Linux pair is static musl, not a glibc build tied to whichever
  // distribution released it; the matrix and the workflow row have to agree.
  assert.equal(PLATFORMS['linux/x64'].triple, 'x86_64-unknown-linux-musl', 'linux/x64 builds the static musl target')
  assert.match(
    fs.readFileSync(path.join(root, '.github', 'workflows', 'dsh-plugin-binaries.yml'), 'utf8'),
    /target: x86_64-unknown-linux-musl/,
    'the binaries workflow builds the musl target the matrix names',
  )

  const rustArch = { x64: 'x86_64', arm64: 'aarch64' }
  const rustOs = { win32: 'windows', linux: 'linux', darwin: 'darwin' }
  for (const [key, row] of Object.entries(PLATFORMS)) {
    const [platform, arch] = key.split('/')
    assert.ok(row.triple.includes(rustOs[platform]), `${key} builds a ${platform} target (got ${row.triple})`)
    assert.ok(row.triple.startsWith(rustArch[arch]), `${key} builds its own architecture (got ${row.triple})`)
    const stem = platform === 'win32' ? 'windows' : platform === 'darwin' ? 'macos' : platform
    assert.match(row.helper, new RegExp(`^dsh-bridge-${stem}-${arch}(\\.exe)?$`), `${key} helper carries its platform`)
    assert.match(row.backend, new RegExp(`^wave-insight-web-backend-${stem}-${arch}(\\.exe)?$`), `${key} backend carries its platform`)
  }
})

/**
 * A Cordis-shaped context with one real `waveInsight` projection that can be
 * folded directly, so the tests cover the fold, the restore and the tool
 * results together.
 *
 * `legacyRegister` models the 0.2.1 desktop, whose `register()` returns the
 * Cordis effect disposer instead of an awaitable handle: the only reader there
 * is `registry.stateOf(session, key)`, and reading `stateOf` off the returned
 * value throws. Both shapes have to restore a resumed session.
 */
function createHarness({ legacyRegister = false } = {}) {
  const tools = []
  const effects = []
  const promptSections = []
  const disposed = []
  /** Every `(session, key)` the registry was asked for. */
  const registryReads = []
  /** @type {Map<string, { definition: any, states: Map<string, any> }>} */
  const units = new Map()

  /** Read a unit's current state, materializing it like the registry's cellFor. */
  function readUnit(key, session) {
    const unit = units.get(key)
    if (!unit) return undefined
    const id = String(session?.id ?? 'unknown')
    if (!unit.states.has(id)) unit.states.set(id, unit.definition.init(session?.header ?? {}, 0))
    return unit.states.get(id)
  }

  const handle = {
    stateOf(session) {
      return readUnit('waveInsight', session)
    },
    fold(id, event) {
      const unit = units.get('waveInsight')
      const current = unit.states.get(id) ?? unit.definition.init({}, 0)
      const next = unit.definition.apply(current, event)
      unit.states.set(id, next)
      return next
    },
  }

  const ctx = {
    effect(execute, label) {
      const dispose = execute()
      effects.push({ label, dispose })
      return () => {}
    },
    tools: {
      register(definition) {
        tools.push(definition)
        return () => {}
      },
    },
    sessionProjections: {
      async register(definition) {
        units.set(definition.key, { definition, states: new Map() })
        if (legacyRegister) {
          // 0.2.1: the effect disposer. Awaiting it yields a plain function —
          // Cordis's own wrapper resolves to the registered cleanup — so the
          // awaited value has no stateOf and the keyed read is the only way in.
          const settle = () => {}
          const dispose = () => settle
          dispose.then = (onFulfilled, onRejected) => Promise.resolve(settle).then(onFulfilled, onRejected)
          return dispose
        }
        return handle
      },
      stateOf(session, key) {
        registryReads.push([session, key])
        return readUnit(key, session)
      },
    },
    systemPrompt: {
      section(definition) {
        promptSections.push(definition)
        return () => {}
      },
    },
    on(name, handler) {
      if (name === 'session/disposed') disposed.push(handler)
      return () => {}
    },
    logger: { warn() {} },
  }

  return { ctx, handle, tools, effects, promptSections, disposed, registryReads }
}

/** @param {string} [sessionId] */
function makeExec(sessionId = 'smoke') {
  const controller = new AbortController()
  return { agent: { session: { id: sessionId } }, signal: controller.signal, controller }
}

const harness = createHarness()

/**
 * Run one tool and return its canonical value plus the persisted meta payload.
 * @param {string} name
 * @param {object} args
 * @param {ReturnType<typeof makeExec>} [exec]
 */
async function call(name, args, exec = makeExec()) {
  const tool = harness.tools.find((definition) => definition.name === name)
  assert.ok(tool, `tool ${name} is registered`)
  const value = await tool.execute(args, exec)
  return { tool, value, meta: tool.output.presentationMeta(args, value) }
}

/** Fold one tool result into the session projection, native or PTC. */
function fold(meta, id = 'smoke', type = 'tool/result', isError = false) {
  const data =
    type === 'tool/result' ? { message: { isError }, meta } : { isError, meta }
  return harness.handle.fold(id, { type, data })
}

test('config accepts an omitted row config', () => {  // The Loader validates `undefined` when a row declares no `config`; an
  // undefaulted object schema would fail activation for the shipped patch row.
  const omitted = plugin.Config['~standard'].validate(undefined)
  assert.equal(omitted.issues, undefined, 'config-less activation is valid')
  assert.deepEqual(omitted.value, {})
  const given = plugin.Config['~standard'].validate({ helperPath: 'F:/bin/helper.exe' })
  assert.equal(given.issues, undefined)
  assert.equal(given.value.helperPath, 'F:/bin/helper.exe')
  assert.ok(plugin.Config['~standard'].validate({ helperPath: 7 }).issues, 'a non-string path is rejected')
})

test('wave-insight host half', async (t) => {
  await plugin.apply(harness.ctx, {})

  await t.test('registers the waveform and design tools, one projection and one prompt section', () => {
    assert.deepEqual(
      harness.tools.map((definition) => definition.name).sort(),
      [
        'add_signals',
        'find_declaration',
        'find_signals',
        'find_sources',
        'get_changes',
        'get_diagnostics',
        'list_modules',
        'list_waveform',
        'load_waveform',
        'remove_signals',
        'set_cursors',
        'set_time_range',
      ],
    )
    for (const definition of harness.tools) {
      assert.equal(typeof definition.execute, 'function')
      assert.equal(definition.output.schema.type, 'object')
      assert.equal(typeof definition.output.render, 'function')
      assert.equal(typeof definition.output.presentationMeta, 'function')
    }
    assert.equal(harness.promptSections.length, 1)
    assert.equal(harness.promptSections[0].name, 'wave-insight')
  })

  await t.test('a tool that needs a dump fails with a code instead of a fake success', async () => {
    await assert.rejects(
      () => call('add_signals', { signals: ['top.clk'] }),
      (error) => error.code === 'NO_WAVEFORM',
    )
  })

  await t.test('load_waveform publishes the dump, rows and the live viewer URL', async () => {
    const loaded = await call('load_waveform', { path: dump })
    assert.match(loaded.value.summary, /wave\.vcd/)
    assert.equal(loaded.value.state.dump.name, 'wave.vcd')
    assert.equal(loaded.value.state.dump.format, 'VCD')
    assert.equal(loaded.value.state.dump.signalCount, 21)
    assert.deepEqual(loaded.value.state.visible, { start: 5, end: 1600 })
    assert.equal(loaded.value.state.rows.length, 20)
    assert.ok(loaded.value.state.ui.url || loaded.value.state.ui.error, 'the viewer reports a URL or a reason')
    assert.equal(loaded.meta.waveInsight.state.ui.url, null, 'the viewer URL is not persisted to the log')

    const state = fold(loaded.meta)
    assert.equal(state.dump.name, 'wave.vcd')
    assert.deepEqual(state.rows, loaded.value.state.rows)
    assert.equal(state.ui.url, loaded.value.state.ui.url)
  })

  await t.test('a stale viewer URL in the log never reaches the panel', () => {
    const stale = {
      waveInsight: {
        version: 1,
        state: {
          ...harness.handle.stateOf({ id: 'smoke' }),
          ui: { url: 'http://127.0.0.1:1/', error: null },
        },
      },
    }
    const state = fold(stale)
    assert.notEqual(state.ui.url, 'http://127.0.0.1:1/')
    assert.equal(state.ui.url, harness.handle.stateOf({ id: 'smoke' }).ui.url)
  })

  await t.test('the projection ignores failed results', async () => {
    const before = harness.handle.stateOf({ id: 'smoke' })
    const after = fold({ waveInsight: { version: 1, state: { ...before, status: 'should be ignored' } } }, 'smoke', 'tool/ptc-dispatch', true)
    assert.equal(after, before, 'a failed PTC dispatch leaves the state reference untouched')
  })

  await t.test('find_signals resolves hierarchical paths', async () => {
    const found = await call('find_signals', { query: 'clk' })
    assert.match(found.value.summary, /top\.clk/)
    assert.equal(found.value.state, null, 'a pure lookup does not rewrite the view')
  })

  await t.test('add_signals validates, deduplicates and reports unknown paths', async () => {
    const added = await call('add_signals', { signals: ['top.u_cpu16.active', 'top.u_cpu16.active', 'no.such.signal'] })
    assert.match(added.value.summary, /added 1 row/)
    assert.match(added.value.summary, /unknown 1: no\.such\.signal/)
    assert.ok(added.value.state.rows.includes('top.u_cpu16.active'))
    assert.equal(added.value.state.rows.filter((row) => row === 'top.u_cpu16.active').length, 1)
  })

  await t.test('add_signals reports paths that are already shown', async () => {
    const again = await call('add_signals', { signals: ['top.u_cpu16.active'] })
    assert.match(again.value.summary, /added 0 rows, 1 already shown/)
  })

  await t.test('set_time_range swaps and clamps to the dump', async () => {
    const swapped = await call('set_time_range', { start: 200, end: 100 })
    assert.deepEqual(swapped.value.state.visible, { start: 100, end: 200 })
    const clamped = await call('set_time_range', { start: 0, end: 999_999 })
    assert.deepEqual(clamped.value.state.visible, { start: 5, end: 1600 })
    assert.match(clamped.value.summary, /clamped/)
  })

  await t.test('set_cursors clamps and clears', async () => {
    const set = await call('set_cursors', { one: 1200, two: 9_999_999 })
    assert.deepEqual(set.value.state.cursors, { one: 1200, two: 1600 })
    const cleared = await call('set_cursors', { clear: true })
    assert.deepEqual(cleared.value.state.cursors, { one: null, two: null })
    await assert.rejects(() => call('set_cursors', { one: 'later' }), (error) => error.code === 'INVALID_ARGS')
  })

  await t.test('get_changes returns bounded time/value pairs and deduplicates signals', async () => {
    const changes = await call('get_changes', { signals: ['top.clk', 'top.clk'], start: 5, end: 1600, max: 8 })
    assert.equal(changes.value.values.length, 1, 'duplicate paths collapse to one signal')
    assert.ok(changes.value.values[0].changes.length > 0)
    assert.ok(changes.value.values[0].changes.length <= 24)
    assert.equal(typeof changes.value.values[0].changes[0].time, 'number')
    assert.equal(typeof changes.value.values[0].changes[0].value, 'string')
    const rendered = changes.tool.output.render({}, changes.value)
    assert.ok(rendered.length >= 2, 'the model sees the value pairs as text')
    assert.match(rendered[1].text, /top\.clk: \d+=/)
  })

  await t.test('remove_signals reports rows that were not shown', async () => {
    const removed = await call('remove_signals', { signals: ['top.u_cpu16.active', 'not.shown'] })
    assert.match(removed.value.summary, /removed 1 row/)
    assert.match(removed.value.summary, /not shown: not\.shown/)
    assert.ok(!removed.value.state.rows.includes('top.u_cpu16.active'))
  })

  await t.test('list_waveform reports the current view', async () => {
    const listed = await call('list_waveform', {})
    assert.match(listed.value.summary, /wave\.vcd \(VCD\)/)
    assert.match(listed.value.summary, /range/)
  })

  await t.test('malformed arguments fail with INVALID_ARGS, not a broken view', async () => {
    await assert.rejects(
      () => call('set_time_range', { start: 'abc', end: 1 }),
      (error) => error.code === 'INVALID_ARGS',
    )
    await assert.rejects(
      () => call('get_changes', { signals: [] }),
      (error) => error.code === 'INVALID_ARGS',
    )
    // Another session has no dump of its own; the view is per session.
    await assert.rejects(
      () => call('set_time_range', { start: 0, end: 10 }, makeExec('fresh-session')),
      (error) => error.code === 'NO_WAVEFORM',
    )
  })

  await t.test('a missing dump is a helper failure with a code', async () => {
    await assert.rejects(
      () => call('load_waveform', { path: path.join(root, 'qa', 'fixtures', 'does-not-exist.vcd') }),
      (error) => error.code === 'HELPER_ERROR' && /does-not-exist\.vcd/.test(error.message),
    )
  })

  await t.test('cancellation keeps the caller reason instead of becoming a failure', async () => {
    const exec = makeExec()
    const reason = new Error('user cancelled')
    exec.controller.abort(reason)
    await assert.rejects(
      () => call('load_waveform', { path: dump }, exec),
      (error) => error === reason,
    )
  })

  await t.test('session disposal drops the cache and the next call restores from the projection', async () => {
    assert.equal(harness.disposed.length, 1)
    harness.disposed[0]({ id: 'smoke' })
    const listed = await call('list_waveform', {})
    // The durable projection still owns the dump, so a disposed session is
    // rebuilt instead of claiming that nothing is loaded.
    assert.equal(listed.value.state.dump.name, 'wave.vcd')
    assert.equal(listed.value.state.rows.length, 20)
  })

  await t.test('disposal stops the viewer backend', async () => {
    for (const effect of harness.effects) {
      if (typeof effect.dispose === 'function') await effect.dispose()
    }
  })
})

test('the design tools explain the sources behind a waveform', async (t) => {
  // The design half reads Verilog/SystemVerilog through the same library the
  // source pane uses, so a model and the viewer cannot disagree about where a
  // signal is declared. These tools never touch the panel state: they only
  // answer, so `state` stays null and the projection is untouched.
  const local = createHarness()
  await plugin.apply(local.ctx, {})
  const exec = makeExec('design')
  const call = async (name, args) => {
    const tool = local.tools.find((definition) => definition.name === name)
    assert.ok(tool, `tool ${name} is registered`)
    return tool.execute(args, exec)
  }
  const sources = [path.join(root, 'qa', 'fixtures', 'top.sv'), path.join(root, 'qa', 'fixtures', 'cpu.sv')]

  await t.test('list_modules reports definitions, ports and instances', async () => {
    const listed = await call('list_modules', { paths: sources })
    assert.match(listed.summary, /2 modules in 2 file\(s\)/)
    assert.match(listed.summary, /top \(top\.sv:3\)/)
    assert.match(listed.summary, /output logic \[15:0\] result/)
    assert.match(listed.summary, /instances: cpu u_cpu, cpu u_cpu16/)
    assert.equal(listed.state, null, 'a pure lookup does not rewrite the panel view')
  })

  await t.test('find_declaration locates a hierarchical signal and its references', async () => {
    const found = await call('find_declaration', { paths: sources, top: 'top', signal: 'top.u_cpu.pc' })
    assert.match(found.summary, /top\.u_cpu\.pc is declared at .*cpu\.sv:9:30/)
    assert.match(found.summary, /output logic \[7:0\] in cpu \(top\.u_cpu\)/)
    assert.match(found.summary, /driven at .*cpu\.sv:20:12/)
    assert.match(found.summary, /loaded at .*top\.sv:17:10/)
  })

  await t.test('a path the design does not know is an answer, not a failure', async () => {
    const missing = await call('find_declaration', { paths: sources, top: 'top', signal: 'top.nope' })
    assert.match(missing.summary, /top\.nope is not in the design elaborated as top/)
  })

  await t.test('get_diagnostics reports unresolved names and clears on a clean design', async () => {
    const broken = await call('get_diagnostics', { paths: [path.join(root, 'qa', 'fixtures', 'link_errors.sv')], top: 'link_top' })
    assert.match(broken.summary, /2 diagnostics/)
    assert.match(broken.summary, /missing_driver/)
    assert.match(broken.summary, /unresolved-name/)
    const clean = await call('get_diagnostics', { paths: sources, top: 'top' })
    assert.match(clean.summary, /no diagnostics/)
  })

  await t.test('find_sources answers the companion scan, empty or not', async () => {
    const found = await call('find_sources', { path: path.join(root, 'qa', 'fixtures', 'wave.vcd') })
    assert.match(found.summary, /no companion RTL next to wave\.vcd/)
  })

  await t.test('a design tool without sources says what to pass', async () => {
    await assert.rejects(
      () => call('list_modules', {}),
      (error) => error.code === 'NO_SOURCES' || error.code === 'NO_WAVEFORM',
    )
    await assert.rejects(
      () => call('list_modules', { paths: [] }),
      (error) => error.code === 'INVALID_ARGS',
    )
  })

  for (const effect of local.effects) {
    if (typeof effect.dispose === 'function') await effect.dispose()
  }
})
test('a resumed session restores its dump on every registry shape', async (t) => {
  // The 0.2.1 desktop's register() resolves to the Cordis effect disposer, so a
  // reader built from "whatever register() returned" is a function with no
  // stateOf. The tools then answer NO_WAVEFORM for a session whose panel is
  // showing the dump, and no amount of retrying fixes it: the session cache
  // restores from the durable projection on its first call after the restart.
  //
  // The seeded state is a real fixture: the tools go on to drive that dump
  // through the helper.
  const stale = {
    dump: {
      name: 'wave.vcd',
      path: dump,
      format: 'VCD',
      timescale: '1ns',
      signalCount: 21,
      timeRange: { start: 5, end: 1600 },
    },
    rows: ['top.clk', 'top.rst'],
    visible: { start: 100, end: 200 },
    cursors: { one: 10, two: 20 },
    status: 'wave.vcd is loaded',
    ui: { url: 'http://127.0.0.1:1/?t=stale', error: null },
  }

  for (const [shape, options] of [
    ['handle registry (0.2.2+)', {}],
    ['disposer registry (0.2.1)', { legacyRegister: true }],
  ]) {
    await t.test(shape, async () => {
      const local = createHarness(options)
      // Seed the log before the plugin mounts: the load happened in the
      // previous Host process, so no tool call in this one ever sees it.
      await plugin.apply(local.ctx, {})
      local.handle.fold('resumed', {
        type: 'tool/result',
        seq: 0,
        data: { message: { isError: false }, meta: { waveInsight: { version: 1, state: stale } } },
      })

      const exec = makeExec('resumed')
      const listed = await (async () => {
        const tool = local.tools.find((definition) => definition.name === 'list_waveform')
        return tool.execute({}, exec)
      })()
      assert.equal(listed.state.dump?.name, 'wave.vcd', 'the resumed session starts from the durable dump')
      assert.deepEqual(listed.state.rows, stale.rows, 'the restored rows come from the log')
      assert.deepEqual(listed.state.visible, stale.visible, 'the restored window comes from the log')
      assert.notEqual(listed.state.ui?.url, stale.ui.url, 'the stale viewer URL never reaches the panel')

      const added = await (async () => {
        const tool = local.tools.find((definition) => definition.name === 'add_signals')
        return tool.execute({ signals: ['top.u_cpu16.active'] }, exec)
      })()
      assert.ok(added.state.rows.includes('top.u_cpu16.active'), 'a tool can drive the restored dump')

      if (options.legacyRegister) {
        assert.ok(local.registryReads.length > 0, 'the legacy registry is read through stateOf(session, key)')
        for (const [, key] of local.registryReads) assert.equal(key, 'waveInsight', 'the read names the unit key')
      }
      for (const effect of local.effects) {
        if (typeof effect.dispose === 'function') await effect.dispose()
      }
    })
  }
})

test('a wiring that cannot read the projection says so instead of pretending', async (t) => {
  const local = createHarness()
  await plugin.apply(local.ctx, {})
  local.handle.fold('resumed', {
    type: 'tool/result',
    seq: 0,
    data: {
      message: { isError: false },
      meta: {
        waveInsight: {
          version: 1,
          state: {
            dump: { name: 'des.fst', path: 'F:/dumps/des.fst', format: 'FST', timescale: null, signalCount: 1, timeRange: { start: 0, end: 1 } },
            rows: [],
            visible: null,
            cursors: { one: null, two: null },
            status: 'loaded',
            ui: { url: null, error: null },
          },
        },
      },
    },
  })

  const { SessionViews } = await import('../dsh-plugin/host/sessions.js')
  const warnings = []
  const original = console.warn
  console.warn = (message) => warnings.push(String(message))
  try {
    // A projection that is missing, or one that is not a reader at all (the
    // regression: the raw effect disposer), must not fail silently.
    for (const projection of [null, () => {}]) {
      const sessions = new SessionViews({ projection, live: () => ({ url: null, error: null }) })
      const view = sessions.get({ agent: { session: { id: 'resumed' } } })
      assert.equal(view.dump, null, 'no reader means an empty view')
    }
  } finally {
    console.warn = original
  }
  assert.ok(warnings.length > 0, 'the tool path warns about the missing reader at least once')
  for (const effect of local.effects) {
    if (typeof effect.dispose === 'function') await effect.dispose()
  }
})

test('the panel transport: tokenized URL, health probe, shim token', async (t) => {
  const assets = resolveAssets()
  if (!assets.backend.path) {
    t.skip(`no viewer backend for ${assets.platform}/${assets.arch}`)
    return
  }
  const backend = new WebBackend({ asset: assets.backend, uiDir: assets.uiDir, platform: assets.platform })
  try {
    const url = await backend.ready(15_000)
    assert.ok(url, backend.error ?? 'the viewer backend did not start')
    assert.match(url, /^http:\/\/127\.0\.0\.1:\d+\/\?t=[0-9a-f]{32}$/, 'the URL carries the process token')
    const token = new URL(url).searchParams.get('t')

    const health = await fetch(new URL('health', url))
    assert.equal(health.status, 200, 'the panel probes /health before showing the iframe')

    const body = JSON.stringify({ cmd: 'ping', args: {} })
    const headers = { 'content-type': 'application/json' }
    const denied = await fetch(new URL('invoke', url), { method: 'POST', headers, body })
    assert.equal(denied.status, 403, 'a request without the token is refused')

    const allowed = await fetch(new URL('invoke', url), {
      method: 'POST',
      headers: { ...headers, 'x-wave-token': token },
      body,
    })
    assert.equal(allowed.status, 200)
    assert.equal((await allowed.json()).ok, true)

    const shim = await (await fetch(new URL('dsh-invoke.js', url))).text()
    assert.match(shim, /x-wave-token/, 'the served shim sends the token on /invoke')
    assert.match(shim, /wave-dsh-ready/, 'the served shim announces readiness to the panel')
  } finally {
    await backend.close()
  }
})
