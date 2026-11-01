# Wave Insight browser QA suite

Scripted end-to-end checks for the built front end. This used to live in
`%TEMP%\wi-qa`; it is a repository asset now, runs from its own fixtures and
fails the run when the matrix is red.

## Requirements

- Node.js 18 or newer (the runner uses `fetch`, `AbortSignal.timeout` and
  `fs.cpSync`).
- A Chrome/Chromium build. The runner looks for the usual per-OS locations; set
  `CHROME=C:\path\to\chrome.exe` (or the equivalent) to override.

## Run it

```sh
# full matrix against the existing build in front-end/dist
node qa/run.mjs

# a subset (comma or space separated)
node qa/run.mjs --scenarios empty,vcd,layout,layoutRules

# the R2 layout rules are required, not pending
node qa/run.mjs --layout

# the self-hosted editor check (CDN hosts blocked) instead of the matrix
node qa/run.mjs --offline

# stage a different build
node qa/run.mjs --dist path/to/dist
```

Windows users can use the thin wrapper:

```powershell
.\qa\run.ps1
.\qa\run.ps1 -Scenarios empty,vcd,layout -Layout
.\qa\run.ps1 -Offline
.\qa\run.ps1 -List          # print the scenario names
```

Exit codes: `0` the matrix is green, `1` at least one assertion failed, `2` the
harness itself could not run (no Chrome, build missing, server did not start).

`run.mjs` copies `front-end/dist` into `qa/.stage` (gitignored), starts
`serve.js` on a free port, starts headless Chrome with a throwaway profile and
CDP on a second free port, runs the matrix, then kills both. Screenshots go to
`qa/artifacts/` (also gitignored). `--keep` keeps `qa/.stage` for debugging.

There is no build step in the runner: build the front end first
(`cd front-end && trunk build --no-default-features --features wasm --release`).
CI downloads the bundle built by the `build` job instead.

## What it drives

`qa.js` talks CDP to headless Chrome, uploads fixtures through
`DOM.setFileInputFiles`, clicks with real `Input.dispatchMouseEvent` /
`dispatchKeyEvent`, and screenshots on demand. The static server serves the
staged build at `/` and the fixtures at `/fixtures/` (the drop scenario fetches
`/fixtures/wave.vcd` from the page), so nothing is copied into the build.

