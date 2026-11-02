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

`run.mjs` copies `front-end/dist` into a per-run `qa/.stage/<pid>/` (gitignored, so parallel runs do not collide), starts
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
`exportTxt`, `corruption`, `multiDump`, `bitRanges`, `completion`, `inspectorBits`,
`splitView`, `largeDump`, `bulkAdd`.

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

`corruption` is the browser-level regression proof for the Wave 7 parser
hardening: it uploads a truncated FST, an FST with a flipped length field
(the old multi-terabyte allocation abort) and a truncated VCD, and asserts
each one produces an error status that names the file and says what to do,
with no page exception — then loads `wave.vcd` and adds a row to prove the
app is still usable.

`multiDump` covers the multi-dump UI. The `div.dump-tabs` strip (role=tablist,
aria-label "Open dumps", title "N dumps open, M MB of dump files") is present
whenever at least one dump is open: with one dump it shows a single
`div.dump-tab` (`.dump-tab.active`, `button.dump-tab-name` with aria-selected
and the file name, `button.dump-tab-close` with aria-label "Close <name>"),
and a second dump adds a tab and activates it. It asserts the row lists are
per dump (adding `pc` to each gives `top.pc[7:0]` and `tb_top.pc[7:0]`, and
switching tabs shows the right list), that the status line reports "2 dumps
open", that re-opening the same file focuses the existing tab instead of
duplicating it, that closing the active tab hands over to the neighbour with
its rows, that closing the last tab removes the strip and brings the empty
state (`.start-hint`) back, that re-opening after the empty state restores
the strip and the remembered rows, and that Ctrl+W with the focus inside the
strip closes the active dump. The assertions are DOM/state based, never
style based.

`bitRanges` is the browser-level proof for the Wave 8 driver/load bit ranges:
it uploads `bit_ranges.sv` with no dump (the elaborator picks `bit_ranges` as
the top, which the scenario asserts so a change in that pick is visible), then
uses the `__qa.provider('hover', ...)` debug hook on the `q` declaration and
on a `q` usage: the hover markdown must name `bit_ranges.q`, count the two
drivers and carry "- driven here: q[3:0], q[7:4]". A hover on `y` (driven as
a whole signal) must have no driven-here line, which keeps the assertion
specific to bit ranges rather than to any driver.

`completion` and `inspectorBits` cover the Wave 10 editor/inspector features.
`completion` uses the completion provider seam: `window.__waveInsightProviders`
now also carries the 6th provider,
`completion.provideCompletionItems(model, position, context, token)` (the
4-argument monaco form; `__qa.provider('completion', file, line, col)` wraps
it), plus `completion.triggerCharacters === ["."]`. The scenario asserts the
plain mode (enclosing module members and module names, with the monaco kinds
Variable 4 / Field 3 / Module 8 / Reference 21), that a partial prefix
narrows the list (`.cl` -> `clk` only), that `u_cpu.` returns only that
module's members with a kind and a detail, and that an unknown qualifier and
a position before the first module return an empty list. `inspectorBits`
opens `bit_ranges.sv`, selects `q` and asserts the Drivers rows render
`<span class="inspector-bits" title="q[3:0]">q[3:0]</span>` between the name
and the size for the two sliced drivers (`q[3:0]`, `q[7:4]`), while a
whole-signal driver (`y`) adds no range label.

`splitView` covers the two-dump split view. With one dump there is no
`.split-toolbar` and no `.split-view` (the DOM is exactly the old one). With
two dumps the toolbar (role=toolbar, aria-label "Split view") appears; its
toggle (`aria-pressed`) opens a `.split-view` that keeps the interactive left
`.wave-pane` (`#wave-surface`) and adds a read-only `.split-pane` (name,
"read only" mode, `canvas.split-canvas`). While open, the divider is a
`.split-separator` (role=separator, aria-orientation=vertical, aria-label
"Split divider", aria-valuemin 25 / aria-valuemax 75, aria-valuenow, tabindex
0); ArrowLeft/ArrowRight move it by 2 %, Home/End jump to 25/75, and dragging
it shows a `.split-shield` overlay, previews `--split-ratio` live and commits
on release. The ratio is persisted in `wave-insight.layout.v1` as
`split_ratio`. Unlinked, the right pane has its own row scroll (plain wheel),
zoom (Ctrl+wheel) and pan (drag) and the left gestures do not touch it; linked
again it follows the left window. A window resize keeps the right canvas
backing store in step with its css box, and closing removes `.split-view`,
keeps the toolbar and leaves `#wave-surface` as the only wave pane.

For the linked/unlinked checks the right pane publishes its last paint on
`.split-pane`: `data-frame="view=<axis>:<size> cursors=<c1>/<c2> scroll=<px>
dpr=<ratio> rows=<n> first=<i> end=<t> w=<px>"` and
`title="window <axis> @ <size>"` (plus `aria-label="Split pane: <name>"`), so
a freeze/re-sync assertion can compare the right pane's window directly
instead of relying only on the canvas pixels.

`largeDump` covers the memory-first behaviour on big dumps. Its fixtures are
generated at run time by `fixtures/generators/gen-large.js` into
`qa/artifacts/large/` (gitignored, deterministic LCG): a realistic shape with
thousands of signals, 1/8/16/32/64-bit buses and many time steps. The default
size is 30 MB (a few seconds); bigger sizes are opt-in with
`QA_LARGE_MB=348,480`. The scenario asserts that a load ends in either
`waveform loaded` or the actionable refusal (never a crash or blank page), that
`window.__waveDebug.memory` publishes numeric accounting
(`wasmBytes`, `fileBytes`, `decodedBytes`, `budget`, `dumps`, `unloaded`,
`released`, `overBudget`) with the decode cache inside its budget (or the
documented `overBudget` soft flag) and the wasm bytes inside the 1280 MB policy
cap, that rows can be added and scrolled, that the dump-tabs strip and the hook
count both dumps in a split view, that closing a dump returns its bytes
(`dumps`/`fileBytes` drop, `released` grows), and that a sparse 513 MB file is
refused with the documented 512 MB message and guidance.

