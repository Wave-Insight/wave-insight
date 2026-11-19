# Changelog

All notable changes to the Wave Insight DSH plugin are documented here. The
format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Plugin
versions are independent of the Wave Insight application version.

## [0.2.5] - 2026-11-19

### Fixed

- The bundled viewer picks up the 0.23.6 renderer fix: a multi bit row that
  changed before the visible time range keeps its band and value label instead
  of staying blank until its next transition.

## [0.2.4] - 2026-11-19

### Added

- The tab chip names the open dump (`Wave Insight · test.vcd`) through the
  `sidebar.right.pane.tab.title` seat, so several waveform tabs are
  distinguishable at a glance.
- `qa/viewer-build-stamp.mjs` plus `dsh-plugin/ui/build.json` record a hash of
  everything that produces the bundled viewer, and CI fails when `ui/` predates
  the front-end sources — the drift that kept the DSH panel on a viewer built
  before the native-dump fix.

### Fixed

- The loopback backend answers `dump_memory_stats` and `process_memory` with
  empty shapes instead of `unknown command`, removing two failed round-trips on
  every opened dump (the viewer's memory readout stays blank).

### Changed

- `cargo fmt` on the native-dump guard; the README documents the viewer
  rebuild-and-stamp flow.

## [0.2.3] - 2026-11-19

### Changed

- The version gate accepts the 0.2.1 desktop line (`engines.dsh:
  ">=0.2.1-rc.5"`). It required `>=0.2.2-rc.3`, and the Desktop app's updater
  offers a 0.2.1-rc.5 build — a lower version that would have blocked the whole
  bundle from loading after the update. The plugin's Host APIs are the same on
  both lines.

## [0.2.2] - 2026-11-19

### Fixed

- The embedded viewer actually renders a dump now. Three independent defects
  each hid the panel behind "Nothing loaded yet":
  - the viewer's empty-state guard only consulted the wasm store, while a
    native/web-remote dump lives in the remote session with no wasm handle, so
    a successfully opened dump was still painted as "nothing loaded";
  - the post-adoption handshake was posted as a JS `Map` (a
    `serde_wasm_bindgen` artifact), so the host's `event.data.type` read was
    undefined and the row/range/cursor replay never ran;
  - the projection could stamp a null viewer URL before the loopback listener
    was up, leaving a resumed session with no way to learn the live one.
- The bundled viewer is rebuilt from the current web-remote front end.

### Added

- The host suite fails on a UTF-8 BOM in `package.json`, a locale file, the
  patch or `ui/index.html`: a BOM made the plugin manager report
  `Unexpected token` and drop the entire bundle from the composition.

## [0.2.1] - 2026-11-19

### Fixed

- The embedded viewer receives its commands. `serde_wasm_bindgen::to_value`
  hands the wasm app a JS `Map` for every argument object, and the shim's
  `JSON.stringify({ cmd, args })` rendered that as `{}`, so `open_dump` always
  failed with "missing field `path`" and the panel stayed on "Nothing loaded yet".
  The shim now normalizes Maps, Sets and typed arrays before the fetch, which
  also fixes `dump_hierarchy`, `dump_window`, `dump_value_at` and `dump_find`.
- The viewer handshake marks its post-mount announce (`boot: true`), and the
  Client opens the dump on that handshake instead of guessing when the wasm app
  is ready. The previous announce at `DOMContentLoaded` raced the app's command
  listener, so the first `open_path` was often dropped.
- The Client replays the whole view (rows, range, cursors) after every
  handshake, because the app clears rows when it adopts a dump.

### Added

- `qa/dsh-invoke-shim.test.mjs` covers the argument serialization, the token
  header and the origin/source checks of the web-remote shim.
- CI runs that suite and watches the shim file.

## [0.2.0] - 2026-11-18

### Fixed

- The bundle activates again. `Config` is now a hand-written Standard Schema:
  the previous `zod` object rejected the `undefined` a config-less row passes to
  the Loader, and on a directory-linked install `import 'zod'` resolved to a 3.x
  copy without `~standard`, so activation threw before any tool registered.
- The panel no longer re-opens the same dump forever. The readiness handshake
  keeps the opened path instead of clearing it, so the row/range/cursor replay
  finally runs instead of looping `open_path` through a full dump reopen.
- A viewer URL from an earlier Host run can no longer reach the iframe: the
  projection strips host-local fields from folded state and stamps the live URL,
  and the panel probes `/health` before rendering.
- Inbound `wave-dsh-ready` messages are checked against the iframe's window and
  origin; the shim only accepts commands from its framing window.
- Tool failures stay real `isError` results with a stable code in the message,
  and cancellation keeps the caller's reason instead of `[object Object]`.
- `get_changes` deduplicates signal paths and no longer mislabels values when
  the model repeats a path; decoded changes are capped to what the model reads.
- `add_signals` resolves every path in one helper call again (with a fallback for
  helpers built before the `validate` command).
- GHW dumps report a real time range (first/last time step) instead of being
  clamped to a one-tick window; real/string/event values render as text instead
  of Rust `Debug` output.
- `remove_signals` and `set_cursors` require a loaded dump, and cursors are
  clamped to the dump range.
- The viewer backend no longer deadlocks on an unread stderr pipe, a failed
  start no longer leaks the child or latches a permanent error, and the plugin's
  cleanup effect is installed before the first `await`.
- Two mounted instances share one viewer backend by refcount instead of the
  first unload killing the other's panel.

### Changed

- Host half split into `host/` modules (assets, bridge, daemon, projection,
  sessions, tools, schemas); the Client half is restructured around a pure
  view-diff function and a bounded, event-driven auto-open.
- Tool results carry `{ waveInsight: { version, state } }`; the view shape now
  matches the panel exactly (`timeRange`, `signalCount`, `ui`), and
  `stateVersion` is 4, so stale checkpoints are discarded.
- The projection accepts no `inheritedEventCount` bookkeeping and no host-local
  fields; `apply` returns the previous state reference when nothing renders
  differently.
- Locale dictionaries are registered inside `ctx.effect` (reload-safe) and the
  unsupported `zh-cn` dictionary is gone.
- Loopback backend: per-process token on `/invoke`, `Host`/`Origin` checks,
  token-free `/health`, `dump_find`, immutable caching for hashed assets,
  `nosniff`, and registry calls moved off the async runtime.
- The UI shim closes its own dumps on `pagehide`, so a reloaded panel no longer
  leaks mappings until the 32-dump cap.
- Packaging: `./package.json` export, `host/` in `files`, honest platform table,
  `engines.dsh`, and no runtime dependencies.

### Removed

- The `zod` dependency.

## [0.1.5] - 2026-11-18

### Changed

- The projection state schema now describes the dump, visible range and
  cursors concretely instead of accepting `z.any`; `stateVersion` is bumped so
  stale checkpoints are discarded.
- The derived per-Session working snapshot is evicted on `session/disposed`,
  so long-running Hosts no longer accumulate one view per Session ever seen.

## [0.1.4] - 2026-11-18

### Added

- `helperPath` and `backendPath` are declared in a validated `Config`.
- The package ships its Apache-2.0 LICENSE and a focused CI workflow that runs
  the Host and backend smoke tests plus both front-end feature checks.

### Changed

- The Host no longer persists decoded waveform values: the embedded viewer
  fetches its own bounded windows from the loopback backend. This removes up to
  ~240 KB of state from every tool result and speeds up `load_waveform`.
- `add_signals` validates paths with a zero-capacity query instead of a cached
  set capped at 500/10 000 signals; signals beyond those limits now work, and
  a dropped row over the 40-row cap is reported.
- The right-sidebar dictionaries register under DSH's built-in `zh` locale id,
  so the Chinese UI is actually translated.

## [0.1.3] - 2026-11-18

### Added

- The sidebar tab now renders the **real Wave Insight viewer** in an iframe
  instead of a hand-written SVG sketch.
- A new `wave-insight-web-backend` crate hosts the built UI and answers the
  front end's waveform queries over one loopback `/invoke` route, backed by the
  native memory-mapped dump registry. The Host starts it automatically and
  publishes its URL through the projection.
- The front end has a `web-remote` feature (mutually exclusive with `tauri`)
  that talks to that backend and accepts one-way view commands from the Host.
- Host view changes are forwarded to the embedded UI: open dump, add/remove
  signals, visible time range and cursors.

### Changed

- The old React/SVG waveform panel was removed from `client.js`.

## [0.1.2] - 2026-11-18

### Added

- `get_changes` now returns bounded `time`/`value` pairs to the model and
  renders them in the tool result. Previously it reported only a change count,
  so an assistant could never read what the waveform actually did.

### Fixed

- Tool failures are real `isError` results with stable codes instead of
  success-looking `Error:` text; session telemetry and the UI now see failures.
- The `waveInsight` projection folds PTC sub-call results
  (`tool/ptc-dispatch`) as well as native `tool/result` events, so waveform
  commands issued from `run_code` update the panel.
- A failed `openTab` is logged and retried; the Session is only marked opened
  after a successful call.
- The helper process now runs asynchronously and honors `exec.signal`, so a
  large dump no longer blocks every other DSH session and cancellation stops
  the helper.