Scenario list (the runner's `--list` prints it, default is all):

`empty`, `vcd`, `fst`, `bothDumps`, `sourcesOnly`, `sourcesAndDump`,
`clickToAdd`, `waveJump`, `inspector`, `lsp`, `zoomPan`, `cursors`, `formats`,
`colourAndDelete`, `layout`, `drop`, `badInputs`, `tabClose`, `deepWave`,
`deepEditor`, `deepSidebar`, `keysInEditor`, `addWithClosedTabs`, `extras`,
`tierA`, `layoutRules`, `traceEnd`, `waveTabs`, `navigation`, `busExpand`,
`busBench`, `busBands`, `snap`, `valueColumn`, `analog`, `analogStep`,
`mismatch`, `loadDrive`, `inlayHints`, `wave1`, `wave2`, `virtualization`,
`persistence`, `session`, `findValue`, `a11y`, `elaboration`, `diagnostics`,
`exportTxt`.

`wave1` asserts the Wave 1 features (fixed point readout, search options);
`wave2` asserts the Wave 2 features (CSV export through a real download,
the load report on the status line, LSP diagnostics as editor markers plus
the top-bar problem chip); `virtualization` proves the sidebar tree and
signal list keep the DOM bounded while the seam still reports the whole
list; `persistence` writes layout and per-row settings, reloads the page
and asserts they come back; `session` reloads and re-uploads the same dump
to prove the row list is restored in order; `wave2` also exports the row as
a MAT-file and checks its level-5 header; `a11y` checks the listbox/dialog
semantics, the modal focus trap, Escape behaviour, arrow-key selection and
the real tab-closer buttons.

`elaboration` uploads `generate.sv` / `arrayed.sv` with no dump open and
asserts the sidebar module tree shows the elaborated hierarchy
(`gen_top.g[0].u_leaf` ... `gen_top.g[3].u_leaf`, `arrayed_top.u[0]` ...
`arrayed_top.v[3]`) through the `__waveInsightSidebar` seam plus the row
`title` paths; `diagnostics` opens `link_errors.sv` and asserts the
elaboration diagnostics reach the editor as monaco markers, the
`__waveInsightLsp.diagnostics()` hook and the top-bar problem chip;
`exportTxt` exports a row as TXT and checks it is `<time> <value>` lines with
no header and the same values as the CSV export of the same row.

Known gap in `diagnostics`: only the picked top module is elaborated
(`link_top` for `link_errors.sv`), so the port-width mismatch declared in
`width_top` is not surfaced by the UI; the scenario asserts the unresolved
name path instead and documents this here.

Baseline on the Wave 6 build (2026-11-02, 49 scenarios):

```
=== summary: 356 passed, 0 failed, 0 pending (R2 layout, required), 0 deferred (R2 step 4), 2 retired (canvas rewrite) ===
```

The summary has five counters: **passed**, **failed**, **pending** (an R2 rule
the build does not implement yet; counts as a pass by default, a requirement
under `--layout`), **deferred** (an R2 step-4 rule, reported but never red) and
**retired** (an assertion the canvas rewrite made unobservable; the reason is
printed so nothing disappears silently).

## Fixtures

`qa/fixtures/` — all of them are read from here, never from the build:

| Fixture | Provenance |
|---|---|
| `top.sv`, `cpu.sv`, `legacy.v`, `malformed.sv`, `io_mode_wave.fst` | copied verbatim from `wave-insight-lib/tests/data/` |
| `generate.sv`, `arrayed.sv`, `link_errors.sv` | copied verbatim from `wave-insight-lib/tests/data/`, for the `elaboration` and `diagnostics` scenarios |
| `wave.vcd` | generated by `fixtures/generators/gen-vcd.js` (two scopes, 1-bit and multi-bit signals, x/z, ~1000 changes) |
| `mismatch.vcd` | generated by `fixtures/generators/gen-mismatch.js` from `wave.vcd` (root scope renamed `tb_top`) |
| `sparse.vcd` | generated by `fixtures/generators/gen-sparse.js` (one edge every 2000 time steps, for the cursor-snap checks) |
| `busy.vcd` | generated 32-bit-bus waveform for the `busBench` scenario (3.8 MB; no generator was kept, the file is the fixture) |
| `many_signals.vcd` | generated by `fixtures/generators/gen-many-signals.js` (2340 signals: a 2100-signal top scope plus 120 child scopes), for the `virtualization` scenario |
| `bad.vcd` | 22-byte file that is not a dump ("this is not a dump"), for the error-path checks |
| `binary.sv` | 10-byte source with invalid UTF-8 (`module \xff\xfe`), for the decode-error check |
| `wave-as-txt.txt` | `wave.vcd` renamed to `.txt`, for the "unsupported extension" check |

Regenerate the generated fixtures with:

```sh
node qa/fixtures/generators/gen-vcd.js
node qa/fixtures/generators/gen-mismatch.js
node qa/fixtures/generators/gen-sparse.js
node qa/fixtures/generators/gen-many-signals.js
```

## CI

`.github/workflows/rust.yml` has an advisory `qa` job: it downloads the
`web-dist` artifact produced by the `build` job and runs the stable subset
`empty,vcd,layout,layoutRules` in headless Chrome. It is marked
`continue-on-error: true` on purpose — the assertions are DOM/canvas driven and
a runner-image change (fonts, GPU/SwiftShader) can flake them. Drop that line
once it has a green track record and make it a required check.

## Reading the renderer

R1 replaced the per-row `<svg>` with one canvas. Every assertion that used to
read `.wave-row polyline|text|line` goes through two seams instead:

- `window.__waveDebug` — the renderer's own counters:
  `rows, changes, visible, decoded, decoded_bytes, redraw_ms, decode_ms,
  total_rows, scroll_top, first_row, x_axis, size, cursor1, cursor2`
  (page helpers: `__qa.debug()`, `__qa.dbg(field)`, `__qa.totalRows()`);
- a pixel hash/ink of `#wave-surface` (`__qa.surfaceHash()`,
  `__qa.surfaceInk(y0, y1)`), which is how "the row was recoloured",
  "the format changed" and "the canvas repainted" are checked now.

`R2` (see `docs/state-store.md`) additionally defines `render_count` on
`window.__waveDebug`; the splitter assertion uses it as soon as it exists.

## Locked layout constants (R2, asserted by `layoutRules`)

| Quantity | Value |
|---|---|
| collapsed editor | tab strip only, editor content height **0** |
| expanded editor content, minimum | **120 px** |
| `layout.editor_ratio` | clamped to **[0.15, 0.75]** on every write |
| wave pane minimum | **4 rows** (`4 * --row-h`) |

The wave minimum wins over the ratio: if the ratio would push the wave pane
below four rows, the ratio is trimmed, never the rows.

`layoutRules` also asserts `window.__waveDebug.render_count`, which must live in
`App` (a `Cell<u32>` bumped at the end of every `update` that returns `true`,
copied into `__waveDebug` from `rendered()`, never itself triggering a render).
It counts **shell** re-renders only; do not turn it into a per-component counter.

`__waveDebug.pane_render_count` is the same idea for `WaveShow` itself (carried
from `__wavePaneRenderCount`). The `tierA` scenario uses both: a cursor click and
a drag-zoom must move **neither** counter while the pointer moves, the commit
must land on mouseup (canvas hash / ruler change), and the shell must stay flat.

## Offline check

`monaco-offline.js` blocks every CDN host, loads the app from the staged build
and asserts the editor mounts, the AMD loader is the shipped copy, language
features (documentSymbol, SystemVerilog tokenizer) answer, no request leaves the
local origin and no asset 404s. `node qa/run.mjs --offline` runs it against the
same static server as the matrix (it used to start a second server on the same
port, which made the old `-Offline` flag fail with `EADDRINUSE`).
