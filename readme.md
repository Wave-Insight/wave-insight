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

`cargo clippy --workspace` (and anything else that compiles the `app`
crate) needs `front-end/dist` to exist: `tauri::generate_context!` validates
the configured `frontendDist` path at compile time. A clean checkout has no
`dist` (it is gitignored), so CI creates a placeholder first; locally either
run `trunk build` once in `front-end/` or create the same placeholder:

```bash
mkdir -p front-end/dist && printf '<!doctype html><title>placeholder</title>\n' > front-end/dist/index.html
```

With that placeholder all eight CI jobs (Rust test / test-front-end /
test-release / fmt / clippy / build / deploy / qa, plus the audit workflow) are
green on `main` again. The QA job uses a second, root-relative `dist-qa` bundle
(the Pages bundle uses `--public-url /wave-insight/`, so serving it at `/` 404s
every asset) and Node 22, and the release-profile test job caught a corrupt-FST
length that made the FST reader allocate 5e16 bytes - the FST validator now
rejects such files at open (see the changelog).

### benchmarks

The waveform and language-server hot paths have criterion benchmarks under
`wave-insight-lib/benches/`: `waveform.rs` (seven groups - `open`,
`time_range_hint`, `decode`, `value_at`, `window`, `window_many`, `render` -
twenty-three benchmarks) and `lsp.rs` (two groups - `design` and `lsp` - position queries
**and completion** against a synthetic 100-module / ~3.5k-signal design). Large
dumps are generated in memory (a ~1.7 MB / 100k-change and an ~8.3 MB /
500k-change VCD), so no multi-megabyte dump is committed:

```bash
cargo bench -p wave-insight-lib -- --quick               # both suites, quick
cargo bench -p wave-insight-lib -- decode                # one waveform group
cargo bench -p wave-insight-lib --bench lsp -- --quick   # design/LSP/completion
```

The HTML report lands in `target/criterion/report/index.html`. CI never gates on
the numbers: `.github/workflows/bench.yml` compiles the suite on a weekly
schedule and, on manual dispatch, runs the hot-path groups (`open`, `decode`,
`time_range_hint`, `value_at`) plus the `lsp`/`design` groups - which now
include the completion benchmarks - and uploads the criterion report as an
artifact, so a broken benchmark is noticed without letting a noisy runner block a
merge.

