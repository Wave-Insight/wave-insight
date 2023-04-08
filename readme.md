# Wave Insight

[Click to try the web demo](https://wave-insight.github.io/wave-insight/)

A web based wave viewer

## Platform

* `wasm version`: you can click [here](https://wave-insight.github.io/wave-insight/) to have a try. It's only a website so you don't need to download anything or install anything. Normally, web browser only gives 4GB of memory for wasm, so it may crash when you try to load a big vcd file
* `tauri version`: a local app for windows, linux and mac os. You can download at [release page](https://github.com/Wave-Insight/wave-insight/releases). For windows user, you can download `wave_insight_tauri.exe` to run. For linux user, you can download `wave_insight_tauri_ubuntu`, and then you need to run `chmod a+x wave_insight_tauri_ubuntu` and `./wave_insight_tauri_ubuntu` to run this app. For mac user, same to linux user except for the app is `wave_insight_tauri_mac`. You can drag `.vcd` or `.v` file on the app to load
* `server version`: still on developing. For those who like simulate on a remote server. The vcd parser and verilog parser will running on the server. All the communications between server and your web browser are only: which file you want to read, verilog source code, which signal you want to see, and the signal value.

## Build

### wasm version

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cd front-end
trunk serve --release
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

coming soon

```bash
rustup target add wasm32-unknown-unknown
cargo install trunk
cd front-end
trunk build --no-default-features --release --features backend
cd ..
cd back-end
cargo build --release
```

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

