# Changelog

All notable changes to Wave Insight are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): releases are tagged
`vMAJOR.MINOR.PATCH`, and `BREAKING` marks a release that drops a public API or
an on-disk/transport format.

## [Unreleased]

Nothing yet: the next wave lands here.

## [0.18.1] - 2026-11-03

### Fixed

- Desktop in-app file dialog: a dump over 64 MiB opened through the dialog was
  passed to the native `open_dump` as a path relative to the dialog's base
  directory (`USERPROFILE`/`HOME`), but `open_dump` resolves paths against the
  app's working directory - so the native open failed and the app silently fell
  back to the wasm byte path, which is exactly the memory problem the native mode
  exists to avoid (drag-and-drop passed an absolute path and worked). Added the
  `open_dump_parts(parts)` Tauri command, which resolves the dialog's navigation
  parts with the same base-directory rules as the other dialog commands;
  `open_dump(path)` stays absolute-only for drag-and-drop and launch arguments,
  and the front end now calls `remote::try_open_parts(parts)`. Five native tests
  cover the parts resolver (joining, ignored empty/separator-bearing components,
  the absolute result, the clean error for empty parts) and a temp-file open
  through the registry.


## [0.18.0] - 2026-11-03

### Added

- Desktop native renderer MVP: a dump larger than 64 MiB opened in the desktop
  build now goes through the native backend (`open_dump`) and is drawn read-only
  from bounded `dump_window` IPC queries, while smaller files (and every native
  error) keep the wasm path. The mode is visible - a `.dock-tab.native-badge`
  chip reads **native (read-only, mmap)** and the status line reports
  `waveform loaded (native read-only, mmap) — <FMT> <N> MB, range <start>..<end>
  in <ms> ms, mapped <M> MB, <signals> signals in <scopes> scopes` - and the
  sidebar tree, rows and wave canvas reuse the normal components. Single add and
  bulk add work against the native hierarchy; the hierarchy is fetched eagerly
  (paged, 400,000-row safety cap) and the MVP asks the backend for hex values
  (row display settings still work on the parsed bits). One native dump at a time,
  no split/multi-dump in native mode, and find/export are refused with an
  actionable message pointing at the wasm path (files up to 64 MB).
- Native memory-mapped byte source (desktop/host groundwork): the library can
  open a file through a read-only `memmap2` mapping with
  `Waveform::open_path_mmap` / `WaveformSource::Mmap`; readers serve pages
  straight from the mapping, so the source retains **0 heap bytes** (the OS page
  cache owns the resident cost). `Waveform::mapped_bytes` and
  `MemoryStats::source_mapped_bytes` report the mapping size, the mmap module is
  compiled only for non-wasm targets (the browser keeps its in-memory source), and
  tests cover byte-identical answers versus the file/memory sources, `Corrupt`
  errors through the mapping and mapping release on drop. Measured on the real
  97.5 MiB / 58M-change FST: mmap open 9.27 ms, file reader 10.18 ms, read into
  memory 30.31 ms; the new `mmap` benchmark group records the comparison.
- Desktop native dump backend: `src-tauri/src/dumps.rs` keeps a registry of
  natively opened dumps (monotonic ids, 32-dump cap) and answers five IPC
  commands - `open_dump`, `dump_hierarchy` (paged pre-order rows, 4096-row
  pages), `dump_window` (rendered values per signal for a time window, with a
  per-signal change cap and `before` context), `dump_value_at` and
  `close_dump`. File bytes never cross IPC; the webview receives rendered
  windows only, and `close_dump` drops the mapping. Measured: a 4096-row
  hierarchy page 1.82 ms, ten dense signals over the full trace 139 ms (debug),
  first `dump_value_at` 690 ms / warm 175 ms; five native tests cover the
  registry and query round-trip.
- A `test-app` CI job runs the native backend tests on `ubuntu-24.04` with the
  GTK/webkit prerequisites and the clean-checkout `front-end/dist` placeholder
  (the host `test` job excludes the `app` crate because of those prerequisites).

### Changed

