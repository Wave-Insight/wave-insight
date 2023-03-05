# Wave Insight

[Click to try the web demo](https://wave-insight.github.io/wave-insight/)

A web based wave viewer

## Platform

* wasm: you can click [here](https://wave-insight.github.io/wave-insight/) to have a try. It's only a website so you don't need to download anything or install anything. Normally, web browser only gives 4GB of memory for wasm, so it may crash when you try to load a big vcd file
* tauri version: will comes in the future. Will be a local app for windows, linux and mac os
* server version: still on developing. For those who like simulate on a remote server. The vcd parser and verilog parser will running on the server. All the communications between server and your web browser are only: which file you want to read, verilog source code, which signal you want to see, and the signal value.

## Build

### wasm version

```bash
cd front-end
trunk serve --release
```

then you can open your browser and go to `localhost:8080`

### tauri version

coming soon

### server version

coming soon

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

