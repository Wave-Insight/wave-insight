# Wave Insight

[Click to try the web demo](https://wave-insight.github.io/wave-insight/)

A wave viewer and debugger for Verilog/SystemVerilog: it reads VCD, FST and GHW
dumps, shows the hierarchy and the waveforms, and links signals to their
declarations, drivers and loads in the source.

## Platform

There are two builds, and both parse everything in the application itself — no
server and no second copy of the reader:

* `wasm version`: you can click [here](https://wave-insight.github.io/wave-insight/) to have a try. It's only a website so you don't need to download anything or install anything. A browser normally gives wasm 4GB of memory, so a very large dump can still exhaust it; a dump is opened lazily (only the header and a cheap scan of the time stamps, values are decoded per signal as you look at them), so the size that fits is much larger than it used to be.
* `tauri version`: a local app for windows, linux and mac os. You can download at [release page](https://github.com/Wave-Insight/wave-insight/releases). For windows user, you can download `wave_insight_tauri.exe` to run. For linux user, you can download `wave_insight_tauri_ubuntu`, and then you need to run `chmod a+x wave_insight_tauri_ubuntu` and `./wave_insight_tauri_ubuntu` to run this app. For mac user, same to linux user except for the app is `wave_insight_tauri_mac`. You can drag `.vcd`, `.fst`, `.ghw` or `.v`/`.sv` files on the app to load. The tauri backend only opens the file dialog and returns the bytes; the dump is parsed by the webview with the same code the browser build uses.

The old `server version` (a websocket that streamed a serialised waveform model
to a web page) has been removed: the browser reads files itself, so it had no
job left. What remains in the `back-end` crate is the stdio transport of the
language server (`cargo run -p back-end --bin lsp`).

## Build

### toolchain

This project is pinned to `rust 1.67.1` by [`rust-toolchain.toml`](rust-toolchain.toml). Some
versions in `Cargo.lock` are from early 2023 and do not build with rustc >= 1.68 anymore
(for example `proc-macro2 1.0.56` fails with `E0635: unknown feature proc_macro_span_shrink`).
To move to a newer rust, run `cargo update` first, fix whatever breaks, then bump the channel
in that file.

### test and bench

```bash
cargo test
```

The benchmarks use the unstable `test` crate, so they are behind a feature:

```bash
cargo +nightly bench -p wave-insight-lib --features nightly-bench
```

They read `test.vcd` from the repository root, which is git ignored, so drop one there first.

### wasm version

The bundle carries the SystemVerilog parser and the waveform backend, so it is
large. Build it with size optimisation, which takes the release chunk from
about 28 MB down to 9 MB (1.9 MB over the wire once the server g-zips it):

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cd front-end
CARGO_PROFILE_RELEASE_OPT_LEVEL=s trunk serve --release
```

then you can open your browser and go to `localhost:8080`

### tauri version

download from [release page](https://github.com/Wave-Insight/wave-insight/releases), or build on your own

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cargo install tauri
cargo tauri build
```

## Usage

Load a `.vcd`, `.fst` or `.ghw` dump and any `.v`/`.sv` sources it came from.
Click a signal in the sidebar (or an identifier in the source) to add it to the
wave view; scroll to move through the signals, `ctrl`+wheel to zoom, drag to
zoom into a range, left/right click to place the two cursors, `z`/`f` to zoom
out and fit.

## Todos

- [ ] better verilog parser(will move to project [`Werilog`](https://github.com/Wave-Insight/Werilog))
- [ ] add signal to waveshow window when clicking on verilog code
- [ ] signal driver tree and load tree
- [ ] write the value on the driver/load tree base on the cursor position
- [ ] export signal value to .txt or .mat or csv
- [ ] inline python for writing own process on signal value
- [ ] signal value show in analog way
- [ ] signal value show in fixed point way
- [ ] better signal search: match case, match whole word, and regular expression
- [ ] signal bundles: axi, avalong and so on
- [ ] SpinalHDL