- In the desktop build, dumps over 64 MiB now open through the native backend and
  are drawn read-only from mmap-backed windows over IPC, instead of being copied
  into the webview's wasm memory; dumps up to 64 MiB keep the wasm path, and a
  native error falls back to it as well. The web build is unchanged. Runtime
  verification of the desktop path (real IPC, drag-and-drop, mmap/lock/close
  lifecycle, window latency/zoom smoothness on huge dumps) still needs a desktop
  run - this environment has no GUI.
- The desktop architecture now has both halves in place: the native foundation
  (mmap source, registry, IPC queries) and the renderer MVP that draws it. Native
  parity (find, export, split/multi-dump, lazy hierarchy, `dump_value_at`
  wiring) is the follow-up wave.

### Notes

- The desktop large-file mode is an **MVP**: one native dump at a time, read-only
  windows, eager paged hierarchy and hex values. Runtime verification on a real
  desktop (IPC round trips, drag-and-drop, mmap/lock/close lifecycle, window
  latency and zoom smoothness on huge dumps) is still pending; native parity
  (find, export, split/multi-dump, lazy hierarchy, `dump_value_at` wiring) is the
  follow-up wave.

## [0.17.0] - 2026-11-03

### Added

- Bounded windowed rendering, the real fix for the 3+ GB dense-dump crash: the
  renderer no longer materialises the full history of every visible row. It
  requests capped windows through the new `Waveform::window_changes_many` /
  `WindowedSignal { changes, before, capped }` API (one stream pass for all rows,
  a fair per-row cap of roughly one change per pixel, early exit, O(rows * cap)
  memory) and keeps a per-row window cache inside the existing global 64 MiB
  decode budget. Dense *visible* rows were the trigger, not the file size: a real
  97.5 MiB / 3,025-signal / 58M-change FST loaded and scrolled fine, while one
  visible 10M-change row used to decode 229 MB and grow the wasm by ~0.5 GB per
  row. New `window_many` benchmark group: `full_trace_capped` 2.33 ms for eight
  rows versus 99.6 ms for the full-history decode of the same rows.
- FST validation hardening at open: the validator now checks the internal FST
  counts (geometry `max_handle`, blackout entries, time-table items, frame and
  value-change handle counts, alias chains) against the bytes actually present,
  so a corrupt file cannot make the reader allocate an absurd buffer.
- Desktop raw IPC: `choose_vcd` / `choose_vcd_absolute` return
  `tauri::ipc::Response` (the raw body) instead of `Vec<u8>`. Tauri 2 serialised
  a byte vector as a JSON number array, so a 100 MB dump became hundreds of MB of
  JSON plus a 100M-element JS array and another wasm copy; the raw response is one
  `ArrayBuffer` and one copy into wasm.
- Bulk-add discoverability: every module/signal row has an always-visible add
  button, a right-click (or Shift+F10) row menu offers **Add all signals in this
  scope (N)**, **Add signal**, **Select for bulk add**, **Select all visible (M)**
  and **Clear bulk selection**, the Modules header has **Select all**, and a
  sticky **bulk bar** footer (`role=status`, "N signals selected", Add selected,
  Clear, the Ctrl/Cmd-click / Shift-click hint) appears without moving the tree
  rows. Above the 2000-row cap `.tree-add-selected` visibly gains `.over-cap` +
  `aria-disabled="true"` while the bulk-bar action stays clickable so the
  actionable refusal can be shown.
- Browser QA: the new `denseDump` scenario (16 assertions - a dense VCD proxy
  with 200k+ steps plus an opt-in real FST via `QA_LARGE_FST=<path>`: bounded
  decoded window, no wasm growth over zoom cycles, a second dense row not doubling
  the history, and the same policy on the real 97.5 MiB FST) and the `bulkAdd`
  scenario (20 assertions - discoverability, context menu, select-all, sticky bar
  without row movement, over-cap state); the matrix is 58 scenarios / 524
  assertions. The `fst-gen` recipe (libfst clone + `gen.c`) is committed under
  `qa/fixtures/generators/`; the 97 MB fixtures stay in `%TEMP%`.
