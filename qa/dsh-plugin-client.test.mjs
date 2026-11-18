// Client-half test: loads `dsh-plugin/client.js` behind a fake module table and
// a fake Cordis context, then checks the pure view-diff logic and what `apply`
// registers. No DOM and no React renderer involved.
//
//   node qa/dsh-plugin-client.test.mjs
import assert from 'node:assert/strict'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const root = path.resolve(fileURLToPath(new URL('..', import.meta.url)))

/** @type {any[]} */
const registrations = []
globalThis.window = {
  __ModuleLoader__: {
    load(registration) {
      registrations.push(registration)
    },
  },
  addEventListener() {},
  removeEventListener() {},
}

const fakeReact = {
  createElement: () => null,
  useCallback: (fn) => fn,
  useEffect: () => {},
  useRef: () => ({ current: null }),
  useState: (value) => [value, () => {}],
}
const require = (id) => {
  if (id === 'react') return fakeReact
  throw new Error(`unexpected module ${id}`)
}

await import(`file:///${path.join(root, 'dsh-plugin', 'client.js').replace(/\\/g, '/')}`)

test('client module registers one factory under the package id', () => {
  assert.equal(registrations.length, 1)
  assert.equal(registrations[0].id, '@wave-insight/dsh-plugin')
  assert.equal(typeof registrations[0].factory, 'function')
})

const plugin = registrations[0].factory(require)
const { viewCommands, sameRange, sameCursors, cursorCommand } = plugin.__test

test('view diff turns rows, range and cursors into viewer commands', () => {
  const fresh = viewCommands(null, {
    rows: ['top.clk', 'top.pc'],
    visible: { start: 5, end: 100 },
    cursors: { one: null, two: null },
  })
  assert.deepEqual(fresh, [
    { kind: 'add_signals', signals: ['top.clk', 'top.pc'] },
    { kind: 'set_time_range', start: 5, end: 100 },
  ])

  const unchanged = viewCommands(
    { rows: ['top.clk', 'top.pc'], visible: { start: 5, end: 100 }, cursors: { one: null, two: null } },
    { rows: ['top.pc', 'top.clk'], visible: { start: 5, end: 100 }, cursors: { one: null, two: null } },
  )
  assert.deepEqual(unchanged, [], 'row order alone does not churn the viewer')
})

test('view diff removes rows before adding new ones', () => {
  const commands = viewCommands(
    { rows: ['a', 'b'], visible: { start: 0, end: 10 }, cursors: { one: 3, two: null } },
    { rows: ['b', 'c'], visible: { start: 0, end: 20 }, cursors: { one: 3, two: 9 } },
  )
  assert.deepEqual(commands, [
    { kind: 'remove_signals', signals: ['a'] },
    { kind: 'add_signals', signals: ['c'] },
    { kind: 'set_time_range', start: 0, end: 20 },
    { kind: 'set_cursors', one: { action: 'set', time: 3 }, two: { action: 'set', time: 9 } },
  ])
})

test('view diff never sends open_path and skips an absent range', () => {
  const commands = viewCommands(null, { rows: [], visible: null, cursors: { one: null, two: null } })
  assert.deepEqual(commands, [], 'an empty view needs no commands')
  const withCursor = viewCommands(null, { rows: [], visible: null, cursors: { one: 4, two: null } })
  assert.deepEqual(withCursor, [
    { kind: 'set_cursors', one: { action: 'set', time: 4 }, two: { action: 'clear' } },
  ])
  assert.ok(![...commands, ...withCursor].some((command) => command.kind === 'open_path'))
})

test('range and cursor comparison is value based', () => {
  assert.equal(sameRange({ start: 1, end: 2 }, { start: 1, end: 2 }), true)
  assert.equal(sameRange({ start: 1, end: 2 }, { start: 1, end: 3 }), false)
  assert.equal(sameRange(null, null), true)
  assert.equal(sameRange(null, { start: 1, end: 2 }), false)
  assert.equal(sameCursors({ one: null, two: null }, { one: null, two: null }), true)
  assert.equal(sameCursors({ one: 1 }, { one: 2 }), false)
  assert.deepEqual(cursorCommand(7), { action: 'set', time: 7 })
  assert.deepEqual(cursorCommand(null), { action: 'clear' })
})

test('apply registers dictionaries, the tab type, the body and auto-open in effects', () => {
  /** @type {any[]} */
  const locales = []
  /** @type {any[]} */
  const tabs = []
  /** @type {any[]} */
  const slots = []
  /** @type {Function[]} */
  const cleanups = []
  const signals = /** @type {any[]} */ ([])

  const ctx = {
    effect(execute) {
      const dispose = execute()
      if (typeof dispose === 'function') cleanups.push(dispose)
      return () => {}
    },
    locale: {
      register(namespace, locale) {
        locales.push({ namespace, locale })
        return () => {}
      },
      bind: () => (key) => key,
    },
    sidebarRightTabs: {
      register(definition) {
        tabs.push(definition)
        return () => {}
      },
    },
    slots: {
      inject(name, callback) {
        callback()
        return () => {}
      },
      register(options, Component) {
        slots.push({ options, Component })
        return () => {}
      },
    },
    sessions: { binding: () => undefined },
    sidebarRight: {
      mounted: { getSnapshot: () => undefined, subscribe: () => () => {} },
      openTab() {},
      openTabIn() {},
    },
  }

  plugin.apply(ctx)

  assert.deepEqual(
    locales.map((entry) => entry.locale).sort(),
    ['en', 'zh'],
    'both built-in locales are registered, and only those',
  )
  for (const entry of locales) assert.equal(entry.namespace, 'waveInsight')

  assert.equal(tabs.length, 1)
  assert.equal(tabs[0].id, '@wave-insight/dsh-plugin')
  assert.equal(tabs[0].kind, 'wave-insight')
  assert.equal(tabs[0].priority, 'extension')
  assert.equal(typeof tabs[0].title, 'function')
  assert.equal(tabs[0].guide.length, 1)
  assert.equal(tabs[0].guide[0].id, '@wave-insight/dsh-plugin')

  assert.equal(slots.length, 2, 'the body and the live tab title')
  const body = slots.find((slot) => slot.options.name === 'sidebar.right.pane.tab')
  const title = slots.find((slot) => slot.options.name === 'sidebar.right.pane.tab.title')
  for (const slot of [body, title]) {
    assert.equal(slot.options.key, '@wave-insight/dsh-plugin')
    assert.equal(slot.options.locale, 'waveInsight')
    assert.equal(typeof slot.Component, 'function')
  }

  assert.equal(cleanups.length, 5, 'dictionaries, tab type, body, title and auto-open each own an effect')
  assert.equal(signals.length, 0)
})
