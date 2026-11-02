# Wave Insight

[Click to try the web demo](https://wave-insight.github.io/wave-insight/)

A wave viewer and debugger for Verilog/SystemVerilog: it reads VCD, FST and GHW
dumps, shows the hierarchy and the waveforms, and links signals to their
declarations, drivers and loads in the source.

## Platform

There are two builds, and both parse everything in the application itself — no
server and no second copy of the reader:

* `wasm version`: you can click [here](https://wave-insight.github.io/wave-insight/) to have a try. It's only a website so you don't need to download anything or install anything. A browser normally gives wasm 4GB of memory, so a very large dump can still exhaust it; a dump is opened lazily (only the header and a cheap scan of the time stamps, values are decoded per signal as you look at them), so the size that fits is much larger than it used to be.
* `tauri version`: a local app for Windows, Linux and macOS. You can download it at the [release page](https://github.com/Wave-Insight/wave-insight/releases). For Windows users there is `wave_insight_tauri.exe`; Linux users download `wave_insight_tauri_ubuntu`, then run `chmod a+x wave_insight_tauri_ubuntu` and `./wave_insight_tauri_ubuntu`. The macOS asset is `wave_insight_tauri_mac_arm64` (Apple Silicon only; it is not signed, so Gatekeeper needs the usual "Open anyway" confirmation). You can drag `.vcd`, `.fst`, `.ghw` or `.v`/`.sv` files on the app to load. The shell is **Tauri 2**: the backend only opens the file dialog and returns the bytes (plus drag-and-drop paths); the dump is parsed by the webview with the same code the browser build uses.

The old `server version` (a websocket that streamed a serialised waveform model
to a web page) has been removed: the browser reads files itself, so it had no
job left. What remains in the `back-end` crate is the stdio transport of the
language server (`cargo run -p back-end --bin lsp`).

## Build

### toolchain

The workspace is pinned to **rust 1.98.1** by [`rust-toolchain.toml`](rust-toolchain.toml);
`rustup` installs that compiler automatically, and CI does the same through
`rustup show`, so the version lives in one place. The wasm/desktop builds also
need the wasm target:

```bash
rustup target add wasm32-unknown-unknown
```

The front end is **Yew 0.21** with the `csr` renderer (`features = ["csr"]`,
`yew::Renderer::<App>::new().render()`); the 0.19 -> 0.21 migration and its
result (5 `changed()` impls, the dropped clippy workaround, 179 front-end tests)
are recorded in [`docs/migrations/yew-0.21.md`](docs/migrations/yew-0.21.md).

### test and lint

```bash
# the crates that build on the host (this is what CI runs)
cargo test --workspace --exclude front-end --exclude app --locked

# the front end's non-DOM tests: reducer, store, layout, geometry, value formatting
cargo test -p front-end --bins --locked

# what CI also checks
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

### benchmarks

The waveform and language-server hot paths have criterion benchmarks under
`wave-insight-lib/benches/`: `waveform.rs` (six groups - `open`,
`time_range_hint`, `decode`, `value_at`, `window`, `render` - twenty
benchmarks) and `lsp.rs` (two groups - `design`, `lsp` - position queries
against a synthetic 100-module / ~3.5k-signal design). Large dumps are generated
in memory (a ~1.7 MB / 100k-change and an ~8.3 MB / 500k-change VCD), so no
multi-megabyte dump is committed:

```bash
cargo bench -p wave-insight-lib -- --quick               # both suites, quick
cargo bench -p wave-insight-lib -- decode                # one waveform group
cargo bench -p wave-insight-lib --bench lsp -- --quick   # LSP/design queries
```

The HTML report lands in `target/criterion/report/index.html`. CI never gates on
the numbers: `.github/workflows/bench.yml` compiles the suite on a weekly
schedule and, on manual dispatch, runs the hot-path groups (`open`, `decode`,
`time_range_hint`, `value_at`) plus the `lsp`/`design` queries and uploads the
criterion report as an artifact, so a broken benchmark is noticed without
letting a noisy runner block a merge.

Indicative `--quick` numbers from one developer machine (measurements, not
budgets): opening a small inline VCD 7.3 us / 1.7 MB 410 us / 8.3 MB 1.75 ms;
the first trace-range scan 0.68 ms and 6.4 ns when cached (the VCD range is now
read from the header plus a bounded tail scan, O(header + tail) instead of the
whole body, which took it down from 13.5 ms on the 8.3 MB synthetic dump);
decoding 11.6 ms for one signal / 11.8 ms sequentially / 12.1 ms batched / 38 ns
when cached (the sequential decode got ~10% faster by reserving its allocation
up front); `value_at` 306 ns packed and 147 ns decoded; a streamed window 9.1 /
13.2 / 46.2 ms; formatting 148 ns for a 32-bit hex value and 271 us for a
2500-change row. The language-server queries used to scan the whole elaboration
on every request; the span index built with each elaboration makes them cheap:
`design/item_at` 174.51 ms -> 20.13 us, `design/signals_at` 52.52 ms ->
19.44 us, `lsp/definition` 2.048 ms -> 12.42 us, `lsp/references` 2.095 ms ->
58.28 us, and `documentSymbol` -11.6% (its module index is per file now). The
answers are byte-identical to the old scan (checked by a differential test at
every fixture position). The old `nightly-bench` feature gate still enables
nothing and is not used by these benchmarks.

### supply chain

The committed lockfile is watched by a supply-chain gate: [`deny.toml`](deny.toml)
holds the policy (RustSec advisories, licences, banned crates and a source
allow-list) and `.github/workflows/audit.yml` runs `cargo deny check` weekly and
on every push to `main` as a **hard** gate. Three unmaintained crates are ignored
with reasons in `deny.toml` (all build-time or transitive `tauri` 1.x
dependencies, removed by the tauri 2 migration); vulnerabilities are never
ignored - RUSTSEC-2026-0384 (the yanked tokio 1.54.0) was found by this gate and
fixed by moving the lockfile to tokio 1.54.1. Run it locally with
`cargo install cargo-deny && cargo deny check`. [`.github/dependabot.yml`](.github/dependabot.yml)
keeps the lockfile moving with grouped weekly cargo/actions PRs; major bumps of
`tauri`/`tauri-build`/`yew` are held until the migration in the roadmap. The
Rust CI also runs the host tests once in the release profile
(`cargo test ... --release`), because LTO / `codegen-units = 1` /
`panic = "abort"` only show their breakage there. (The repository root is still
a convenient place for a test dump: `test*.vcd` there is git-ignored.)

### browser QA

The end-to-end matrix lives in the repository now: [`qa/`](qa/README.md) drives
the built `front-end/dist` in headless Chrome over CDP, uploads its own fixtures
and asserts the user-visible behaviour (the current matrix has **55 scenarios /
446 assertions**; `--list` prints them). The full matrix is green with the R2
layout rules required. The newest scenarios cover the generated/arrayed design
tree in the sidebar, elaboration diagnostics reaching the editor and the top-bar
chip, the TXT export matching the CSV for the same row, multi-dump tabs and the
split view, the driver/load bit ranges in the editor hover and the inspector
(`bitRanges`, `inspectorBits`), editor completion (`completion`), and the
`corruption` regression: deterministic truncated-FST / flipped-FST-length /
header-truncated-VCD fixtures must produce an actionable error and recover to a
valid dump instead of crashing the page. Each run stages its build in its own
`qa/.stage/<pid>/`, so parallel matrix runs no longer collide. Build the front end first, then run:

```bash
node qa/run.mjs                 # full matrix against front-end/dist
node qa/run.mjs --layout        # the R2 layout rules are required, not pending
node qa/run.mjs --offline       # self-hosted editor check with CDNs blocked
node qa/run.mjs --scenarios empty,vcd,layout
```

Windows users can use the thin wrapper (`qa\run.ps1`, same flags);
`qa/run.ps1 -List` prints the scenario names. Exit codes are real: `0` green,
`1` a failing assertion, `2` the harness could not run (missing Chrome/build). CI
runs an advisory smoke job (`empty,vcd,layout,layoutRules`) against the bundle the
build job produced. The **full matrix** also runs in CI: `.github/workflows/qa.yml`
builds the bundle with the CI wasm profile and runs `node qa/run.mjs --layout`
every Monday (05:00 UTC) and on manual dispatch; it is advisory (it cannot block a
merge) and uploads the screenshots and log when it fails. The suite's README
explains the fixtures and how to add a scenario.

### wasm version

The bundle carries the SystemVerilog parser and the waveform backend, so it is
large. Build it with size optimisation and trunk, then open `localhost:8080`:

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked --version 0.22.1
cd front-end
CARGO_PROFILE_RELEASE_OPT_LEVEL=s trunk serve --release
```

The release wasm with the CI profile is **10,003,350 bytes = 9.54 MiB**
(about 2 MB over the wire, because GitHub Pages serves the assets
gzip-compressed). CI pins trunk 0.22.1 and fails the build if the release wasm
grows past **10 MiB** (`WASM_SIZE_BUDGET` in the Rust workflow), so a size
blow-up shows up before it ships; the current build leaves roughly half a
MiB of headroom, so a big new dependency needs a budget review. Keep the
`CARGO_PROFILE_RELEASE_OPT_LEVEL=s`
part of the command: a plain `trunk build --release` uses the default
`opt-level = 3` and produces a ~12.36 MiB bundle that *looks* like a budget
regression. The site itself is deployed from the same workflow with
`trunk build --release --public-url /wave-insight/` so every asset resolves under
the Pages sub-path.

### tauri version

download from [release page](https://github.com/Wave-Insight/wave-insight/releases), or build on your own

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked --version 0.22.1
cargo install tauri-cli --version "^2"
cargo tauri build
```

The desktop app is **Tauri 2** (`tauri 2.12` / `tauri-build 2.7`): the CLI is
`tauri-cli` 2.x (the `tauri` crate is only the runtime library), the config is the
v2 schema (`frontendDist`/`devUrl`, `app.windows`, top-level `bundle`) with an
explicit capability in `src-tauri/capabilities/default.json`, and the front end
talks to it through `__TAURI__.core.invoke` plus the webview `onDragDropEvent`.
On Linux the v2 prerequisites are needed - Ubuntu 24.04 ships the right one:

```bash
sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev \
    libayatana-appindicator3-dev librsvg2-dev libxdo-dev libssl-dev
```

The build runs `trunk build --features tauri` first (see `beforeBuildCommand` in
`src-tauri/tauri.conf.json`) and then bundles the desktop app. Note that in this
repository's environment the desktop build is **compile-verified only**
(`cargo check -p app`, `trunk build --features tauri`); opening the window,
the IPC commands, drag-and-drop and `cargo tauri build` bundles are validated by
the tag workflow on the real runners, not by a desktop runtime test here.

### dependency migrations

Both planned migrations are done and their plans record the results: Tauri
1.x -> 2.x landed in Wave 7
([`docs/migrations/tauri-2.md`](docs/migrations/tauri-2.md), phase 1/phase 2
results) and Yew 0.19 -> 0.21 landed in Wave 9
([`docs/migrations/yew-0.21.md`](docs/migrations/yew-0.21.md), `Result (Wave 9)`).
Each plan keeps the breaker list, the steps and the verification that was run.

## Usage

Load a `.vcd`, `.fst` or `.ghw` dump and any `.v`/`.sv` sources it came from.
Click a signal in the sidebar (or an identifier in the source) to add it to the
wave view; scroll to move through the signals, `ctrl`+wheel to zoom, drag to
zoom into a range, left/right click to place the two cursors, `z`/`f` to zoom
out and fit, `n`/`shift+n` to jump between value transitions. `Delete` removes
the selected rows. The wave frame has a `Drivers / loads` tab that shows the
declaration, drivers and loads of the selected signal and jumps to them; a
sliced driver/load shows the slice it touches (e.g. `q[3:0]`) next to the size. A row's
`...` menu opens its settings: number format (including a fixed-point exponent),
an analog trace, colour, load/drive actions and an **Export** section. To the
right of each signal name the value at the cursor is shown after a cursor is
placed.

- **Export**: the settings dialog's `CSV (row)`, `TXT (row)`, `CSV (all rows)`
  and `MAT (row)` buttons write a row as `time,value` CSV, `<time> <value>` TXT or
  a MATLAB level-5 file, or every row as one long-format `time,signal,value` CSV.
  Values use the row's on-screen format (fixed point included); the MAT file has
  two n x 1 double columns, `wave_time` and `sig_<sanitized name>`, with x/z bits
  written as `NaN` and signed/fixed-point values matching the value column, and it
  is one row at a time (the all-rows button is CSV only). The download is named
  after the dump (`<dump>-<signal>.csv`, `<dump>-rows.csv`; the dump's file stem
  is used when it has one, otherwise its root scope). A generated file over 200 MB
  is refused with an actionable message instead of freezing the tab.
- **Several dumps**: open more than one file at a time (the file picker accepts
  multiple files, or drop them one after another). Each open dump gets a tab in
  the strip above the `Waveform`/`Drivers loads` tabs, so even a single-dump
  session shows one tab (with its close button). A tab keeps that dump's rows,
  zoom/pan, cursors and inspector selection; clicking it switches, its close
  button (or `Ctrl+W` while the strip has focus) closes it and activates a
  neighbour. Closing the last tab removes the strip and brings the empty start
  state back; reopening a dump after that restores its remembered rows.
  Reopening a file that is already open focuses its existing tab instead of
  adding a second copy. The open dumps are budgeted: their file bytes may not
  exceed 1.5 GiB, and past that the open is
  refused with an actionable message ("close a dump tab first") rather than
  letting the tab run out of wasm memory. The status line appends
  `— N dumps open, M MB`.
- **Split view**: with at least two dumps open, the wave frame shows a split
  toolbar: **Split view** opens a right pane, every *other* dump gets a **Show
  <name> on the right** choice, **Linked** toggles a shared time axis and **Close
  split** closes the second pane. The left pane stays interactive; the right pane
  is read-only (its dump's name plus a `read only` label, painted with the same
  canvas machinery and decoding only visible rows). Linked mode maps the left
  pane's visible window onto the right dump by fraction of trace (clamped) and
  draws the cursors at the mapped times. Limits: the right pane is read-only (no
  independent pan/zoom, no row scrolling or selection), the split is a fixed
  50/50 (no draggable ratio yet), and the split state is not persisted across
  reloads. A single-dump session renders no split DOM at all. See
  [`docs/design/multi-dump.md`](docs/design/multi-dump.md).
- **Large dumps**: a browser dump is read in 8 MB chunks with a progress row in
  the sidebar and a reading line in the top bar, then a `parsing ...` line while
  the parser runs. Esc or the cancel button stops the read and reports that the
  file was not opened. Dumps of 100 MB and up warn that parsing may take a while;
  dumps over 512 MB are refused before a byte is read, with advice to trim the
  dump. The success line reports what it cost: `waveform loaded — opened in
  412 ms, 105 MB, wasm memory 320 MB`.
- **Diagnostics**: parser/front-end diagnostics and elaboration diagnostics
  (`unresolved-name`, `width-mismatch`, wildcard and implicit connection
  problems, non-constant generate bounds) are merged per document, deduplicated
  and published into the editor as monaco markers (squiggles); the top bar keeps
  an `N problem(s)` chip visible even when the source pane is collapsed. The
  merged diagnostics are republished when the top module changes. Source hover
  cards also list the bit ranges a signal is driven or read through, as written
  in the source (e.g. `- driven here: q[3:0], q[7:4]`). The source editor also
  completes: typing (or Ctrl+Space) suggests the enclosing module's
  ports/signals/parameters/genvars/instances plus the database's module names,
  and after `instance.` it lists that instance's module members, filtered by a
  case-insensitive prefix.
- **Find a value**: the ruler box (`time / value`) accepts either kind of query.
  Type an integer and press Enter to move cursor 1 to that time (the old goto
  behaviour); type anything else and press Enter to jump to the *next* change of
  the selected row whose formatted value matches (Shift+Enter searches backwards).
  Matching uses the same formatting as the value column, fixed point and x/z
  included, and is case-insensitive. The status line reports
  `found <value> in <row> at t=... (n/total)`; a miss is an error status. This is
  the Verdi find-value flow.
- **Big designs**: the sidebar module tree and signal list are virtualized above
  80 rows - only the rows inside the scroll viewport exist in the DOM, with
  spacers keeping the scrollbar the real length - so a dump with thousands of
  signals stays responsive. Lists of 80 rows or fewer keep the plain DOM (and
  the same selection behaviour) they always had.
- **Persistence**: the window layout (sidebar width, editor/wave ratio, sidebar
  visibility) and each signal's display settings (format, fixed point, analog,
  colour) are remembered in the browser's `localStorage` and restored on the next
  visit. The row list is remembered too, per dump identity (the file name and its
  byte size): reopening the same dump puts its rows back in order with their
  settings. Limits: up to 20 dumps and 200 rows per dump are remembered (the
  least recently opened dump is dropped first), rows added before any dump is open
  are not attributed to a dump, and a same-name file whose byte size is unknown
  falls back to matching on the name alone. Remembered rows whose signal is no
  longer in the dump are skipped and the load line says how many. A future or
  corrupt stored payload, or a browser that denies storage, falls back to the
  defaults.

The keyboard and accessibility contract: `Delete` removes the selected rows,
`ctrl+A` selects every row, the arrow keys and `Home`/`End` move the row
selection, `Escape` closes dialogs and the `go to` box without clearing the
selection, and `ctrl+W`/`ctrl+F4` closes the active source tab. Dialogs are
labelled modals (`role="dialog"`, `aria-modal`) that trap Tab, take focus on
open and give it back to the opener on close; focus rings are visible, the tab
closer is a real button, and the wave list and candidate picker expose
`listbox`/`option` semantics.

## Known limitations

* **Elaboration coverage.** The design front end is `sv-parser` 0.13.5 plus an
  elaborator: modules, ports (by name/position/`.*`), parameters, nets/variables,
  instances and the common `assign`/`always` blocks work. Generate regions,
  constant-bound generate-for loops and generate-if/case branches are unrolled
  (named blocks keep their labels, anonymous blocks become `genblk1`, ... and
  iterations get `g[2]`-style scopes, so paths look like `top.g[2].u`); arrayed
  instances (`mod u[3:0]`) expand to one per element in Verdi order (`u[3]`,
  `u[2]`, ...).  Limits: the generate shape is frozen from the module's *default*
  parameter values (a per-instance override that would change a generate bound
  is not reflected, whereas array bounds are re-evaluated per instance and do
  honour overrides), non-constant generate bounds stay warnings, and there is no
  generate node in the instance tree - paths are flattened. `typedef`/structs/
  enums, interfaces and modports, packages, classes, tasks/functions and
  unsupported width expressions are still skipped with a diagnostic instead of
  aborting the file.
* **GHW is experimental.** wellen cannot stream GHW, so a GHW dump is read
  eagerly: the time axis is learned by reading the body, and value queries
  materialise the requested signals. On a large GHW this is slow and memory
  hungry compared with VCD/FST (which are streamed lazily).
* **Language-server coverage.** Definition/hover/references/symbols come from the
  declaration spans the elaborator recorded; a construct from the "skipped"
  list above has no definition to jump to, and name lookup is best effort when a
  signal cannot be resolved across instances. Editor squiggles and the top bar's
  problem count now include the elaboration diagnostics (`unresolved-name`,
  `width-mismatch`, non-constant generate bounds) merged with the parser/front-end
  ones per document, deduplicated and republished when the top module changes. A
  skipped construct may still be reported as a warning even though the source is
  valid SystemVerilog. Completion now works in the editor through the Monaco
  provider (trigger `.`, case-insensitive prefix filtering, cancellation
  honoured). Its limits: member completion resolves the instance chains the
  design models, but arrayed-instance selectors (`u[1].`) and generate scopes or
  packages cannot be used as qualifiers, and the prefix extraction is not
  select-aware (only identifiers and dots before the caret are read). One
  documented gap:
  only the picked top module is elaborated, so an elaboration diagnostic that
  depends on another top (for example the width-mismatch fixture) is not
  surfaced in the editor yet.
* **Corrupt dumps are recovered on a best-effort basis.** The readers validate
  input and turn bad files into an error message instead of panicking: FST files
  get a structural pre-parse check (block lengths against EOF, plus gzip /
  hierarchy / time-table plausibility caps), so a file with flipped length fields
  is rejected as corrupt instead of driving a giant allocation or aborting the
  viewer. The check is plausibility-based, not a full FST validator, and the
  library still wraps third-party parsing in `catch_unwind`. The release profile
  is `panic = "abort"` (needed for the wasm size), so where unwinding is not
  available a dependency panic still aborts the page/app rather than recovering.
  A VCD truncated only in its body has no trailer to detect, so it opens as a
  shorter but structurally valid dump; the corruption fixture therefore cuts the
  header.
* **Very large dumps.** The lazy path keeps memory proportional to what is on
  screen plus the raw file bytes, but a browser tab is not a database: the
  105 MB test dump used during development loaded in well under a second and
  stayed responsive, while its renderer memory grew by roughly 2.8x the file
  size. That is why the loader refuses dumps over 512 MB up front (and warns from
  100 MB) instead of letting the tab run out of wasm memory; a dump that large is
  still better opened in the desktop build, which reads the file from disk.
* **Split-view limits.** The side-by-side view is an MVP: with two or more
  dumps the left pane is interactive and the right pane is **read-only** - it
  cannot pan or zoom on its own, and its rows cannot be scrolled or selected -
  the split is a fixed 50/50 with no draggable ratio yet, and the split state is
  not persisted across reloads (see
  [`docs/design/multi-dump.md`](docs/design/multi-dump.md)). The open dumps share
  a 1.5 GiB file-bytes budget; past it a new open is refused with an actionable
  message.
* **Bit-range hover is textual.** The source hover lists the bit ranges a
  signal is driven/read through, but only the final identifier's select is
  recorded: `+:`-style and width-star ranges are not interpreted, a
  generate-loop select stays the text `[i]`. The source hover and the
  Drivers/loads inspector rows both show the slice now (`q[3:0]`; a whole-signal
  reference shows nothing).

## Todos

Done (shipped):

- [x] signal driver tree and load tree — the `Drivers / loads` tab and the inspector list both, with click-to-jump
- [x] add a signal to the wave view when clicking an identifier in the source (with a candidate picker when ambiguous)
- [x] show a signal's value in analog form (per-row setting)
- [x] show cursor values and locate time (`go to` box, next/previous transition, draggable ruler)
- [x] expand a bus into its bits; per-row formats, colour and multi-select delete
- [x] self-host the editor (pinned monaco build, no CDN)

Done in the next release (unreleased at the time of writing):

- [x] fixed-point display (settings dialog: exponent)
- [x] signal search options: match case, whole word and regular expression
- [x] export signal values as CSV, TXT or MAT (MAT is one row at a time; x/z bits become NaN)
- [x] large-dump loading: chunked read with progress and cancel, a readable `parsing ...` step, and a 512 MB size guard
- [x] language-server diagnostics: editor squiggles plus a top-bar `N problem(s)` chip
- [x] browser QA suite moved into the repository (`qa/`, 55 scenarios / 446 assertions, own fixtures) with an advisory CI smoke job and a weekly full-matrix workflow
- [x] generate regions/loops/conditionals with constant bounds are unrolled, and arrayed instances expand to per-element paths (see Known limitations for the remaining gaps)
- [x] sidebar module tree and signal list virtualization above 80 rows
- [x] window layout and per-signal display settings persist across reloads (versioned `localStorage`)
- [x] accessibility/keyboard pass: modal semantics with a focus trap and focus return, visible focus rings, arrow/Home/End row selection, `ctrl+W`/`ctrl+F4` tab close, tier-A draggable ruler
- [x] driver/load diagnostics: unresolved names, wildcard/implicit connections and port width mismatches are reported, and now reach the editor through the merged LSP diagnostics
- [x] tauri file browser starts in the user's home directory
- [x] favicon data-URI, so the `/favicon.ico` 404 console error is gone
- [x] elaboration diagnostics reach the editor: the language server merges design + elaboration diagnostics per document, deduplicates them and republishes on a top-module change
- [x] session restore: the row list is remembered per dump identity (name + byte size; 20 dumps / 200 rows, LRU) and skipped rows are reported in one status line
- [x] criterion benchmarks for the waveform hot paths (six groups / twenty benchmarks) with an advisory bench workflow that compiles weekly and runs the hot-path groups on dispatch
- [x] supply-chain gate: `deny.toml` policy plus a weekly/push `cargo deny` audit workflow (it caught the yanked tokio 1.54.0)
- [x] find a value in the selected row: type a value in the ruler box and press Enter (Shift+Enter searches backwards) to walk the changes with that formatted value (Verdi find-value flow)
- [x] fast dump open: the VCD time range is found from the header plus a bounded tail scan instead of the whole body (~20x cheaper on the 8.3 MB benchmark)
- [x] migration plans for yew 0.21 and Tauri 2 added under `docs/migrations/` (breaker lists, steps, verification)
- [x] LSP/design position queries indexed: `textDocument/definition` answers in ~12 us on the synthetic 100-module design (byte-identical to the old scan, differential-tested) with a new `bench lsp` suite
- [x] browser QA coverage for the generated/arrayed design tree, elaboration diagnostics in the editor, the TXT export, multi-dump tabs and the split view, driver/load bit ranges in the hover and the inspector, editor completion, and corruption recovery (truncated/flipped FST, header-truncated VCD) - 55 scenarios / 446 assertions
- [x] multi-dump + split-view design note under `docs/design/multi-dump.md` (planned Wave 8)
- [x] Tauri 2 migration: v2 config + capabilities, `__TAURI__.core` IPC and `onDragDropEvent` drag-and-drop, Linux webkit2gtk-4.1 prerequisites, CI on Ubuntu 24.04 (compile-verified; runtime and bundles are validated by the tag workflow)
- [x] waveform hardening: a corrupt/truncated FST is rejected by a structural pre-parse validator instead of aborting the viewer; aliased signals stream under the requested variable index; deterministic VCD/FST fuzz and query-tier differential tests cover the entry points
- [x] R2 step 3b: `State.design` (DesignDb + elaboration cache + top), the derived module tree, `focus.instance` + module-tree selection, and editor edits dispatched as `SourceReloaded` (re-elaborate + LSP change)
- [x] open several dumps at once: a dump tab strip (one tab per open dump; closing the last one returns to the empty state), per-tab rows/viewport/focus/session restore, dedupe by file identity, close-activates-a-neighbour, a 1.5 GiB open-dumps budget and the status line's "N dumps open, M MB"
- [x] driver/load bit ranges recorded from selects and shown in the source hover ("driven here: q[3:0], q[7:4]"; limits in Known limitations)
- [x] Yew 0.21 migration: `features = ["csr"]`, `yew::Renderer` instead of `start_app`, the 5 `changed(old_props)` impls, the old crate-wide clippy allow removed, 179 front-end tests green; the CI-profile wasm is 9.49 MiB (inside the 10 MiB budget)
- [x] LSP completion server-side: `textDocument/completion` answers ports/signals, parameters/localparams/genvars, instances and module names with details/sortText (no elaboration needed); the Monaco wiring is still to come
- [x] side-by-side split view (MVP): split toolbar, read-only right pane, linked time axis by fraction of trace, mapped cursors; per-dump selectors; tab switches swap panes and closing a dump remaps or closes the split
- [x] editor completion popup: Monaco provider (trigger `.`, kind mapping, insertText/detail/sortText, cancellation) plus the v2 server-side member completions after `instance.`
- [x] inspector shows the sliced reference for drivers/loads (`.inspector-bits`, whole-signal references show nothing)
- [x] QA harness runs stage their build in a per-run directory (parallel-safe) and `.github/workflows/qa.yml` runs the full matrix weekly + on dispatch (advisory)

Still open:

- [ ] split-view polish: right-pane interaction (pan/zoom, row scrolling/selection), a draggable split ratio, and persisted split state
- [ ] completion qualifiers for arrayed instances/generate scopes, and select-aware prefix extraction
- [ ] inline python for writing your own process on signal values
- [ ] signal bundles (AXI, Avalon and so on)
- [ ] SpinalHDL support
- [ ] better Verilog parser — the separate `Werilog` rewrite was dropped; `sv-parser` 0.13.5 stays, and gaps are covered in the elaborator (see Known limitations)

## License

Wave Insight is released under the **Apache License 2.0** — see [`LICENSE`](LICENSE).

The editor bundled under `front-end/monaco` is [monaco-editor](https://github.com/microsoft/monaco-editor)
0.44.0, which is **MIT**-licensed and is vendored unmodified from its npm
release; its licence terms cover that directory. All Rust dependencies keep
their own licences; the policy in [`deny.toml`](deny.toml) and the audit workflow
are the authoritative check (`cargo deny check`).