- CI is green again (all eight jobs on `main`): the clippy job creates a
  `front-end/dist` placeholder on a clean checkout, the build job also produces a
  root-relative `dist-qa` bundle for the QA job, and the QA job uses Node 22 (the
  CDP client needs the global `WebSocket`; Node 20 does not have it).

### Changed

- The drawing path is windowed: a visible row decodes a capped time window into a
  per-row cache (shared 64 MiB budget) instead of its full history, and
  `__waveDebug.memory.decodedBytes` now reflects the window caches. The readme's
  memory section documents the windowed model and its soft-budget limit.
- FST validation is part of opening a dump, not a best-effort extra: the
  structural walk now covers the data-block internals (frame values, value-change
  handles, alias chains) in addition to the top-level block list.
- The QA harness requires Node >= 22 and the CI QA job consumes the new
  `dist-qa` artifact instead of the Pages bundle.

### Fixed

- CI red: `cargo clippy` failed on a clean checkout because
  `tauri::generate_context!` validates `front-end/dist` at compile time; the QA
  advisory job 404ed every asset because it served the `--public-url
  /wave-insight/` Pages bundle at `/`; and it pinned Node 20, which lacks the
  global `WebSocket` the CDP client uses.
- `test-release` aborted on a corrupt FST whose alias-chain length made
  `fst-reader` try to allocate 5e16 bytes; the hardened validator now rejects it
  as `Corrupt` with a readable message before the reader runs.
- The user-reported 3+ GB crash on dense dumps (a clock changing on every time
  step): the full-history decode per visible row is gone in favour of the bounded
  windowed path.

## [0.16.0] - 2026-11-02

### Added

- Bulk add from the module tree: Ctrl/Cmd+click toggles a scope in a
  multi-selection, Shift+click selects the visible range (rows marked
  `.tree-row.selected`/`aria-selected`), the Modules header shows **Add selected
  (N)** and each row's **+** adds everything in that scope; Ctrl/Cmd+Enter is the
  keyboard path. All signals go through one batch reducer action (`RowsAdded`):
  dedupe, remembered display settings, ONE rows-version bump and session write
  (500 rows = one render), status `added N signals (M already in the wave)`, and
  a selection above the 2000-row cap is rejected with an error instead of a
  partial add.
- Memory-first large dumps: ONE global 64 MiB decode budget covers the decoded
  histories of every open dump and of the split pane (it used to be 64 MiB per
  pane); when it is exceeded, dumps no pane draws give their decoded histories
  back first and their parsed bodies only if that is not enough, while a drawn
  dump is never evicted (the budget is soft, reported as `overBudget`). The
  loader reserves the read buffer exactly and fallibly, and a pre-flight estimate
  (48 MB base + 3.2x file for VCD, 0.6x for FST, 2.0x for GHW, against a 1280 MiB
  policy cap) refuses a load before any byte is read with an actionable message
  instead of aborting the allocator. `window.__waveDebug.memory` publishes
  `{ wasmBytes, fileBytes, decodedBytes, budget, dumps, unloaded, released,
  overBudget }` and the load line reports the wasm memory. Closing a dump or a
  split frees its decoded data and parsed body immediately. Measured load peaks
  dropped 31-44% (34 MB: 137.7 -> 85.9 MB; 139 MB: 521.8 -> 295.3 MB; 349 MB:
  1033.9 -> 715.0 MB), and a 501 MB VCD is refused up front instead of attempting
  a ~1.5 GB allocation.
- Library memory efficiency: hierarchy names are interned (`Arc<str>`), full
  paths are built lazily, and `memory_stats` reports exact retained bytes; on a
  200k-signal hierarchy, hierarchy bytes dropped 58.3% (78.8 MB -> 32.9 MB,
  394 -> 164 B/signal). A `ByteSource` trait (file or in-memory implementations)
  is the groundwork for a memory-mapped native backend.
- A design note (`docs/design/large-files.md`) documenting the current memory
  model, why the browser has no mmap, the desktop native memory-mapped backend
  plan (memmap2) and the browser worker/OPFS lazy-byte-source option, with
  budgets and verification.
