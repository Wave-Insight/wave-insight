# Changelog

All notable changes to the Wave Insight DSH plugin are documented here. The
format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Plugin
versions are independent of the Wave Insight application version.

## [0.2.10] - 2026-11-20

### Added

- `load_sources` puts sources in the panel's source pane, so the viewer shows
  the same design the design tools read. Call it with `paths`, or with nothing
  to open the companion RTL next to the loaded dump. The opened list is part of
  the durable view (`STATE_VERSION` 5), so a resumed session replays exactly the
  files it had: the client sends one `open_source` command per new file, and the
  iframe reads the text through the loopback backend.
- `wave-insight-web-backend` answers `read_source { path }`: the browser page
  cannot read a local file, so a companion `.v`/`.sv` the host points at travels
  back through `/invoke`. It sits behind the same token, loopback host and
  same-origin checks as every other command.
- The viewer's `wave-web-command` listener handles `open_source` by handing the
  text to the same `Msg::GetVerilog` the file picker uses, so an opened source
  goes through the ordinary design-database and LSP path.

### Changed

- The bundled viewer is rebuilt from the current front end, which the new
  `open_source` path required.

## [0.2.9] - 2026-11-20

### Added

- **The tools can read the Verilog/SystemVerilog behind a waveform.** The viewer
  has parsed sources since Wave 3, but the Host half never reached that library,
  so a model could see the signals and nothing about where they come from. Four
  read-only tools close the gap, and they answer from the same `DesignDb` the
  source pane uses, so the two cannot disagree:
  - `find_sources` lists the companion RTL next to a dump (`rtl/`,
    `generated/`/`verilog/`, in the dump's directory and the two above it).
  - `list_modules` reports every module definition with its file and line, its
    ports (direction, kind, width) and its child instances.
  - `find_declaration` locates one hierarchical signal: the declaration span,
    the module and instance, and every span that drives or reads it.
  - `get_diagnostics` reports what the parser and the elaborator could not
    resolve, with a `file:line:col` span per entry.

  Each takes `paths` (explicit files) or `dump` (find the RTL next to a
  waveform, defaulting to the file the panel shows) and leaves the panel view
  untouched. `dsh-bridge` grew the matching `sources` / `modules` / `declare` /
  `diagnostics` commands.
- The companion-RTL scan moved from `src-tauri` into
  `wave_insight_lib::design::companion` (gated off wasm), so the desktop open
  path and the DSH helper run one rule set instead of two copies.

### Fixed

- `load_waveform` accepts and documents `.fsdb`: the helper has read FSDB
  through Verdi's converter since 0.2.6, but the tool's description and schema
  named only VCD/FST/GHW and steered the model away from a working format.
- The INVALID_ARGS text for a list argument says "paths" instead of "signal
  paths", which the design tools now use.

## [0.2.8] - 2026-11-20

### Fixed

- A resumed session's tools can read the dump the panel is showing again. The
  session cache rebuilt its view from the `waveInsight` projection through
  `register()`'s return value assuming it was the registry handle, but on the
  0.2.1 desktop line that value is the Cordis effect disposer: `stateOf` was
  called on a function, and reader failures were swallowed by an empty `catch`.
  Every `add_signals` / `set_time_range` / `get_changes` call after a Host
  restart answered `NO_WAVEFORM` until `load_waveform` ran again, while the panel
  kept rendering the dump from the log.
  `projectionReader()` now normalizes whatever `register()` resolved to — the
  awaitable handle's `stateOf(session)` on 0.2.2+, the registry's
  `stateOf(session, key)` on 0.2.1 — and a wiring with neither warns once
  instead of failing silently again.
- The regression suite mounts both registry shapes and asserts that a session
  seeded from the log alone restores its dump, its rows and its window, and that
  a missing reader is reported rather than written off as "not ready yet".

### Changed

- The Linux pair targets `x86_64-unknown-linux-musl`: the helper and the viewer
  backend are static, so one build runs on Alpine and on every glibc
  distribution. The matrix no longer pins `ubuntu-22.04` to hold the glibc
  requirement down.

## [0.2.7] - 2026-11-20

### Added

- macOS x64 joins the bundle: the helper and the viewer backend are built for
  Intel Macs too, so `darwin/x64` is a supported row instead of a silent gap.

### Changed

- The Linux and macOS viewer backends now ship with the bundle. Only the Windows
  build was packaged before, so the side panel had no backend anywhere else and
  fell back to its failure message.
- The native binaries are no longer committed. `dsh-plugin/bin/` is ignored by
  git and filled from the `dsh-plugin-binaries` workflow — which now builds the
  helper *and* the backend for all four platforms — and the release job unpacks
  those artifacts instead of relying on whatever was checked in.
  `node dsh-plugin/scripts/binaries.mjs stage` reproduces the layout from a local
  `cargo build`, `verify` (also run by `prepublishOnly`) refuses a package with a
  platform missing, and a test keeps `package.json` "files" and the platform
  matrix in step.
- The Linux builds are linked on Ubuntu 22.04, so they start on distributions
  with an older glibc instead of demanding 2.38.

### Fixed

- The tarball listed four of the eight binaries the Host looks for, so an
  installed package was missing every viewer backend but the Windows one.

## [0.2.6] - 2026-11-20

### Added

- The bundled viewer is the 0.24.0 build: waveform comparison (the Compare
  toolbar with per-slot colours, the coloured signal badges in the SIGNALS list
  and the union member roots) and FSDB traces through Verdi's `fsdb2vcd`.

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
