// Client half: a right-bar tab hosting the real Wave Insight UI.
// The Host starts a loopback web backend, publishes its URL through the
// `waveInsight` projection, and this component renders that UI in an iframe.
// View changes from Host tools are forwarded as one-way commands.
// React comes from the browser module table; no second React installation.
window.__ModuleLoader__.load({
  id: '@wave-insight/dsh-plugin',
  factory(require) {
    const React = require('react')
    const h = React.createElement
    const { useEffect, useRef } = React

    const TAB_ID = '@wave-insight/dsh-plugin'
    const KIND = 'wave-insight'

    let sidebar = null
    let sessions = null
    let translate = (key) => key
    const opened = new Set()
    const pending = new Set()
    let warned = false

    function openPanel() {
      if (!sidebar) return false
      try {
        sidebar.openTab(KIND)
        warned = false
        return true
      } catch (error) {
        // openTab throws when no Session surface is mounted yet. Keep the
        // failure visible and leave the Session pending so the retry loop can
        // open the tab later instead of silently never showing the panel.
        if (!warned) {
          console.warn('[wave-insight] could not open the tab yet, retrying:', error)
          warned = true
        }
        return false
      }
    }

    function watchSession(sessionId) {
      if (!sessionId || opened.has(sessionId)) return () => {}
      const binding = sessions?.binding?.(sessionId)
      const face = binding?.session?.projections?.faceOf?.('waveInsight')
      if (!face) return () => {}
      const check = () => {
        const value = face.getSnapshot?.()
        if (!value?.dump || opened.has(sessionId)) return
        pending.add(sessionId)
        if (openPanel()) {
          opened.add(sessionId)
          pending.delete(sessionId)
        }
      }
      const unsubscribe = face.subscribe?.(check)
      check()
      return () => unsubscribe?.()
    }

    function NativePanel({ useProjection }) {
      const wave = useProjection ? useProjection('waveInsight') : undefined
      const frame = useRef(null)
      const ready = useRef(false)
      const last = useRef(null)
      const view = useRef(wave)
      view.current = wave

      function send(command) {
        const url = view.current?.uiUrl
        const target = frame.current?.contentWindow
        if (!url || !target) return
        try {
          target.postMessage({ type: 'wave-dsh', command }, new URL(url).origin)
        } catch (error) {
          console.warn('[wave-insight] could not send a view command:', error)
        }
      }

      function pushView(value) {
        if (!value?.dump?.path) return
        const previous = last.current
        if (!previous || previous.dump?.path !== value.dump.path) {
          // Opening is async in the app; the rest of the view is replayed when
          // the app reports that the new dump has been adopted.
          ready.current = false
          send({ kind: 'open_path', path: value.dump.path })
          last.current = value
          return
        }
        if (!ready.current) return
        const rowsBefore = previous.rows ?? []
        const removed = rowsBefore.filter((row) => !value.rows.includes(row))
        if (removed.length) send({ kind: 'remove_signals', signals: removed })
        const added = value.rows.filter((row) => !rowsBefore.includes(row))
        if (added.length) send({ kind: 'add_signals', signals: added })
        if (
          !previous.visible ||
          previous.visible.start !== value.visible?.start ||
          previous.visible.end !== value.visible?.end
        ) {
          send({ kind: 'set_time_range', start: value.visible.start, end: value.visible.end })
        }
        if (JSON.stringify(previous.cursors) !== JSON.stringify(value.cursors)) {
          const cursor = (time) =>
            typeof time === 'number' ? { action: 'set', time } : { action: 'clear' }
          send({
            kind: 'set_cursors',
            one: cursor(value.cursors?.one),
            two: cursor(value.cursors?.two),
          })
        }
        last.current = value
      }

      useEffect(() => {
        const onMessage = (event) => {
          if (event.data?.type !== 'wave-dsh-ready') return
          ready.current = true
          last.current = null
          pushView(view.current)
        }
        window.addEventListener('message', onMessage)
        return () => window.removeEventListener('message', onMessage)
      }, [])

      useEffect(() => {
        pushView(wave)
      }, [wave?.dump?.path, wave?.rows, wave?.visible, wave?.cursors, wave?.uiUrl])

      if (!wave?.uiUrl) {
        return h(
          'div',
          { style: { padding: 16, fontSize: 12, color: 'var(--dsw-alias-label-secondary)' } },
          wave?.uiError ? `Wave Insight UI unavailable: ${wave.uiError}` : translate('waiting'),
        )
      }
      return h('iframe', {
        ref: frame,
        src: wave.uiUrl,
        title: translate('title'),
        style: {
          width: '100%',
          height: '100%',
          border: 0,
          display: 'block',
          background: 'var(--dsw-alias-bg-base)',
        },
      })
    }

    return {
      inject: ['slots', 'sidebarRight', 'sidebarRightTabs', 'sessions', 'locale'],
      apply(ctx) {
        sidebar = ctx.sidebarRight
        sessions = ctx.sessions
        try {
          const strings = {
            en: {
              title: 'Wave Insight',
              waiting: 'Waiting for the waveform UI…',
              emptyHint: 'Ask the AI to load a waveform file.',
            },
            zh: {
              title: 'Wave Insight',
              waiting: '正在启动波形界面…',
              emptyHint: '让 AI 加载一个波形文件。',
            },
          }
          // DSH's built-in locale ids are `en` and `zh`; `zh-cn` is also
          // registered for browsers that report a regional Chinese tag.
          strings['zh-cn'] = strings.zh
          ctx.locale.register('waveInsight', 'en', strings.en)
          ctx.locale.register('waveInsight', 'zh', strings.zh)
          ctx.locale.register('waveInsight', 'zh-cn', strings['zh-cn'])
          translate = ctx.locale.bind('waveInsight')
        } catch {
          translate = (key) =>
            ({
              title: 'Wave Insight',
              waiting: 'Waiting for the waveform UI…',
              emptyHint: 'Ask the AI to load a waveform file.',
            })[key] ?? key
        }

        ctx.effect(
          () =>
            ctx.sidebarRightTabs.register({
              id: TAB_ID,
              kind: KIND,
              title: () => translate('title'),
              priority: 'extension',
              guide: [
                {
                  id: TAB_ID,
                  order: 900,
                  title: () => translate('title'),
                  description: () => translate('emptyHint'),
                },
              ],
            }),
          'wave-insight: tab type',
        )

        ctx.effect(
          () =>
            ctx.slots.inject('sidebar.right.pane.tab', () =>
              ctx.slots.register(
                { name: 'sidebar.right.pane.tab', key: TAB_ID },
                NativePanel,
              ),
            ),
          'wave-insight: panel body',
        )

        // Open the tab when a projection first carries a dump. This avoids
        // replacing the host's tool-call cards with a custom tool view.
        ctx.effect(() => {
          let stopWatching = () => {}
          const sync = () => {
            stopWatching()
            pending.clear()
            const sessionId = sidebar?.mounted?.getSnapshot?.()
            stopWatching = watchSession(sessionId)
          }
          const unsubscribe = sidebar?.mounted?.subscribe?.(sync)
          sync()
          return () => {
            unsubscribe?.()
            stopWatching()
          }
        }, 'wave-insight: auto-open')

        // A failed openTab is retried on the mounted Session until it succeeds.
        ctx.effect(() => {
          const retry = () => {
            const sessionId = sidebar?.mounted?.getSnapshot?.()
            if (!sessionId || opened.has(sessionId)) return
            if (!pending.has(sessionId)) {
              const face = sessions?.binding?.(sessionId)?.session?.projections?.faceOf?.('waveInsight')
              if (!face?.getSnapshot?.()?.dump) return
              pending.add(sessionId)
            }
            if (!openPanel()) return
            opened.add(sessionId)
            pending.delete(sessionId)
          }
          if (ctx.timer?.interval) return ctx.timer.interval(retry, 500)
          const id = setInterval(retry, 500)
          return () => clearInterval(id)
        }, 'wave-insight: open retry')
      },
    }
  },
})
