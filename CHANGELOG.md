# Changelog

All notable changes to Wave Insight are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): releases are tagged
`vMAJOR.MINOR.PATCH`, and `BREAKING` marks a release that drops a public API or
an on-disk/transport format.

## [Unreleased]

Nothing yet: the next wave lands here.

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