Latest `--quick` numbers from one developer machine (measurements, not budgets;
no benchmark moved more than ~20% against the numbers recorded in earlier waves):
`design/item_at` 19.6 us, `design/signals_at` 19.1 us, `lsp/definition`
12.2 us, `lsp/references` 56.8 us, `document_symbol` ~100 us, and the new
completion group - `lsp/completion_plain` 101.04 us, `completion_prefix`
29.83 us, `completion_member` 36.45 us. On the waveform side: opening the
8.3 MB synthetic VCD 1.82 ms, the first `time_range_hint` call 0.59-0.66 ms,
`decode/single` 11.0 ms, the large streamed window 41.5 ms and
`render/row_hex_8bit` 244 us. The bounded drawing path added in Wave 13 is
`window_many/full_trace_capped` at 2.33 ms for eight rows where the full-history
decode of the same rows takes 99.6 ms - and it does not materialise the
histories. The remaining documented ranges are unchanged.
The span index built with each elaboration is why the position queries are in
the microsecond range; their answers are byte-identical to the old scans
(differential test at every fixture position).

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
and asserts the user-visible behaviour (the current matrix has **58 scenarios /
524 assertions**; `--list` prints them). The full matrix is green with the R2
layout rules required. The newest scenarios cover the generated/arrayed design
tree in the sidebar, elaboration diagnostics reaching the editor and the top-bar
chip, the TXT export matching the CSV for the same row, multi-dump tabs and the
split view, the driver/load bit ranges in the editor hover and the inspector
(`bitRanges`, `inspectorBits`), editor completion (`completion`), and the
`corruption` regression: deterministic truncated-FST / flipped-FST-length /
header-truncated-VCD fixtures (plus a conflicting-width VCD) must produce an
actionable error and recover to a valid dump instead of crashing the page, and
the `largeDump` scenario covers the memory hook, release on close, split
accounting, the 512 MB guard and a well-formed wide dump. Each run stages its build in its own
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
merge) and uploads the screenshots and log when it fails. The suite needs **Node >= 22** (the CDP client uses the global `WebSocket`, which
Node 20 lacks); the CI QA job runs Node 22 against a root-relative `dist-qa`
bundle. The suite's README explains the fixtures and how to add a scenario.

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
the tag workflow on the real runners, not by a desktop runtime test here. The dump/source commands return `tauri::ipc::Response` (the raw
body) instead of `Vec<u8>`, so a 100 MB dump crosses the IPC boundary as one
`ArrayBuffer` and one copy into wasm - it used to be serialised as a JSON array
of numbers (hundreds of MB of JSON, a 100M-element JS array and another wasm
copy). This path only runs in the desktop build, so it is verified by compilation
here and belongs to the next release's desktop test.

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
  is a comparison surface for its dump (its name plus a `read only` label, the
  same canvas machinery, decoding only visible rows). In **unlinked** mode the
  right pane scrolls its rows with the wheel (virtualized and clamped), zooms with
  Ctrl+wheel anchored at the pointer and pans with a left-drag; those gestures
  commit to that dump only and never re-render the shell. **Linked** mode follows
  the left pane's window by fraction of trace (clamped) and draws the cursors at
  the mapped times; switching back to unlinked freezes the window the pane is
  showing instead of jumping to a default, and a resize observer keeps the canvas
  backing store in step with its box. A `role=separator` divider between the
  panes can be dragged or moved with ArrowLeft/ArrowRight (2% steps) and
  Home/End; the 25-75% ratio is saved with the layout (`split_ratio`; an old
  saved layout loads at the 50/50 default). A single-dump session renders no split
  DOM at all. Remaining limits: unlinked mode does not show its own cursors yet,
  there is no visible row scrollbar, a drag-pan ends at the pane edge (the pointer
  is not captured by the window), and whether the split is open is not persisted.
  See [`docs/design/multi-dump.md`](docs/design/multi-dump.md).
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
  case-insensitive prefix. Member completion understands arrayed-instance
  selectors (`u[1].`, `u[i].` - a bare `u.` on an arrayed instance lists nothing
  rather than guessing the element) and generate scopes (`g[0].`, `g[i].`,
  nested `g[0].u_leaf.`; a literal selector must name an unrolled element). A
  select on a signal (`q[3:0].`) returns nothing on purpose while the plain
  selector stays a filter, and a leading dot inside a port list filters
  connection names (`.cl` -> `clk`).
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
- **Bulk add**: in the module tree, Ctrl/Cmd+click toggles a scope in a
  multi-selection and Shift+click selects the visible range (selected rows are
  marked `.tree-row.selected`/`aria-selected`). Every row also has an
  always-visible add button, and a right-click (or Shift+F10) opens a row menu:
  **Add all signals in this scope (N)**, **Add signal**, **Select for bulk add**,
  **Select all visible (M)**, **Clear bulk selection**. The Modules header has
  **Select all**, and once anything is selected a sticky **bulk bar** footer shows
  "N signals selected" with **Add selected** and **Clear** plus the Ctrl/Shift
  hint - it does not move the tree rows. One batch adds everything: dedupe and
  the remembered display settings apply, the row list updates once (500 rows =
  one render), the status line says `added N signals (M already in the wave)`,
  and above the 2000-row cap the add buttons turn visibly over-cap and the action
  reports the error instead of a partial add. Limits: the selection is
  session-only and is cleared when the tree rebuilds, bulk add does not move the
  inspector selection, and there is no marquee or Shift+arrow selection yet.
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

## Memory and very large dumps

The browser build runs the whole engine (parser, hierarchy, decode cache, canvas
renderer, LSP) inside one wasm32 module, and there is no mmap for a page-picked
file. What that means in practice:

