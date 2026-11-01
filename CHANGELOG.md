# Changelog

All notable changes to Wave Insight are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): releases are tagged
`vMAJOR.MINOR.PATCH`, and `BREAKING` marks a release that drops a public API or
an on-disk/transport format.

## [Unreleased]

Nothing yet: the next wave lands here.

## [0.9.0] - 2026-11-02

### Added

- Find value on the time ruler: the box accepts an integer (jump cursor 1 to that
  time, the old goto) or any other text (a Verdi-style value search). Enter jumps
  to the next change of the selected row whose formatted value matches,
  Shift+Enter to the previous one; matching uses the value column's formatting
  (fixed point and x/z included) and is case-insensitive. The status line reports
  `found <value> in <row> at t=... (n/total)`, and a miss is an error status.
  Covered by a new `findValue` QA scenario; the matrix now has 46 scenarios and the
full run is green: **330 passed, 0 failed** with the R2 layout rules required.
- Fast dump open: the VCD time range is now O(header + tail) - a bounded forward
  scan for the first timestamp (16 KiB chunks, 1 MiB budget) and a growing tail
  scan for the last one (16 KiB up to 4 MiB), each falling back to the exhaustive
  scan when the bounded window cannot decide. The 8.3 MB synthetic benchmark
  dropped from 13.602 ms to 684.5 us for the first `time_range_hint` call
  (~19.9x); the exhaustive fallback keeps the answer exact.
- R2 step 3 (architecture, no user-visible change): the store now owns the open
  sources (name/text/language/version/active tab, a versioned slice), the
  inspector focus (`focus.signal`) and jump requests (`nav.goto` +
  `jump_token`). The `Effect` enum is real (`LspOpen`, `LspChange`,
  `LspClose`, `LspTop`, `MonacoShow`) and `App` executes it through
  `LspBridge`; `App.verilog_source`, the selected signal and the shell-owned goto
  token are gone (the shell only mirrors the store's `focus.signal` and
  `nav.goto` for rendering). The language server's document table is a mirror of
  `State.sources` kept in step by the open/change/close effects, so
  `window.__waveInsightLsp.stats().documents` still reports the open-document
  count the QA matrix asserts.
- Migration plans for the two dependency jumps: `docs/migrations/yew-0.21.md`
  and `docs/migrations/tauri-2.md` (current/target versions, what the app
  actually uses, breaker list, steps, files and verification).

### Changed

- The ruler box is now `time / value`: an integer still goes to that time, any
  other text starts a value search on the selected row (see Added).
- Opening a dump no longer scans the whole VCD body for the time range (header
  scan plus the growing tail scan above, exhaustive fallback below).
- Decoding a batch of signals reserves the change buffer up front: the
  `decode/sequential_8` benchmark went from 13.142 ms to 11.759 ms (-10.5%).
- The lockfile got a semver-compatible `cargo update` (~35 bumps, e.g.
  wasm-bindgen 0.2.130, js-sys/web-sys 0.3.107, tokio 1.54.1, icu 2.4) while yew
  and tauri stay put for their planned migrations; the audit gate and the
  release-profile test job stay green.
- CI: publishing to gh-pages moved out of the build job into its own deploy job
  that is the only one with a write token (the build job is read-only and uploads
  the bundle as an artifact); `tauri_pre` got a concurrency group; the action
  pinning policy is documented in the workflow.

### Fixed

- VCD timestamps that share a line with a value (a "#" token mid-line) are now
  accepted by the time-range scanner instead of being missed.

## [0.8.0] - 2026-11-02

### Added

- The language server merges elaboration diagnostics with the design-database
  diagnostics of every open document: `width-mismatch`, `unresolved-name`,
  an unresolved `.*` wildcard or implicit `.name` connection, and non-constant
  generate bounds are deduplicated and published through
  `textDocument/publishDiagnostics`, and they are republished when the top module
  changes. The editor's monaco markers and
  the top bar's problem chip pick them up through the existing bridge. Covered by
  the server's diagnostics tests and the stdio transport tests.
- Criterion benchmarks for the waveform hot paths
  (`wave-insight-lib/benches/waveform.rs`; six groups / twenty benchmarks: open,
  time_range_hint, decode, value_at, window, render). Large inputs are generated
  in memory, so no multi-megabyte dump is committed.
  `.github/workflows/bench.yml` is advisory on purpose: it compiles the suite on a
  weekly schedule and, on manual dispatch, runs the hot-path groups
  (`open`, `decode`, `time_range_hint`, `value_at`) and uploads the criterion
  HTML report.
