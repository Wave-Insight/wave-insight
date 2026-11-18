// Client half: a right-bar tab that follows the `waveInsight` projection.
// React comes from the browser module table; no second React installation.
window.__ModuleLoader__.load({
  id: '@wave-insight/dsh-plugin',
  factory(require) {
    const React = require('react')
    const h = React.createElement
    const { useMemo } = React

    const TAB_ID = '@wave-insight/dsh-plugin'
    const KIND = 'wave-insight'
    const W = 1000
    const ROW_H = 34

    let sidebar = null
    let sessions = null
    let translate = (key) => key
    const opened = new Set()

    function openPanel() {
      if (!sidebar) return
      try {
        sidebar.openTab(KIND)
      } catch {
        // No mounted session yet; the next projection frame will retry.
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
        opened.add(sessionId)
        openPanel()
      }
      const unsubscribe = face.subscribe?.(check)
      check()
      return () => unsubscribe?.()
    }

    function level(value) {
      const text = String(value ?? '')
      if (text === '0') return ROW_H - 6
      if (text === '1') return 6
      if (/^[xzhulw-]/.test(text)) return ROW_H / 2
      let hash = 0
      for (let i = 0; i < text.length; i++) hash = (hash * 31 + text.charCodeAt(i)) | 0
      return 8 + (Math.abs(hash) % (ROW_H - 16))
    }

    function wavePath(changes, start, end) {
      const span = Math.max(1, end - start)
      const x = (time) => Math.max(0, Math.min(W, ((time - start) / span) * W))
      if (!changes || changes.length === 0) return `M0,${ROW_H / 2} L${W},${ROW_H / 2}`
      let path = `M0,${level(changes[0].v)}`
      let y = level(changes[0].v)
      for (const change of changes) {
        const cx = x(change.t).toFixed(1)
        const next = level(change.v)
        path += ` L${cx},${y} L${cx},${next}`
        y = next
      }
      return `${path} L${W},${y}`
    }

    function cursorLines(cursors, start, end) {
      const span = Math.max(1, end - start)
      const items = []
      for (const value of [cursors?.one, cursors?.two]) {
        if (typeof value !== 'number') continue
        const x = ((value - start) / span) * W
        if (x < 0 || x > W) continue
        items.push(
          h('line', {
            key: `cursor-${value}`,
            x1: x,
            x2: x,
            y1: 0,
            y2: ROW_H,
            stroke: 'var(--dsw-alias-state-warn-primary)',
            strokeWidth: 1,
            strokeDasharray: '3 2',
          }),
        )
      }
      return items
    }

    function WaveRow({ row, changes, start, end, cursors }) {
      const path = useMemo(
        () => wavePath(changes, start, end),
        [changes, start, end],
      )
      const labels = (changes || []).slice(0, 4)
      const span = Math.max(1, end - start)
      return h(
        'div',
        {
          style: {
            display: 'flex',
            alignItems: 'stretch',
            minHeight: ROW_H,
            borderBottom: '1px solid var(--dsw-alias-border-l1)',
          },
        },
        h(
          'div',
          {
            title: row,
            style: {
              width: 168,
              minWidth: 168,
              padding: '8px 8px 0 4px',
              overflow: 'hidden',
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
              fontSize: 11,
              color: 'var(--dsw-alias-label-primary)',
              borderRight: '1px solid var(--dsw-alias-border-l1)',
            },
          },
          row,
        ),
        h(
          'svg',
          {
            viewBox: `0 0 ${W} ${ROW_H}`,
            preserveAspectRatio: 'none',
            width: '100%',
            height: ROW_H,
            role: 'img',
            'aria-label': `Waveform of ${row}`,
            style: { display: 'block' },
          },
          h('line', {
            x1: 0,
            x2: W,
            y1: ROW_H - 1,
            y2: ROW_H - 1,
            stroke: 'var(--dsw-alias-border-l1)',
            strokeWidth: 1,
          }),
          h('path', {
            d: path,
            fill: 'none',
            stroke: 'var(--dsw-alias-brand-primary)',
            strokeWidth: 1.6,
            vectorEffect: 'non-scaling-stroke',
          }),
          ...cursorLines(cursors, start, end),
          ...labels.map((change, index) =>
            h(
              'text',
              {
                key: `label-${index}`,
                x: Math.min(W - 60, Math.max(0, ((change.t - start) / span) * W + 3)),
                y: 10,
                fill: 'var(--dsw-alias-label-secondary)',
                fontSize: 10,
              },
              String(change.v).slice(0, 12),
            ),
          ),
        ),
      )
    }

    function WavePanel({ useProjection }) {
      const wave = useProjection ? useProjection('waveInsight') : undefined
      if (!wave) {
        return h(
          'div',
          { style: { padding: 16, color: 'var(--dsw-alias-label-secondary)' } },
          translate('waiting'),
        )
      }
      const dump = wave.dump
      const visible = wave.visible ?? dump?.time_range ?? { start: 0, end: 1 }
      const start = Math.floor(visible.start)
      const end = Math.floor(visible.end)
      const span = Math.max(1, end - start)
      return h(
        'div',
        {
          style: {
            display: 'flex',
            flexDirection: 'column',
            height: '100%',
            boxSizing: 'border-box',
            overflow: 'auto',
            background: 'var(--dsw-alias-bg-base)',
            color: 'var(--dsw-alias-label-primary)',
            fontFamily: 'var(--dsw-font-default, inherit)',
          },
        },
        h(
          'div',
          {
            style: {
              padding: '12px 12px 10px',
              borderBottom: '1px solid var(--dsw-alias-border-l2)',
              background: 'var(--dsw-alias-bg-layer-1)',
            },
          },
          h(
            'div',
            { style: { fontSize: 13, fontWeight: 600, marginBottom: 4 } },
            dump ? `${dump.name} · ${dump.format}` : translate('title'),
          ),
          h(
            'div',
            { style: { fontSize: 11, color: 'var(--dsw-alias-label-secondary)' } },
            dump
              ? `${dump.signal_count} signals · ${dump.timescale ?? 'time steps'} · ${start}–${end}`
              : translate('emptyHint'),
          ),
          h(
            'div',
            { style: { fontSize: 11, color: 'var(--dsw-alias-label-secondary)', marginTop: 4 } },
            wave.status,
          ),
        ),
        dump
          ? h(
              'div',
              null,
              h(
                'div',
                {
                  style: {
                    display: 'flex',
                    alignItems: 'center',
                    padding: '6px 4px',
                    borderBottom: '1px solid var(--dsw-alias-border-l1)',
                    fontSize: 10,
                    color: 'var(--dsw-alias-label-secondary)',
                  },
                },
                h('span', { style: { width: 168, minWidth: 168 } }, translate('signals')),
                h('span', null, `${start}`),
                h('span', { style: { flex: 1 } }),
                h('span', null, `${end}`),
              ),
              ...wave.rows.map((row) =>
                h(WaveRow, {
                  key: row,
                  row,
                  changes: wave.waves?.[row] ?? [],
                  start,
                  end,
                  cursors: wave.cursors,
                }),
              ),
              wave.rows.length === 0
                ? h(
                    'div',
                    { style: { padding: 16, fontSize: 12, color: 'var(--dsw-alias-label-secondary)' } },
                    translate('noRows'),
                  )
                : null,
            )
          : null,
      )
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
              waiting: 'Waiting for the waveform projection…',
              emptyHint: 'Ask the AI to load a waveform file.',
              signals: 'Signals',
              noRows: 'No signals on the wave yet. Ask the AI to add some.',
            },
            'zh-cn': {
              title: 'Wave Insight',
              waiting: '正在等待波形数据…',
              emptyHint: '让 AI 加载一个波形文件。',
              signals: '信号',
              noRows: '波形上还没有信号，请让 AI 添加。',
            },
          }
          ctx.locale.register('waveInsight', 'en', strings.en)
          ctx.locale.register('waveInsight', 'zh-cn', strings['zh-cn'])
          translate = ctx.locale.bind('waveInsight')
        } catch {
          translate = (key) =>
            ({
              title: 'Wave Insight',
              waiting: 'Waiting for the waveform projection…',
              emptyHint: 'Ask the AI to load a waveform file.',
              signals: 'Signals',
              noRows: 'No signals on the wave yet. Ask the AI to add some.',
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
                WavePanel,
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
      },
    }
  },
})