- **What is kept.** The picked file is read into wasm memory in 8 MiB slices and
  reserved once at its exact size (a fallible allocation: failure becomes an
  error, not an allocator abort). VCD and GHW keep the file bytes and materialise
  the whole trace body in wellen's packed form; FST streams its body and keeps
  only the header and time table. On top sit the hierarchy and the decoded
  windows of the rows that are (or recently were) on screen.
- **Decode budget.** ONE global 64 MiB budget covers the decoded histories of
  every open dump and of the split pane (it used to be 64 MiB per pane). When it
  is exceeded, dumps that no pane draws give their decoded histories back first,
  then their parsed bodies (re-parsed on the next visit); a dump that is being
  drawn is never evicted, so the window caches of drawn rows can push
  `decodedBytes` over the budget - it is soft, and `overBudget` in the readout
  says when that happens (a drawn row itself is bounded by its window, next
  bullet). Closing a dump or the split frees its decoded data and parsed body
  immediately.
- **Windowed decode (the crash fix).** The renderer no longer materialises a
  row's whole history: it asks `window_changes_many` for a capped window (one
  stream pass, a fair per-row cap of roughly one change per pixel) and keeps a
  per-row window cache inside the global budget. Dense *visible* rows were the
  real blow-up trigger - a 97.5 MiB / 58M-change FST loaded and scrolled fine,
  while one visible 10M-change row used to decode 229 MB and grow the wasm by
  ~0.5 GB; with windows a drawn row is bounded by the window, not the trace.
- **Pre-flight refusal.** Before a byte is read, the loader estimates the wasm
  memory a file needs (48 MB base + 3.2x file for VCD, 0.6x for FST, 2.0x for
  GHW) against a 1280 MiB policy cap and refuses with the numbers when it cannot
  fit: "about N MB of wasm memory are needed ... close a dump you are not looking
  at, split the dump, or open it in the desktop build". A 501 MB VCD is now
  refused up front instead of attempting a ~1.5 GB allocation.
- **Readout.** `window.__waveDebug.memory` publishes `{ wasmBytes, fileBytes,
  decodedBytes, budget, dumps, unloaded, released, overBudget }`, and the load
  line reports the wasm memory used.
- **Measured.** The footprint work cut the load peak by 31-44%: 34 MB -> 85.9 MB
  (was 137.7 MB), 139 MB -> 295.3 MB (was 521.8 MB), 349 MB -> 715.0 MB (was
  1033.9 MB). Hierarchy string interning cut hierarchy bytes by 58.3% on a
  200k-signal design (78.8 MB -> 32.9 MB, 394 B -> 164 B per signal). The
  windowed path benchmarks at 2.33 ms for eight rows over the full trace
  (full-history decode of the same rows: 99.6 ms), and a dense row now stays
  inside the decode budget instead of growing it by the full history.
- **Honest limits.** The browser keeps the whole file for VCD/GHW (there is no
  mmap in the browser); wasm linear memory never shrinks, so a tab's high-water
  mark is its real footprint even after closing dumps; a drawn row decodes only
  a capped window, but that window plus the per-row cache still costs memory, so
  an extremely wide window over a dense row can exceed the soft budget (the
  `overBudget` flag says so); and wasm32 has a hard 4 GiB address space (a
  browser tab usually dies well before that). Prefer FST over VCD for very large
  dumps, keep one or two dumps open in the browser, and use the desktop build for
  multi-GB sessions.
