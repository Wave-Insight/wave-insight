// Client half: the Wave Insight tab in the right sidebar.
//
// The Host starts a loopback backend that hosts the real Wave Insight viewer and
// publishes its URL through the `waveInsight` session projection; this module
// registers the tab type and iframes that URL. Tool calls change the projection,
// and this module forwards the difference as one-way `wave-dsh` commands.
//
// Two rules keep the panel honest:
//   * the URL is Host-process local, so it is probed before it is trusted — a
//     resumed session can carry a port from a previous run;
//   * a command is only sent while the viewer is known to be listening, tracked
//     by the `wave-dsh-ready` handshake rather than by optimistic timing.
window.__ModuleLoader__.load({
  id: '@wave-insight/dsh-plugin',
  factory(require) {
    const React = require('react')
    const h = React.createElement
    const { useCallback, useEffect, useRef, useState } = React

    const KIND = 'wave-insight'
    const TAB_ID = '@wave-insight/dsh-plugin'
    const PROJECTION_KEY = 'waveInsight'
    const NAMESPACE = 'waveInsight'
    const MESSAGE_TYPE = 'wave-dsh'
    const READY_TYPE = 'wave-dsh-ready'
    /** Failed health probes before the panel gives up on one URL. */
    const HEALTH_ATTEMPTS = 8
    /** Bounded retries for opening the tab before a Session surface exists. */
    const OPEN_ATTEMPTS = 10
    /** Bounded `open_path` retries while the viewer finishes mounting. */
    const OPEN_RETRIES = 5
    const BINDING_ATTEMPTS = 10

    const STRINGS = {
      en: {
        title: 'Wave Insight',
        waiting: 'Waiting for the waveform viewer…',
        connecting: 'Starting the waveform viewer…',
        unavailable: 'The waveform viewer is unavailable',
        emptyHint: 'Ask the AI to load a waveform file.',
      },
      zh: {
        title: 'Wave Insight',
        waiting: '正在等待波形界面…',
        connecting: '正在启动波形界面…',
        unavailable: '波形界面不可用',
        emptyHint: '让 AI 加载一个波形文件。',
      },
    }

    /**
     * Operations that move the viewer from `previous` to `next`.
     *
     * `open_path` is not part of this list: opening replaces the viewer's dump
     * asynchronously and the app announces readiness afterwards, so the caller
     * sends it alone and replays the rest on the next handshake.
     * @param {{rows?: string[], visible?: {start:number,end:number}|null, cursors?: {one:number|null,two:number|null}}} [previous]
     * @param {{rows?: string[], visible?: {start:number,end:number}|null, cursors?: {one:number|null,two:number|null}}} next
     */
    function viewCommands(previous, next) {
      if (!next) return []
      const commands = []
      const before = previous?.rows ?? []
      const after = next.rows ?? []
      const removed = before.filter((row) => !after.includes(row))
      if (removed.length > 0) commands.push({ kind: 'remove_signals', signals: removed })
      const added = after.filter((row) => !before.includes(row))
      if (added.length > 0) commands.push({ kind: 'add_signals', signals: added })
      if (next.visible && !sameRange(previous?.visible, next.visible)) {
        commands.push({ kind: 'set_time_range', start: next.visible.start, end: next.visible.end })
      }
      if (!sameCursors(previous?.cursors, next.cursors)) {
        commands.push({
          kind: 'set_cursors',
          one: cursorCommand(next.cursors?.one),
          two: cursorCommand(next.cursors?.two),
        })
      }
      return commands
    }

    /** @param {unknown} left @param {unknown} right */
    function sameRange(left, right) {
      if (!left || !right) return left === right
      return left.start === right.start && left.end === right.end
    }

    /** @param {{one?:number|null,two?:number|null}} [left] @param {{one?:number|null,two?:number|null}} [right] */
    function sameCursors(left, right) {
      return (left?.one ?? null) === (right?.one ?? null) && (left?.two ?? null) === (right?.two ?? null)
    }

    /** @param {number|null|undefined} time */
    function cursorCommand(time) {
      return typeof time === 'number' ? { action: 'set', time } : { action: 'clear' }
    }

    /**
     * Probe a published viewer URL and return it once it answers.
     * A URL that never answers is reported as `null`, so a stale port from a
     * previous Host run shows "starting" instead of a blank iframe.
     * @param {string|null|undefined} source
     * @returns {string|null}
     */
    function useViewerUrl(source) {
      const [live, setLive] = useState(null)
      useEffect(() => {
        setLive(null)
        if (!source) return undefined
        let cancelled = false
        let timer = null
        let attempts = 0
        const probe = () => {
          attempts += 1
          fetch(new URL('health', source), { cache: 'no-store', credentials: 'omit' })
            .then((response) => {
              if (cancelled) return
              if (!response.ok) throw new Error(`HTTP ${response.status}`)
              setLive(source)
            })
            .catch(() => {
              if (cancelled) return
              if (attempts >= HEALTH_ATTEMPTS) return
              timer = setTimeout(probe, Math.min(4000, 250 * 2 ** attempts))
            })
        }
        probe()
        return () => {
          cancelled = true
          if (timer) clearTimeout(timer)
        }
      }, [source])
      return live
    }

    /**
     * @param {{ useProjection?: (key: string) => any, t?: (key: string) => string }} props
     */
    function WavePanel(props) {
      // `useProjection` is a standard prop of this session-scoped slot.
      const view = typeof props.useProjection === 'function' ? props.useProjection(PROJECTION_KEY) : undefined
      const t = props.t ?? ((key) => STRINGS.en[key] ?? key)
      const source = view?.ui?.url ?? null
      const url = useViewerUrl(source)

      const frameRef = useRef(null)
      const readyRef = useRef(false)
      const openedPathRef = useRef(null)
      const sentRef = useRef(null)
      const retryRef = useRef(null)
      const attemptRef = useRef(0)
      const viewRef = useRef(view)
      viewRef.current = view

      const send = useCallback(
        (command) => {
          const target = frameRef.current?.contentWindow
          if (!url || !target) return
          try {
            target.postMessage({ type: MESSAGE_TYPE, command }, new URL(url).origin)
          } catch (error) {
            console.warn('[wave-insight] could not send a viewer command:', error)
          }
        },
        [url],
      )

      const clearRetry = useCallback(() => {
        if (retryRef.current) {
          clearTimeout(retryRef.current)
          retryRef.current = null
        }
      }, [])

      // The viewer may still be mounting when the iframe's load event fires, in
      // which case an `open_path` is delivered before its command listener
      // exists. Re-open until the viewer answers with a handshake.
      const scheduleOpenRetry = useCallback(
        (path) => {
          clearRetry()
          const attempt = attemptRef.current
          if (attempt >= OPEN_RETRIES) return
          retryRef.current = setTimeout(() => {
            if (readyRef.current || openedPathRef.current !== path) return
            attemptRef.current += 1
            send({ kind: 'open_path', path })
            scheduleOpenRetry(path)
          }, 400 + attempt * 500)
        },
        [clearRetry, send],
      )

      const sync = useCallback(() => {
        const current = viewRef.current
        const path = current?.dump?.path
        if (!path) return
        if (openedPathRef.current !== path) {
          // A new dump: the app opens it asynchronously and announces when it
          // can accept commands, so nothing else is sent until then.
          openedPathRef.current = path
          sentRef.current = null
          readyRef.current = false
          attemptRef.current = 0
          send({ kind: 'open_path', path })
          scheduleOpenRetry(path)
          return
        }
        if (!readyRef.current) return
        for (const command of viewCommands(sentRef.current, current)) send(command)
        sentRef.current = current
      }, [scheduleOpenRetry, send])

      useEffect(() => {
        sync()
      }, [view?.dump?.path, view?.rows, view?.visible, view?.cursors, url, sync])

      useEffect(() => {
        if (!url) return undefined
        const origin = new URL(url).origin
        const onMessage = (event) => {
          if (event.origin !== origin) return
          const frame = frameRef.current
          if (!frame || event.source !== frame.contentWindow) return
          // Older viewer builds posted the handshake as a JS Map (a
          // serde_wasm_bindgen artifact); read both shapes.
          const data = event.data instanceof Map ? Object.fromEntries(event.data) : event.data
          if (data?.type !== READY_TYPE) return
          // The viewer announces twice: once when it has mounted and can take
          // commands (`boot`), and again after it adopts a dump. Only the first
          // needs the dump opened; the second needs the view replayed.
          readyRef.current = true
          attemptRef.current = 0
          clearRetry()
          if (data.boot === true) openedPathRef.current = null
          // Replay the whole view: the app may have just opened a dump and lost
          // the commands that raced the open.
          sentRef.current = null
          sync()
        }
        window.addEventListener('message', onMessage)
        return () => window.removeEventListener('message', onMessage)
      }, [clearRetry, sync, url])

      const onFrameLoad = useCallback(() => {
        readyRef.current = false
        openedPathRef.current = null
        sentRef.current = null
        attemptRef.current = 0
        sync()
      }, [sync])

      useEffect(() => clearRetry, [clearRetry])

      if (!source) {
        return h(
          'div',
          { style: { padding: 16, fontSize: 12, color: 'var(--dsw-alias-label-secondary)' } },
          t('waiting'),
        )
      }
      if (!url) {
        const reason = view?.ui?.error
        return h(
          'div',
          { style: { padding: 16, fontSize: 12, color: 'var(--dsw-alias-label-secondary)' } },
          reason ? `${t('unavailable')}: ${reason}` : t('connecting'),
        )
      }
      return h('iframe', {
        ref: frameRef,
        src: url,
        title: t('title'),
        onLoad: onFrameLoad,
        style: {
          width: '100%',
          height: '100%',
          border: 0,
          display: 'block',
          background: 'var(--dsw-alias-bg-base)',
        },
      })
    }

    /**
     * The tab chip: the product name plus the open dump, so several waveform
     * tabs are distinguishable at a glance.
     * @param {{ useProjection?: (key: string) => any, t?: (key: string) => string }} props
     */
    function WaveTitle(props) {
      const view = typeof props.useProjection === 'function' ? props.useProjection(PROJECTION_KEY) : undefined
      const t = props.t ?? ((key) => STRINGS.en[key] ?? key)
      const name = view?.dump?.name
      return name ? `${t('title')} · ${name}` : t('title')
    }

    /**
     * Open the tab when the on-screen session first carries a dump.
     *
     * Retries are bounded and driven by the projection / mounted-session
     * subscriptions; there is deliberately no page-lifetime polling loop.
     * @param {any} ctx
     */
    function watchForDumps(ctx) {
      /** @type {Set<string>} */
      const opened = new Set()
      /** @type {Set<string>} */
      const warned = new Set()
      /** @type {null | (() => void)} */
      let stopFace = null
      /** @type {ReturnType<typeof setTimeout> | null} */
      let timer = null

      const cancel = () => {
        if (timer) {
          clearTimeout(timer)
          timer = null
        }
      }
      const later = (run, delay) => {
        cancel()
        timer = setTimeout(run, delay)
      }

      /** @param {string} sessionId @param {number} attempt */
      const openTab = (sessionId, attempt) => {
        if (opened.has(sessionId)) return
        try {
          const controller = ctx.sidebarRight
          if (typeof controller.openTabIn === 'function') controller.openTabIn(sessionId, KIND)
          else controller.openTab(KIND)
          opened.add(sessionId)
          return
        } catch (error) {
          if (attempt === 0) console.warn('[wave-insight] openTab failed:', error?.message ?? error)
        }
        if (attempt >= OPEN_ATTEMPTS) {
          if (!warned.has(sessionId)) {
            warned.add(sessionId)
            console.warn('[wave-insight] could not open the sidebar tab; open it from the right-bar guide instead')
          }
          return
        }        later(() => openTab(sessionId, attempt + 1), Math.min(1500, 120 * (attempt + 1)))
      }

      /** @param {string} sessionId @param {number} attempt */
      const watch = (sessionId, attempt) => {
        stopFace?.()
        stopFace = null
        cancel()
        if (!sessionId || opened.has(sessionId)) return
        const face = ctx.sessions?.binding?.(sessionId)?.session?.projections?.faceOf?.(PROJECTION_KEY)
        if (!face) {
          // The borrowed binding can appear just after the Session mounts.
          if (attempt < BINDING_ATTEMPTS) later(() => watch(sessionId, attempt + 1), 200)
          return
        }
        const check = () => {
          if (opened.has(sessionId)) return
          if (!face.getSnapshot?.()?.dump) return
          openTab(sessionId, 0)
        }
        stopFace = face.subscribe?.(check) ?? null
        check()
      }

      const sync = () => watch(ctx.sidebarRight.mounted.getSnapshot(), 0)
      const unsubscribe = ctx.sidebarRight.mounted.subscribe(sync)
      sync()
      return () => {
        unsubscribe?.()
        stopFace?.()
        cancel()
      }
    }

    return {
      inject: ['slots', 'sidebarRight', 'sidebarRightTabs', 'sessions', 'locale'],
      apply(ctx) {
        // Dictionaries are fiber-scoped: disposing the fiber removes them, so a
        // reload cannot fail with "already has locale".
        ctx.effect(() => {
          const disposers = [
            ctx.locale.register(NAMESPACE, 'en', STRINGS.en),
            ctx.locale.register(NAMESPACE, 'zh', STRINGS.zh),
          ]
          return () => {
            for (const dispose of disposers) dispose()
          }
        }, 'wave-insight: dictionaries')
        const t = ctx.locale.bind(NAMESPACE)

        ctx.effect(
          () =>
            ctx.sidebarRightTabs.register({
              id: TAB_ID,
              kind: KIND,
              title: () => t('title'),
              priority: 'extension',
              guide: [
                {
                  id: TAB_ID,
                  order: 900,
                  title: () => t('title'),
                  description: () => t('emptyHint'),
                },
              ],
            }),
          'wave-insight: tab type',
        )

        ctx.effect(
          () =>
            ctx.slots.inject('sidebar.right.pane.tab', () =>
              ctx.slots.register({ name: 'sidebar.right.pane.tab', key: TAB_ID, locale: NAMESPACE }, WavePanel),
            ),
          'wave-insight: panel body',
        )

        ctx.effect(
          () =>
            ctx.slots.inject('sidebar.right.pane.tab.title', () =>
              ctx.slots.register({ name: 'sidebar.right.pane.tab.title', key: TAB_ID, locale: NAMESPACE }, WaveTitle),
            ),
          'wave-insight: tab title',
        )

        ctx.effect(() => watchForDumps(ctx), 'wave-insight: auto-open')
      },
      // Test seam: the pure view-diff helpers, with no runtime effect. The
      // loader ignores unknown exports; `qa/dsh-plugin-client.test.mjs` drives
      // them without a DOM.
      __test: { viewCommands, sameRange, sameCursors, cursorCommand, STRINGS },
    }
  },
})