- A supply-chain gate: `deny.toml` carries the policy (advisories, licences,
  bans, source allow-list) with three reasoned ignores for unmaintained build-time
  or transitive `tauri` 1.x crates, and `.github/workflows/audit.yml` runs
  `cargo deny check` weekly and on every push to `main` as a hard gate.
  Dependabot now opens grouped weekly cargo/actions PRs, with major bumps of
  `tauri`, `tauri-build` and `yew` held until their migration. The Rust CI also
  runs the host tests once in the release profile.
- Session restore: the wave row list is remembered per dump identity (file name
  plus byte size), up to 20 dumps and 200 rows per dump with the least recently
  opened dump dropped first. Reopening the same dump restores its rows in order
  with their display settings; remembered rows whose signal is gone are skipped
  and the load status says how many. Rows added before any dump is open are not
  attributed, and a same-name file whose size is unknown falls back to matching
  on the name alone.
- MAT export: the row settings dialog gained `MAT (row)`, which writes a MATLAB
  level-5 v5 file with two n x 1 double columns, `wave_time` and
  `sig_<sanitized name>`. x/z bits become `NaN`, and signed/fixed-point rows use
  the same numeric values as the value column. MAT is one row at a time; the
  all-rows button stays CSV.
- The browser QA suite gained a `session` scenario (write rows, reload,
  re-upload the same dump, assert the rows come back), a MAT-export check,
  and now has 45 scenarios; the full matrix is green: **324 passed, 0 failed**
  with the R2 layout rules required.

### Changed

- Persistence covers the row list now, not just the window layout and the
  per-signal display settings: `wave-insight.sessions.v1` holds one entry per
  dump identity (LRU, capped), restored by the reducer inside
  `Action::DumpOpened`.
- The state store gained a `sessions` slice for the remembered row lists. The
  elaboration-diagnostics merge is not store state: it happens in the language
  server and the store only carries the resulting problem count as before.

### Fixed

- RUSTSEC-2026-0384: tokio 1.54.0 (yanked; a race in `tokio::time::Interval`) was
  surfaced by the new `cargo deny` gate and fixed by moving the lockfile to
  tokio 1.54.1. `cargo deny --all-features check` reports advisories, bans,
  licences and sources clean.

## [0.7.0] - 2026-11-02

### Added

- Generate elaboration: generate regions, constant-bound generate-for loops and
  generate-if/case branches are unrolled at parse time. Named blocks keep their
  labels, anonymous blocks become `genblk1`, ... and each iteration gets a
  `g[2]`-style scope, so generated instances/signals get flat Verdi-style paths
  (`top.g[2].u`) with their source spans and driver/load links intact.
- Arrayed instances (`mod u[3:0]`) are expanded during elaboration to one
  instance per element in Verdi order (`u[3]`, `u[2]`, ...), with the array
  bounds re-evaluated per instance so parameter overrides are honoured.
- Driver/load diagnostics instead of silent drops: an unresolved simple name, an
  unresolved `.*` wildcard or implicit `.name` connection, and a port connected
  to a plain signal of a different width now become `unresolved-name` /
  `width-mismatch` diagnostics. New fixtures and integration tests cover
  generate, arrayed instances and the link diagnostics.
- Sidebar virtualization: the module tree and signal list render only the rows
  inside their scroll viewport above 80 rows (lists of 80 or fewer keep the old
  DOM and selection behaviour). A `window.__waveInsightSidebar` debug seam
  carries the full logical lists for QA.
- Session persistence: the window layout (sidebar width, editor ratio, drawer)
  and per-signal display settings are stored in versioned `localStorage`
  entries (`wave-insight.layout.v1`, `wave-insight.rows.v1`, schema 1). Corrupt,
  future-version or denied storage degrades to defaults; row settings are keyed
  by hierarchical path and capped to the 2000 most recently changed signals, and
  only committed changes are written (a splitter drag writes once, on mouseup).
- Accessibility and keyboard pass: the time ruler is tier-A draggable (one store
  commit on mouseup), `Escape` closes dialogs and the `go to` box without
  clearing the row selection, dialogs are labelled `role="dialog"` modals with
  `aria-modal`, a Tab focus trap and focus returned to the opener; arrow keys and
  `Home`/`End` move the row selection, `ctrl+W`/`ctrl+F4` closes the active
  source tab, focus rings are visible, tab closers are real buttons, and the wave
  list and candidate picker expose `listbox`/`option` semantics.
