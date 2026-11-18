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
import { resolveAssets } from '../dsh-plugin/host/assets.js'
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

/**
 * A Cordis-shaped context with one real `waveInsight` projection that can be
 * folded directly, so the tests cover the fold, the restore and the tool
 * results together.
 */
function createHarness() {
  const tools = []
  const effects = []
  const promptSections = []
  const disposed = []
  /** @type {Map<string, { definition: any, states: Map<string, any> }>} */
  const units = new Map()

  const handle = {
    stateOf(session) {
      const unit = units.get('waveInsight')
      if (!unit) return undefined
      const id = String(session?.id ?? 'unknown')
      if (!unit.states.has(id)) unit.states.set(id, unit.definition.init(session?.header ?? {}, 0))
      return unit.states.get(id)
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
        return handle
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

  return { ctx, handle, tools, effects, promptSections, disposed }
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

  await t.test('registers eight tools, one projection and one prompt section', () => {
    assert.deepEqual(
      harness.tools.map((definition) => definition.name).sort(),
      ['add_signals', 'find_signals', 'get_changes', 'list_waveform', 'load_waveform', 'remove_signals', 'set_cursors', 'set_time_range'],
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