- Browser QA: `largeDump` (29 assertions - memory hook, release on close, split
  accounting, the 512 MB guard, a well-formed wide dump) and three new corruption
  assertions for the conflicting-width VCD; the matrix is 57 scenarios / 503
  assertions, green with the R2 layout rules required. The `gen-large` fixture
  generator's id encoding was fixed too (base-94 counter, unique ids at any
  signal count).

### Changed

- The decode cache budget is global across open dumps and the split pane instead
  of per renderer instance; the free plan, the pre-flight estimate and the
  memory readout live in one front-end module, and closing a dump or the split
  releases its decoded data and parsed body. VCD/GHW still keep the file bytes
  and materialise the body (FST streams), which the readme now states as an
  explicit limit.

### Fixed

- The user-reported large-dump crash root cause: a malformed VCD that declares
  one variable id with conflicting widths made wellen panic, and with the release
  profile's `panic = "abort"` the resulting wasm trap killed the add path (a
  blank viewer). A structural VCD validation at open now rejects it as
  `Corrupt` with an actionable message - "malformed VCD: variable id ! is
  declared with conflicting widths (64 and 8)" - while same-width aliasing stays
  legal. A 501 MB file is also refused by the pre-flight now instead of attempting
  a ~1.5 GB allocation.

## [0.15.0] - 2026-11-02

### Added

- Split-view polish: the read-only right pane is now a real comparison surface.
  In unlinked mode it scrolls its rows with the wheel (virtualized row window,
  clamped to the surface), zooms with Ctrl+wheel anchored at the pointer and pans
  with a left-drag; these gestures commit through
  `ViewCommittedFor { dump, view }` to that dump's own entry and never re-render
  the shell. Linked mode keeps following the left pane, and toggling to unlinked
  now freezes the window the pane is showing instead of jumping back to a default
  (it also no longer remounts on the toggle); a `ResizeObserver` repaints at the
  new css box so the canvas backing store tracks window resizes and divider moves.
  A draggable `.split-separator` (`role=separator`, `aria-valuemin/max/now`
  25/75, vertical) can be dragged with a shield, moved with ArrowLeft/ArrowRight
  in 2% steps and jumped with Home/End; the ratio persists with the layout as
  `split_ratio` (serde-defaulted, so an old saved layout loads at 50/50).
- Completion v3: member completion now resolves arrayed-instance selectors
  (`u[1].`, `u[i].` - a bare arrayed instance yields an empty list instead of
  guessing an element), generate-scope qualifiers (`g[0].`, `g[i].`, nested
  `g[0].u_leaf.`; a literal selector must name an unrolled element, so `g[9].`
  is empty), and the prefix extraction is select-aware: `q[3:0].` returns an
  empty list honestly while the plain `q[3:0]` text still filters. The
  port-connection leading-dot filter now works inside arrayed port lists
  (`.cl` -> `clk`, `.` -> the full list, `.reset` -> `reset`); new fixtures
  cover arrayed/scoped contexts.
- A completion benchmark group in `benches/lsp.rs` on the synthetic 100-module
  design: `lsp/completion_plain` 101.04 us, `completion_prefix` 29.83 us,
  `completion_member` 36.45 us; the bench workflow's `lsp|design` filter already
  covers them.
- Browser QA: the split-view scenario grew its pointer/keyboard/verifier checks;
  the matrix is now 55 scenarios / 458 assertions (was 446), green with the R2
  layout rules required.

### Changed

- The right pane's viewport commits through the new
  `ViewCommittedFor { dump, view }` action (stored on that dump's entry, not the
  active slice), and the divider commits through `SetSplitRatio`; the layout's
  persisted slice gained `split_ratio` (clamped to 0.25-0.75, default 0.5).
- The split separator is real DOM (`role=separator` with aria value attributes),
  so it is keyboard reachable and testable; the tier-A drag preview keeps the
  shell out of the pointer path.
- Benchmark numbers were refreshed against the current tree (no documented
  benchmark drifted by more than ~20%): `design/item_at` 19.6 us,
  `design/signals_at` 19.1 us, `lsp/definition` 12.2 us, `lsp/references`
  56.8 us, `document_symbol` ~100 us; waveform `open/vcd_synthetic_large`
  1.82 ms, first `time_range_hint` 0.59-0.66 ms, `decode/single` 11.0 ms,
  large streamed window 41.5 ms, `render/row_hex_8bit` 244 us.