The wide shape (200,000 signals / ~16 MB) loads and its first row adds: the
earlier wasm panic on it came from the fixture generator, not from the app.
`gen-large.js` used to build two-character ids from a 63-character alphabet, so
above 3969 signals the ids collided (`ALPHABET[63]` is undefined) and a
mixed-width dump declared the same id with conflicting widths; the parser then
trapped. The generator now encodes the index as a base-94 counter over the
printable id alphabet, so every signal gets a unique id at any count (the
scenario asserts the generated fixtures have unique ids and one width per id).
The real bug behind the trap - the parser accepting conflicting-width ids - is
the `corruption` scenario's `min_width_mismatch.vcd` regression: it must be
refused with the actionable message, never trap the wasm.

The oversized-guard fixture carries a valid VCD header before its huge zero
body: the format sniff runs before the 512 MB size guard, so a zero-filled
file would be reported as an unknown format instead of the size refusal.
Bigger well-formed sizes are opt-in with `QA_LARGE_MB` (the estimate policy
refuses one that cannot fit: e.g. a 501 MB file is refused up front with the
estimated need versus the free wasm memory).

`bulkAdd` covers the module-tree multi-select and the batch add. Every
`.tree-row` exposes `aria-selected` and `.tree-row.selected` (`.chosen` is the
single focus instance) and carries a `button.tree-add` whose aria-label/title
is `Add all signals in <scope path>`; the Modules panel head has
`button.tree-add-selected` with text `Add selected (N)`, aria-label
`Add selected signals`, title `Add N signals from the selected scopes` and
`disabled` at 0. Plain click focuses a row and resets the selection; Ctrl/Cmd+click
toggles; Shift+click selects the visible range from the anchor; the batch runs
from the button and from Ctrl/Cmd+Enter. The scenario asserts the scope add, the
scope add, the partial batch (`added N signals (M already in the wave)`), the
fully deduped batch (`no new signals: M already in the wave`), that one batch
re-renders the shell at most once, and the 2000-signal cap refusal
(`too many signals to add at once: N (limit 2000) - select fewer modules`) with
no rows added.

Baseline on the Wave 12 build (2026-11-02, 57 scenarios):

```
=== summary: 503 passed, 0 failed, 0 pending (R2 layout, required), 0 deferred (R2 step 4), 2 retired (canvas rewrite) ===
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
| `bit_ranges.sv`, `bit_ranges_generate.sv` | copied verbatim from `wave-insight-lib/tests/data/`, for the `bitRanges` scenario (two disjoint driver slices on `q`, and a generate-loop select) |
| `wave.vcd` | generated by `fixtures/generators/gen-vcd.js` (two scopes, 1-bit and multi-bit signals, x/z, ~1000 changes) |
| `mismatch.vcd` | generated by `fixtures/generators/gen-mismatch.js` from `wave.vcd` (root scope renamed `tb_top`) |
| `sparse.vcd` | generated by `fixtures/generators/gen-sparse.js` (one edge every 2000 time steps, for the cursor-snap checks) |
| `busy.vcd` | generated 32-bit-bus waveform for the `busBench` scenario (3.8 MB; no generator was kept, the file is the fixture) |
| `many_signals.vcd` | generated by `fixtures/generators/gen-many-signals.js` (2340 signals: a 2100-signal top scope plus 120 child scopes), for the `virtualization` scenario |
| `bad.vcd` | 22-byte file that is not a dump ("this is not a dump"), for the error-path checks |
| `binary.sv` | 10-byte source with invalid UTF-8 (`module \xff\xfe`), for the decode-error check |
| `wave-as-txt.txt` | `wave.vcd` renamed to `.txt`, for the "unsupported extension" check |
| `truncated.fst` | generated by `fixtures/generators/gen-corrupt.js`: the first half of `io_mode_wave.fst`, for the `corruption` scenario |
| `flipped.fst` | generated by `fixtures/generators/gen-corrupt.js`: `io_mode_wave.fst` with the first data section's uncompressed-length field set to `u64::MAX`, so the library's plausibility check must reject it |
| `truncated.vcd` | generated by `fixtures/generators/gen-corrupt.js`: `wave.vcd` with its declaration header cut before `$enddefinitions`. A body-only truncation is still a structurally valid (shorter) VCD that wellen accepts, so the corrupt fixture cuts the header |
| `min_width_mismatch.vcd` | the fe-auditor's 298 B repro, committed verbatim: one id `!` declared twice with conflicting widths (64 and 8 bit) and a 64-bit body value; the parser used to trap on it, so the `corruption` scenario asserts the actionable refusal instead |
| `wide_scope.sv` | generated fixture for the `bulkAdd` cap check: `wide_top` instantiates three 1000-signal leaves, so selecting all three asks for 3000 signals, above the 2000-signal bulk-add limit |

Regenerate the generated fixtures with:

```sh
node qa/fixtures/generators/gen-vcd.js
node qa/fixtures/generators/gen-mismatch.js
node qa/fixtures/generators/gen-sparse.js
node qa/fixtures/generators/gen-many-signals.js
node qa/fixtures/generators/gen-corrupt.js
node qa/fixtures/generators/gen-large.js   # large-dump fixtures (written to qa/artifacts/large at run time)
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