- The tauri file browser starts in the user's home directory (`USERPROFILE` /
  `HOME`) instead of the process working directory, which is System32 when the
  desktop app is started from a shortcut.
- A data-URI favicon, so browsers stop probing `/favicon.ico` (a 404 console
  error on every page load).
- The browser QA suite grew three scenarios (`virtualization`, `persistence`,
  `a11y`), 44 in total, and the full matrix is green: **317 passed, 0 failed**
  with the R2 layout rules required. `boot()` clears `localStorage` between
  scenarios because persistence made the matrix order-dependent.

### Changed

- Generate bounds/conditions are resolved while parsing, from the module's
  default parameter values; a per-instance override that would reshape a generate
  is not reflected (arrayed-instance bounds are evaluated per instance and do
  honour overrides). There is no generate node in the instance tree - paths are
  flattened - and non-constant generate bounds stay warnings.
- The state store gained a `row_prefs` slice (remembered per-signal settings)
  and an optional persister; the reducer applies a remembered setting when a row
  is added and records committed changes, and the store writes only the slices
  that changed.
- Elaboration diagnostics are reported through the design API and tests only for
  now; they are not yet forwarded through `textDocument/publishDiagnostics`, so
  the editor's squiggles and the top bar still show parser/front-end diagnostics.

### Fixed

- The tauri file dialog no longer opens in System32 / the app directory.
- The per-load `/favicon.ico` 404 (and its console error) is gone.

## [0.6.0] - 2026-11-02

### Added

- CI gate: the Rust workflow runs the workspace tests, the front end's native
  tests, `cargo fmt --check` and `cargo clippy -D warnings` on every push and
  pull request, with a wasm size budget (10 MiB) so the bundle cannot silently
  grow again.
- Fixed-point display: a row's settings dialog can show its values as a fixed-
  point number with a configurable exponent.
- Signal search options: match case, whole word and regular-expression toggles in
  the sidebar search box.
- Language-server resolution on both halves of a named port connection
  (`.port(expression)`), on instance names and on the instantiated module name;
  covered by a new integration test.
- Library hardening: a value change whose time index falls outside the body's
  time table is now reported as a corrupt-input error instead of panicking while
  decoding; the reader docs state the best-effort panic-recovery contract
  (`panic = "abort"` in release means a dependency panic still aborts).
- Documentation: the README, the R2 store design and the rebuild plan now state
  what is actually implemented; this changelog starts here.
- Export signal values: a row's settings dialog can write that row as `time,value`
  CSV or `<time> <value>` TXT, or every row as one long-format
  `time,signal,value` CSV. Values use the row's on-screen format (fixed point
  included); the download is named after the dump and signal, and an export over
  200 MB is refused with an actionable message.
- Large-dump loading UX: browser dumps are read in 8 MB chunks with a progress
  row in the sidebar and a reading line in the top bar, then a `parsing ...`
  step; Esc or the cancel button aborts the read (`load cancelled — ... was not
  opened`). Dumps from 100 MB warn before parsing and dumps over 512 MB are
  refused before any byte is read; the success line reports parse time, size and
  wasm memory.
- Language-server diagnostics are visible in the editor: design problems are
  published as monaco markers (squiggles) and the top bar keeps an `N problem(s)`
  chip, backed by a `nav.problems` store slice.
- The browser QA suite moved into the repository (`qa/`): 41 scenarios with their
  own fixtures and generators, a cross-platform `run.mjs` (plus a `run.ps1`
  wrapper) with real exit codes, screenshots under `qa/artifacts/`, and a README.
  CI runs an advisory smoke job (`empty,vcd,layout,layoutRules`) against the
  bundle the build job produces.

### Changed

- The toolchain (rust 1.98.1, `rustfmt`, `clippy`, the wasm target) is declared
  once in `rust-toolchain.toml`; CI installs it through `rustup show` instead of
  repeating versions in each workflow.
- The Rust workflows pin trunk 0.22.1, so the wasm-bindgen/wasm-opt defaults
  cannot drift with a trunk release.
- Improved GHW time-axis handling; GHW remains the experimental format (wellen
  cannot stream it, so it is still read eagerly).
- Opening a dump no longer blocks the whole tab on a single read: the browser
  path streams the file in chunks and repaints between them, and the status line
  keeps the stable `waveform loaded` prefix while appending `— opened in ..., ...,
  wasm memory ...`.
- The state store's `nav` slice now carries the language-server problem count
  (`nav.problems`); the top bar subscribes to it, so a diagnostic updates only
  that slice.