### Fixed

- Toggling the split to unlinked mode did not freeze the right pane - it kept
  following the left pane's pans (and remounted on the toggle, losing its row
  window); both are fixed and covered by the split-view verifier checks.
- The split right canvas ignored window resizes: its backing store kept the old
  size and the trace looked stretched until an unrelated store event repainted
  it; it now tracks its box through a `ResizeObserver`.
- Completion returned an empty list at bit-select columns inside instance port
  lists (for example a caret in `u[3:0]` of a port list); the context
  classification now returns the module scope for bit-select/number positions,
  and the leading-dot filter works there again.

## [0.14.0] - 2026-11-02

### Added

- Side-by-side split view (MVP): with two or more dumps open, a split toolbar in
  the wave frame offers a `Split view` toggle, a `Show <name> on the right`
  choice per non-active dump, a `Linked` time-axis toggle and `Close split`. The
  left pane stays interactive and the right pane is read-only (its dump's name
  plus a `read only` label, drawn with the same canvas machinery and decoding
  only visible rows). Linked mode maps the left pane's visible window onto the
  right dump by fraction of trace (clamped) and draws the cursors at the mapped
  times. The store gained `State.split` (`right`, `linked`) with
  `SplitOpened`/`SplitClosed`/`SplitRightChanged`/`SplitLinkedToggled`, and the
  selectors can read a specific dump (`rows_of`, `dump_name_of`, the pane
  handle) instead of only the active one; switching tabs swaps the panes, closing
  a dump remaps the right pane or closes the split when fewer than two remain,
  and a single-dump session renders no split DOM. Limits: the right pane cannot
  pan/zoom on its own and has no row scrolling/selection, the split is a fixed
  50/50 with no draggable ratio yet, and the split state is not persisted.
- Editor completion: the Wave 9 server-side completion now has a Monaco provider
  (registered for design sources, trigger character `.`, LSP-to-Monaco kind
  mapping, `label`/`insertText`/`detail`/`sortText`, cancellation honoured). The
  server side also gained member completion: after `instance.` (and nested
  instance chains the design models, or a leading module name) it returns only
  that module's ports/signals/parameters/genvars/instances, and the prefix after
  the last dot filters labels case-insensitively. Limits: arrayed-instance
  selectors (`u[1].`) and generate scopes/packages are not resolvable as
  qualifiers, and the typed prefix is read as identifiers and dots only (not
  select-aware).
- The Drivers/loads inspector now shows the sliced reference of a driver/load
  (e.g. `q[3:0]`) between the signal name and the size; a whole-signal reference
  shows nothing. New `.inspector-bits` style.
- Browser QA coverage: `completion`, `inspectorBits` and `splitView` scenarios,
  bringing the matrix to 55 scenarios / 446 assertions, green with the R2 layout
  rules required. The harness now stages each run's build in its own
  `qa/.stage/<pid>/` directory, so parallel matrix runs no longer collide.
- A weekly full-matrix CI workflow (`.github/workflows/qa.yml`): builds the bundle
  with the CI wasm profile and runs `node qa/run.mjs --layout` every Monday
  (05:00 UTC) and on manual dispatch; advisory (no push/PR trigger,
  `continue-on-error`) with a 45-minute timeout, uploading the screenshots and
  log when it fails.

### Changed

- The split is store state and the panes are per-dump: the shell renders the
  split toolbar only with at least two dumps, and the right pane is deliberately
  read-only (no editing surface, and the single-pane `window.__waveDebug` hook
  keeps describing the active dump).
- The QA matrix is now also covered by the weekly workflow above; the per-push
  `rust.yml` smoke stays four scenarios and advisory.

## [0.13.0] - 2026-11-02

### Added

