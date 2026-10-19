# Wave Insight

[Click to try the web demo](https://wave-insight.github.io/wave-insight/)

A web based wave viewer

## Platform

* `wasm version`: you can click [here](https://wave-insight.github.io/wave-insight/) to have a try. It's only a website so you don't need to download anything or install anything. Normally, web browser only gives 4GB of memory for wasm, so it may crash when you try to load a big vcd file
* `tauri version`: a local app for windows, linux and mac os. You can download at [release page](https://github.com/Wave-Insight/wave-insight/releases). For windows user, you can download `wave_insight_tauri.exe` to run. For linux user, you can download `wave_insight_tauri_ubuntu`, and then you need to run `chmod a+x wave_insight_tauri_ubuntu` and `./wave_insight_tauri_ubuntu` to run this app. For mac user, same to linux user except for the app is `wave_insight_tauri_mac`. You can drag `.vcd` or `.v` file on the app to load
* `server version`: still on developing. For those who like simulate on a remote server. The vcd parser and verilog parser will running on the server. All the communications between server and your web browser are only: which file you want to read, verilog source code, which signal you want to see, and the signal value.

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

### server version

still a minimal demo, it serves `front-end/dist` on `127.0.0.1:2992` and answers signal
requests over a websocket on port `2993`

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cd front-end
trunk build --no-default-features --release --features backend
cd ..
cd back-end
cargo run --release -- <vcd file> <listen address>
```

both arguments are optional and default to `test.vcd` and `0.0.0.0:2993`

## Usage

load `.vcd` and `.verilog` file

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