### Fixed

- The README no longer claims a nightly benchmark suite, a rust 1.67.1 pin, a
  `cargo install tauri` step or a macOS asset that does not exist.
- The browser QA suite's offline check no longer starts a second static server on
  the port the launcher already uses; the old harness `-Offline` flag failed with
  `EADDRINUSE` because of exactly that.

## [0.5.0] - 2026-10-22

### Added

- Expand a multi-bit signal into its individual bit rows (and collapse them
  again) from the row menu.
- Verdi-style navigation: jump to the next/previous value transition of the
  selected row, a `go to` time box, a draggable time ruler and `ctrl+A`
  select-all rows.

## [0.4.2] - 2026-10-22

### Added

- The load/drive view is a tab of the wave frame instead of a separate dock, so
  it shares the space and the scroll/zoom state of the waveform.

### Fixed

- The waveform stops at the end of the dump instead of drawing past it, and
  analog rows are drawn as steps (no interpolation between samples).

## [0.4.1] - 2026-10-22

### Added

- A docked inspector, a draggable editor/wave splitter and the dump handle in
  the state store.
- Drag the signal-name column wider from its edge.

### Fixed

- Redraw when the wave pane changes size; snap the cursor in pixel space and
  symmetrically; give the label column a real grip.
- Stop the editor from stealing the waveform's key handler; flatten bus edges at
  the window border.
- Land the editor cursor on the first letter of a jumped-to declaration;
  highlight the jumped-to name.
- Delete the selected rows with `Delete`.
- Keep the hint separator from being read as Verilog, and show radix prefixes.

### Changed

- Drop the monaco worker languages the editor never uses (smaller bundle).

## [0.4.0] - 2026-10-21

### Added

- One state store with typed actions and slice subscriptions; the wave rows and
  the viewport/cursors live in it, and splitters use tier-A DOM writes so a drag
  re-renders nothing.
- Honest empty and error states; the editor collapses when there is nothing to
  show.
- Bus bands, cursor snapping, a value column at the cursor and per-row analog
  mode.
- Source clicks add the signal, declarations resolve, the load/drive view
  navigates, and cursor values appear as editor inlay hints.

### Changed

- `BREAKING` (internal): the renderer draws to one `<canvas>`; the old
  per-row SVG is gone, and the wave model is fetched through a cached, batched
  accessor.

## [0.3.0] - 2026-10-20

### Added

- A canvas renderer for the waveform (device-pixel-ratio aware, windowed reads)
  and a self-hosted monaco build, so the editor no longer needs a CDN.
- The R2 state-store design and the locked layout constants, plus a browser
  matrix that drives the built app.

### Fixed

- Ten bugs found by driving the built app (tree expansion, stale labels, deleted
  rows coming back, cursor drift and others).

### Changed

- `BREAKING`: the legacy `data_struct::Module`/`ModuleValue` model and the
  websocket server demo are gone. Only the stdio language-server transport
  remains in the back end.
- The desktop open-file dialog was redesigned.

## [0.2.4] - 2026-10-20

### Fixed

- The release workflow builds the plain Tauri binary itself instead of searching
  for it.

## [0.2.3] - 2026-10-20

### Fixed

- The release workflow locates the Tauri binary wherever cargo put it.

## [0.2.2] - 2026-10-20

### Fixed

- The release workflow finds the Tauri binary on all three runners.

## [0.2.1] - 2026-10-20

### Added

- Plain binaries are published next to the installers, under the names the
  download instructions use.

## [0.2.0] - 2026-10-20

### Added

- New waveform and design backends: VCD/FST/GHW through wellen, and a
  SystemVerilog design database with declarations, drivers and loads.
- A design-aware language server for the desktop and the browser, and two-way
  navigation between the waveform and the source.
- Dark redesign with a measured layout and an aligned time grid.
- FST and GHW dumps can be opened (read as bytes, parsed in the app).
- The web build is deployed to GitHub Pages.

### Changed

- `BREAKING`: rust 1.98.1 everywhere, and the monaco git dependency is gone.
- The release wasm bundle shrank from about 28 MB to about 9 MB.
- The VCD front end reads through a buffered reader (less memory), and malformed
  VCD input is reported instead of panicking.

## [0.1.0] - 2023-04-08

### Added

- Initial release: a VCD waveform viewer with a module/signal tree, two cursors,
  zoom/pan, per-signal display formats, waveform colouring and reordering, a
  monaco-based source view that jumps to declarations, and a Tauri desktop
  shell backed by a websocket demo server.