- Language-server completion (server-side). `initialize` now advertises a
  `completionProvider` (`resolveProvider: false`, `triggerCharacters: ["."]`)
  and `textDocument/completion` answers identifiers visible at the cursor: the
  enclosing module is found from the per-file module list by a binary search over
  line ranges (no elaboration), and the items are ports and signals
  (`Variable`, generate-scoped ones included), parameters/localparams and genvars
  (`Field`), instance names (`Reference`) and every module name of the database
  (`Module`), each with a detail string and a `sortText` that keeps locals before
  modules. Covered by the server's completion tests. The front end does not call
  it yet: wiring the Monaco provider is a later wave, so the app has no completion
  popup today.
- Browser QA `bitRanges` scenario: the editor hover (through the LSP provider
  hook) must show `driven here: q[3:0], q[7:4]` for a bit-sliced signal, and a
  whole-signal hover must show no ranges. The matrix now has 52 scenarios /
  414 assertions and is green with the R2 layout rules required.

### Changed

- Front end migrated to **Yew 0.21**: `yew = { version = "0.21", features = ["csr"] }`,
  `start_app` replaced by `yew::Renderer::<App>::new().render()`, five components
  gained `changed(old_props)`, and the crate-wide `unnecessary_operation` clippy
  allow is gone (0.21's `html!` expansion is clean). No `html!`/`props`/
  `Callback` fallout and no store or DOM changes; 179 front-end tests pass,
  message batching behaves as before (the tier-A render-count and layout gates
  stay green), and the CI-profile release wasm is 9,953,799 bytes = 9.49 MiB
  (+3.6% against the 9.60 MB it was before the migration, still inside the 10 MiB
  budget). A plain `trunk build --release` without
  `CARGO_PROFILE_RELEASE_OPT_LEVEL=s` builds at the default `opt-level = 3` and
  produces ~12.36 MiB - that is not a budget regression, and the docs keep
  pointing at the CI profile command. `docs/migrations/yew-0.21.md` records the
  result and the corrections to the plan.
- Supply chain: the Yew 0.21 dependency graph reaches two unmaintained crates
  (`anymap2` via yew, `bincode 1.3.3` via gloo-worker/gloo). `deny.toml` now
  carries reasoned ignores for both (unmaintained advisories, no vulnerability,
  not on the app's data path - the app is a csr build and never instantiates a
  worker or deserializes bincode); `cargo deny check` is green again. Both go
  away when yew drops them upstream.

## [0.12.0] - 2026-11-02

### Added

- Multi-dump: several dumps can be open at once. The store keeps one entry per
  tab (`State.dumps`: the handle, its time range, the rows, the viewport/cursors,
  the inspector focus and the session-restore state), while a session with no
  dump open (sources-only) keeps using the scratch `rows`/`viewport`/`focus`
  slices, so that workflow still works. `DumpAdded`/`DumpTabActivated`/
  `DumpClosed` add, switch and close tabs; reopening a file that is already open
  focuses its tab instead of adding a duplicate, and closing a tab activates a
  neighbour. A dump tab strip sits above the `Waveform`/`Drivers loads` tabs with
  one tab per open dump (a single dump shows one tab with its close button); each
  tab is a `role=tab` name button (file name, size tooltip) plus a close button.
  Closing the last tab removes the strip and brings the empty start state back,
  and reopening a dump afterwards restores its remembered rows; `Ctrl+W` with the
  focus inside the strip closes the active dump. Per-tab session restore now
  works, the open dumps share a 1.5 GiB file-bytes budget (a new open past it is
  refused with an actionable "close a dump tab first" message), and the status
  line appends
  `— N dumps open, M MB`. The strip's CSS moved to `front-end/styles.css`.
  A side-by-side split view is *not* in yet (step 3 of the design note).
- Driver/load bit ranges: `SignalReference` now carries an optional textual bit
  range (`[3:0]`, `[7]`, `[i]`) recorded from selects in both lvalues and
  expression reads and carried into `FlatSignal`'s drivers/loads. The language
  server's hover adds `- driven here: q[3:0], q[7:4]` / `- read here: ...` lines.
  The field is additive over serde (`#[serde(default)]`), so older payloads
  still decode; new fixtures and `tests/design_bits.rs` cover the cases. Limits:
  only the final identifier's select is recorded, `+:`-style ranges are not
  interpreted, generate-loop selects stay the text `[i]`, and the front-end
  inspector does not display the ranges yet (the API field is ready).
- Browser QA corruption coverage: deterministic fixtures (truncated FST, FST with
  a flipped data-section length field - the old multi-terabyte allocation abort -
  and a header-truncated VCD) and a `corruption` scenario with 18 assertions: no
  page exception, an actionable "could not be opened" error naming the file, the
  error tone, and a valid dump still loading afterwards. A `multiDump` scenario
  was added too; the matrix now has 51 scenarios / 407 assertions, green with the
  R2 layout rules required. A VCD truncated only in its body is documented as
  a structurally valid shorter VCD (there is no trailer to detect), which is why
  the corrupt VCD fixture cuts the header.

### Changed

- The dump-related state is per tab in the store: selectors read the active dump
  entry, the sidebar tree and the language-server top follow the active dump, and
  `State.tree` stays a derived cache. The no-dump sources-only session is kept
  alive by the scratch slices rather than by a synthetic empty dump.
- The dump tab strip (DOM, keyboard routing and CSS) is part of the shell; the
  status line appends the open-tab count and total dump-file size.
- The shell's dump strip subscribes a tab-only `DumpsTabsSlice` whose version
  changes only when the tab list or the active index changes, so a cursor or zoom
  commit changes the active dump's slices without re-rendering the shell (the
  tier-A render-count gate stays green).