- **Direction.** The desktop build can run the engine natively over a read-only
  memory map (`memmap2`) and send only rendered slices to the webview; the
  browser's analogue is a dedicated worker plus an OPFS lazy byte source. The
  `ByteSource` trait (file or in-memory) is the Wave 12 groundwork for both. The
  current model and both plans are in
  [`docs/design/large-files.md`](docs/design/large-files.md).

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
  honoured). It resolves the instance chains, arrayed-instance selectors
  (`u[1].`) and generate scopes (`g[0].`) the design models; an arrayed instance
  without a selector, an unknown qualifier, or a select on a signal (`q[3:0].`)
  deliberately returns an empty list instead of guessing, and packages are not
  used as qualifiers. One documented gap:
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
* **Very large dumps.** See [Memory and very large dumps](#memory-and-very-large-dumps):
  the browser keeps the whole file for VCD/GHW, the 64 MiB decode budget is soft
  (a drawn row decodes a capped window, but window + cache still cost memory)
  and wasm memory never shrinks. The
  loader still refuses files over 512 MB up front (and warns from 100 MB), and
  the memory pre-flight refuses a load whose estimated footprint will not fit
  the 1280 MiB policy cap - with numbers and advice instead of an allocator
  abort. For multi-GB dumps prefer FST and the desktop build (the native
  memory-mapped backend is the next wave).
* **Split-view limits.** The right pane is a comparison surface, not a second
  editor: in unlinked mode it scrolls, zooms and pans on its own but shows no
  cursors of its own yet, has no visible row scrollbar and stops a drag-pan at the
  pane edge (the pointer is not captured by the window); whether the split is
  open is not persisted across reloads (the divider ratio is). The open dumps
  share a 1.5 GiB file-bytes budget; past it a new open is refused with an
  actionable message (see [`docs/design/multi-dump.md`](docs/design/multi-dump.md)).
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
- [x] browser QA suite moved into the repository (`qa/`, 58 scenarios / 524 assertions, own fixtures) with an advisory CI smoke job and a weekly full-matrix workflow
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
- [x] browser QA coverage for the generated/arrayed design tree, elaboration diagnostics in the editor, the TXT export, multi-dump tabs and the split view, driver/load bit ranges in the hover and the inspector, editor completion, corruption recovery (truncated/flipped FST, header-truncated VCD, conflicting-width VCD), the memory-first large-dump behaviour and the dense-row windowed-decode contract - 58 scenarios / 524 assertions
- [x] multi-dump + split-view design note under `docs/design/multi-dump.md` (planned Wave 8)
- [x] Tauri 2 migration: v2 config + capabilities, `__TAURI__.core` IPC and `onDragDropEvent` drag-and-drop, Linux webkit2gtk-4.1 prerequisites, CI on Ubuntu 24.04 (compile-verified; runtime and bundles are validated by the tag workflow)
- [x] waveform hardening: a corrupt/truncated FST is rejected by a structural pre-parse validator instead of aborting the viewer; aliased signals stream under the requested variable index; deterministic VCD/FST fuzz and query-tier differential tests cover the entry points
- [x] R2 step 3b: `State.design` (DesignDb + elaboration cache + top), the derived module tree, `focus.instance` + module-tree selection, and editor edits dispatched as `SourceReloaded` (re-elaborate + LSP change)
- [x] open several dumps at once: a dump tab strip (one tab per open dump; closing the last one returns to the empty state), per-tab rows/viewport/focus/session restore, dedupe by file identity, close-activates-a-neighbour, a 1.5 GiB open-dumps budget and the status line's "N dumps open, M MB"
- [x] driver/load bit ranges recorded from selects and shown in the source hover ("driven here: q[3:0], q[7:4]"; limits in Known limitations)
- [x] Yew 0.21 migration: `features = ["csr"]`, `yew::Renderer` instead of `start_app`, the 5 `changed(old_props)` impls, the old crate-wide clippy allow removed, 179 front-end tests green; the CI-profile wasm is 9.49 MiB (inside the 10 MiB budget)
- [x] LSP completion: `textDocument/completion` answers ports/signals, parameters/localparams/genvars, instances and module names with details/sortText (no elaboration needed), and the Monaco provider is wired so the editor shows a completion popup (Wave 10)
- [x] side-by-side split view (MVP): split toolbar, read-only right pane, linked time axis by fraction of trace, mapped cursors; per-dump selectors; tab switches swap panes and closing a dump remaps or closes the split
- [x] editor completion popup: Monaco provider (trigger `.`, kind mapping, insertText/detail/sortText, cancellation) plus the v2 server-side member completions after `instance.`
- [x] inspector shows the sliced reference for drivers/loads (`.inspector-bits`, whole-signal references show nothing)
- [x] QA harness runs stage their build in a per-run directory (parallel-safe) and `.github/workflows/qa.yml` runs the full matrix weekly + on dispatch (advisory)
- [x] split-view polish: unlinked right pane scroll/zoom/pan committed to its dump only, linked freeze on unlink, resize observer, draggable 25-75% divider (drag/keyboard) with a persisted `split_ratio`
- [x] completion v3: arrayed-instance selectors, generate-scope qualifiers (literal must name an unrolled element), nested scope/instance chains, select-aware prefix handling and the port-connection leading-dot filter
- [x] completion benchmarks in `benches/lsp.rs` (`completion_plain`/`completion_prefix`/`completion_member`), covered by the bench workflow's `lsp|design` filter
- [x] three verifier-found bugs fixed before release: split unlinked toggle not freezing the right pane, the right canvas ignoring window resizes, and completion returning nothing at bit-select columns in instance port lists
- [x] bulk add from the module tree: Ctrl/Cmd+click and Shift+click multi-selection, `Add selected (N)` + per-scope add + Ctrl/Cmd+Enter, one `RowsAdded` batch (dedupe, row prefs, one rows-version bump/session write), status line and the 2000-row cap error
- [x] memory-first large dumps: one global 64 MiB decode budget across dumps/split with a free plan, exact fallible read buffer, pre-flight estimate (48 MB + 3.2x VCD / 0.6x FST / 2.0x GHW vs 1280 MiB cap) with actionable refusals, `window.__waveDebug.memory` readout, and freeing on dump/split close (load peaks down 31-44%)
- [x] library memory efficiency: hierarchy string interning (-58.3% hierarchy bytes, 394 -> 164 B/signal on 200k signals), lazy full paths, exact `memory_stats` accounting and the file-or-memory `ByteSource` trait
- [x] the user-reported crash root cause fixed: a malformed VCD with one variable id declared at conflicting widths is rejected as Corrupt at open (wellen used to panic and trap the wasm); same-width aliasing stays legal
- [x] design note `docs/design/large-files.md`: current memory model, why the browser has no mmap, the desktop native memory-mapped backend plan and the browser worker/OPFS option
- [x] bounded windowed rendering (the 3+ GB dense-row crash fix): `window_changes_many` + `WindowedSignal` (one stream pass, fair per-pixel cap, O(rows*cap) memory) with a per-row window cache inside the global 64 MiB budget, plus the `window_many` benchmark (2.33 ms vs 99.6 ms full history for 8 rows) and the `denseDump` QA scenario
- [x] FST validation hardening: internal counts (geometry max handle, blackouts, time-table items, frame/value-change handle counts, alias chains) are checked against the bytes present, so a corrupt alias-chain length is a `Corrupt` error instead of a 5e16-byte allocation
- [x] desktop raw IPC: `choose_vcd`/`choose_vcd_absolute` return `tauri::ipc::Response` (one ArrayBuffer + one wasm copy) instead of `Vec<u8>` serialised as a JSON number array
- [x] bulk-add discoverability: always-visible per-row add button, right-click/Shift+F10 row menu, Select all in the Modules head, sticky bulk bar (count/Add selected/Clear/Ctrl-Shift hint) that does not move the rows, and a visible over-cap state
- [x] CI green again: the clippy job creates a `front-end/dist` placeholder on a clean checkout, the QA job builds a root-relative `dist-qa` bundle and uses Node 22, and the release test run exposed the corrupt-FST allocation path above

Still open:

- [ ] desktop native backend with memory-mapped files (`docs/design/large-files.md`, the next architectural wave)
- [ ] browser worker + OPFS lazy byte source for large web sessions (the browser's mmap analogue)
- [ ] split-view remaining limits: unlinked cursor independence, a visible row scrollbar, window-captured drag-pan, and persisting whether the split is open
- [ ] completion: packages as qualifiers (optional; everything else the design models resolves)
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