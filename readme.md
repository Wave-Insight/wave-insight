# Wave Insight

[Click to try the web demo](https://wave-insight.github.io/wave-insight/)

A wave viewer and debugger for Verilog/SystemVerilog: it reads VCD, FST and GHW
dumps, shows the hierarchy and the waveforms, and links signals to their
declarations, drivers and loads in the source.

## Platform

There are two builds, and both parse everything in the application itself — no
server and no second copy of the reader:

* `wasm version`: you can click [here](https://wave-insight.github.io/wave-insight/) to have a try. It's only a website so you don't need to download anything or install anything. A browser normally gives wasm 4GB of memory, so a very large dump can still exhaust it; a dump is opened lazily (only the header and a cheap scan of the time stamps, values are decoded per signal as you look at them), so the size that fits is much larger than it used to be.
* `tauri version`: a local app for Windows, Linux and macOS. You can download it at the [release page](https://github.com/Wave-Insight/wave-insight/releases). For Windows users there is `wave_insight_tauri.exe`; Linux users download `wave_insight_tauri_ubuntu`, then run `chmod a+x wave_insight_tauri_ubuntu` and `./wave_insight_tauri_ubuntu`. The macOS asset is `wave_insight_tauri_mac_arm64` (Apple Silicon only; it is not signed, so Gatekeeper needs the usual "Open anyway" confirmation). You can drag `.vcd`, `.fst`, `.ghw` or `.v`/`.sv` files on the app to load. The tauri backend only opens the file dialog and returns the bytes; the dump is parsed by the webview with the same code the browser build uses.

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

There are currently no benchmarks: the repository has no `benches/` targets, and
the library's old `nightly-bench` feature gate enables nothing. (The repository
root is still a convenient place for a test dump: `test*.vcd` there is
git-ignored.)

### wasm version

The bundle carries the SystemVerilog parser and the waveform backend, so it is
large. Build it with size optimisation and trunk, then open `localhost:8080`:

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked --version 0.22.1
cd front-end
CARGO_PROFILE_RELEASE_OPT_LEVEL=s trunk serve --release
```

The release wasm is about 9.6 MB (about 2 MB over the wire, because GitHub Pages
serves the assets gzip-compressed). CI pins trunk 0.22.1 and fails the build if
the release wasm grows past **10 MiB** (`WASM_SIZE_BUDGET` in the Rust
workflow), so a size blow-up shows up before it ships. The site itself is
deployed from the same workflow with `trunk build --release --public-url
/wave-insight/` so every asset resolves under the Pages sub-path.

### tauri version

download from [release page](https://github.com/Wave-Insight/wave-insight/releases), or build on your own

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk --locked --version 0.22.1
cargo install tauri-cli --version "^1"
cargo tauri build
```

The Tauri CLI is `tauri-cli` (the `tauri` crate itself is only the runtime
library, so installing it gives you no `cargo tauri` subcommand). The pinned
`tauri-cli` 1.x matches the 1.2 runtime used by `src-tauri`; the build runs
`trunk build --features tauri` first (see `beforeBuildCommand` in
`src-tauri/tauri.conf.json`) and then bundles the desktop app.

## Usage

Load a `.vcd`, `.fst` or `.ghw` dump and any `.v`/`.sv` sources it came from.
Click a signal in the sidebar (or an identifier in the source) to add it to the
wave view; scroll to move through the signals, `ctrl`+wheel to zoom, drag to
zoom into a range, left/right click to place the two cursors, `z`/`f` to zoom
out and fit, `n`/`shift+n` to jump between value transitions. `Delete` removes
the selected rows. The wave frame has a `Drivers / loads` tab that shows the
declaration, drivers and loads of the selected signal and jumps to them. A row's
`...` menu opens its settings: number format (including a fixed-point exponent),
analog trace, colour, and load/drive actions. To the right of each signal name
the value at the cursor is shown after a cursor is placed.

## Known limitations

* **Elaboration coverage.** The design front end is `sv-parser` 0.13.5 plus an
  elaborator: modules, ports (by name/position/`.*`), parameters, nets/variables,
  instances and the common `assign`/`always` blocks work. `generate` regions,
  loops and conditionals are **not** elaborated; `typedef`/structs/enums,
  interfaces and modports, packages, classes, tasks/functions, arrayed instances
  and unsupported width expressions are skipped with a diagnostic instead of
  aborting the file.
* **GHW is experimental.** wellen cannot stream GHW, so a GHW dump is read
  eagerly: the time axis is learned by reading the body, and value queries
  materialise the requested signals. On a large GHW this is slow and memory
  hungry compared with VCD/FST (which are streamed lazily).
* **Language-server coverage.** Definition/hover/references/symbols come from the
  declaration spans the elaborator recorded; a construct from the "skipped"
  list above has no definition to jump to, and name lookup is best effort when a
  signal cannot be resolved across instances.
* **Corrupt dumps are recovered on a best-effort basis.** The readers validate
  input and turn bad files into an error message instead of panicking, and the
  library wraps third-party parsing in `catch_unwind`. The release profile is
  `panic = "abort"` (needed for the wasm size), so where unwinding is not
  available a dependency panic still aborts the page/app rather than recovering.
* **Very large dumps.** The lazy path keeps memory proportional to what is on
  screen plus the raw file bytes, but a browser tab is not a database: the
  105 MB test dump used during development loaded in well under a second and
  stayed responsive, while its renderer memory grew by roughly 2.8x the file
  size, so dumps far beyond that will eventually hit the wasm memory cap.

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

Still open:

- [ ] export signal values to .txt/.mat/.csv
- [ ] inline python for writing your own process on signal values
- [ ] signal bundles (AXI, Avalon and so on)
- [ ] SpinalHDL support
- [ ] better Verilog parser — the separate `Werilog` rewrite was dropped; `sv-parser` 0.13.5 stays, and gaps are covered in the elaborator (see Known limitations)

## License

Wave Insight is released under the **Apache License 2.0** — see [`LICENSE`](LICENSE).

The editor bundled under `front-end/monaco` is [monaco-editor](https://github.com/microsoft/monaco-editor)
0.44.0, which is **MIT**-licensed and is vendored unmodified from its npm
release; its licence terms cover that directory. All Rust dependencies keep
their own licences (run `cargo metadata`/`cargo deny`-style tooling if you need
the full list).