- Bit ranges cross the wasm/serde boundary additively; a missing field in an
  older model decodes as `None`.

## [0.11.0] - 2026-11-02

### Added

- Tauri 2 migration (shell and front-end wiring): `tauri 2.12.5` /
  `tauri-build 2.7.4`, the v2 config schema (`frontendDist`/`devUrl`,
  `app.windows` with label `main`, top-level `bundle`, `mainBinaryName: "app"`,
  explicit CSP) and a new `src-tauri/capabilities/default.json`
  (`core:default`; the app's own commands are not ACL-gated). The front end now
  calls `__TAURI__.core.invoke` and `__TAURI__.webview.getCurrentWebview()
  .onDragDropEvent`; the removed v1 `tauri://file-drop` listener is gone and
  dropped absolute paths map into the existing file-drop path. CI moved to
  `ubuntu-24.04` with `libwebkit2gtk-4.1-dev` (+ `libxdo-dev`/`libssl-dev`)
  in the tauri, tauri_pre and clippy jobs. Verified here: `cargo check -p app`,
  the front end with `--features tauri` and with the default features, 171
  front-end tests, and a `trunk build --features tauri` (the wasm-bindgen pass
  validates the new JS bindings); opening the window, the IPC commands, real
  drag-and-drop and `cargo tauri build` bundles remain runtime-only and are
  gated by the tag workflow.
- R2 step 3b finished (architecture, no user-visible change): `State.design`
  owns the `DesignDb`, its elaboration cache and the top module; `SharedDesign`
  is gone, `State.tree` is a derived cache, the module-tree selection lives in
  `focus.instance` (the sidebar derives it and dispatches `FocusChanged`), the
  path picker and the settings dialog are store slices, and editor edits dispatch
  `SourceReloaded` - which emits the LSP change effect and re-elaborates (a no-op
  for unchanged text). `LspBridge` reads the design through the store. What is
  left in step 4 is only the pane wiring: panes could subscribe directly instead
  of receiving shell-mirrored props, the picker/dialog props could be pure store
  reads, and `Effect::MonacoShow` is still a no-op seam.
- Waveform hardening: a corrupt FST whose length fields are flipped used to
  drive a multi-terabyte allocation and an uncatchable process abort; a new
  structural pre-parse validator (block lengths checked against EOF, plus
  gzip/hierarchy/time-table plausibility caps) turns that into a `Corrupt`
  error. `HierarchyView::build` now runs inside the panic guard with a
  256-scope nesting bound, and aliased signals stream under the requested
  variable index (`ValueQuery::all` reports every alias). New deterministic
  fuzz-style tests in `wave-insight-lib/tests/waveform_fuzz.rs`: random valid
  VCDs compared against a hand-built model, truncation/byte-flip sweeps for VCD
  and FST, query-tier differential checks and the aliased-streaming case.

### Changed

- Building the desktop app needs the Tauri 2 CLI
  (`cargo install tauri-cli --version "^2"`) and, on Linux, the webkit2gtk-4.1
  prerequisites (Ubuntu 24.04: `libwebkit2gtk-4.1-dev`, `libxdo-dev`,
  `libssl-dev`). In this repository's environment the desktop build is
  compile-verified only; runtime and bundle checks happen on the tag workflow.
- CI runners moved from Ubuntu 22.04 to 24.04 (the webkit2gtk-4.0 pin was a
  tauri-1.x constraint), and `deny.toml` dropped the `instant`/`fxhash`
  unmaintained ignores because tauri 2 no longer pulls them; the remaining
  `proc-macro-error` ignore stays because tauri 2 still links GTK3 on Linux.
  `docs/migrations/tauri-2.md` records the phase 1/phase 2 results.

### Fixed

- A corrupt or truncated FST no longer aborts the viewer: the structural
  validator reports it as a corrupt dump (the check is plausibility-based, not a
  full FST validator, and the `panic = "abort"` caveat for release builds
  remains).
- Streaming an aliased signal now reports the requested variable's index instead
  of the first alias's.
- The deterministic truncation/byte-flip sweeps over VCD and FST bytes never
  panic; every entry point answers with a value or a `WaveformError`.

## [0.10.0] - 2026-11-02

### Added

- An indexed span structure for the design database, built with each
  elaboration. `DesignDb::item_at`/`signals_at` no longer walk every module,
  signal and driver/load reference on each request, and `documentSymbol` uses a
  per-file module index. On the synthetic 100-module / ~3.5k-signal design the
  benchmarks went from `design/item_at` 174.51 ms to 20.13 us (~8,670x) and
  `design/signals_at` 52.52 ms to 19.44 us, with `lsp/definition` 2.048 ms ->
  12.42 us (~165x), `lsp/references` 2.095 ms -> 58.28 us and `documentSymbol`
  -11.6%. Answers are byte-identical to the brute-force scan (differential test
  at every fixture position plus the deserialized-design fallback).
- A second criterion suite, `benches/lsp.rs`, for the language-server query hot
  paths (groups `design` and `lsp`); `.github/workflows/bench.yml` now runs the
  `lsp|design` groups on manual dispatch as well.
- R2 step 3b (partial, architecture): the path picker and the settings dialog are
  store slices now (`nav.picker`/`nav.dialog` with `PickerOpened`/`PickerChosen`/
  `PickerCancelled` and `DialogOpened`/`DialogClosed`); the wave pane no longer
  owns `menu_show`/`on_setting_idx`. The app and the wave pane subscribe to
  those slices and pass the data to the existing components.
- Three browser QA scenarios - `elaboration` (generated/arrayed design tree in
  the sidebar), `diagnostics` (elaboration unresolved-name diagnostics as
  Monaco markers plus the top-bar chip) and `exportTxt` (the TXT download equals
  the CSV of the same row) - bringing the matrix to 49 scenarios / 356 assertions,
  and the full run is green with the R2 layout rules required (356 passed, 0
  failed).
- A multi-dump and split-view design note (`docs/design/multi-dump.md`): the
  target per-dump model, the UI, migration steps, verification and risks.

### Changed

- R3 legacy deletion (internal, no user-visible change): the library's
  `data_struct` module is gone. `ShowType` moved to
  `wave_insight_lib::waveform::ShowType`; `Signal`/`CodeLocation` were deleted
  and the app's locate/jump path uses a small local `ResolvedSignal` instead.
  The acceptance grep for `data_struct` over the front end, the library and its
  tests/benches returns nothing.
- LSP position queries and `documentSymbol` are answered from the index built
  once per elaboration instead of rescanning the design per request.
- The bench workflow's dispatch filter grew from the waveform hot paths to
  `open|decode|time_range_hint|value_at|lsp|design`.

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