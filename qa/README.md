# Wave Insight browser QA suite

Scripted end-to-end checks for the built front end. This used to live in
`%TEMP%\wi-qa`; it is a repository asset now, runs from its own fixtures and
fails the run when the matrix is red.

## Requirements

- Node.js 22 or newer (the runner uses `fetch`, `AbortSignal.timeout`,
  `fs.cpSync` and the global `WebSocket` the CDP client needs; Node 20 only has
  `WebSocket` behind a flag, which is why the CI jobs pin 22).
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
`signalsSelect`, `blockAdd`,
`colourAndDelete`, `layout`, `drop`, `badInputs`, `tabClose`, `deepWave`,
`deepEditor`, `deepSidebar`, `keysInEditor`, `addWithClosedTabs`, `extras`,
`tierA`, `layoutRules`, `traceEnd`, `waveTabs`, `navigation`, `busExpand`,
`busBench`, `busBands`, `snap`, `valueColumn`, `analog`, `analogStep`,
`mismatch`, `loadDrive`, `inlayHints`, `wave1`, `wave2`, `virtualization`,
`persistence`, `session`, `a11y`, `findValue`, `reload`, `rendererLabels`,
`rendererAlign`, `elaboration`, `diagnostics`, `exportTxt`, `corruption`,
`multiDump`, `bitRanges`, `completion`, `inspectorBits`, `splitView`, `largeDump`,
`bulkAdd`, `denseDump`.

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

`signalsSelect` is the Wave 19a regression for the SIGNALS list, which now
selects like the module tree instead of adding on a plain click: a plain click
selects one row (no wave row, `.sig-row.selected`/`aria-selected`, header
`Add selected (1)` and the sticky footer bar), Ctrl/Cmd+click toggles,
Shift+click ranges over the filtered order, and adding is explicit through a
double click, the per-row `+` (`button.sig-add`), the header `Add selected (N)`
and the footer bar - each one `RowsAdded` batch with the reducer wording
(`added N signals (M already in the wave)` / `no new signals: M already in the
wave`). It proves the filter prunes the selection (a hidden row is never
added), the virtualized list keeps its DOM bounded while Select all still
takes the whole logical list, and above the 2000-signal cap the button turns
`over-cap`/`aria-disabled` and the add is refused with
`— select fewer signals`. `clickToAdd` is the different source-editor path:
since Wave 20 a plain click on an identifier only **selects** it (inspector
focus + a status hint), `Ctrl+Enter` adds the signal under the cursor (one
candidate adds directly, several instances open the candidate picker), and the
wave label column draws the leaf signal name by default with the instance path
in a "Full path" checkbox/tooltip (see **Wave 20** below).

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
`split_ratio`. Unlinked, the right pane pans its own time axis on a plain
wheel, scrolls its rows on Shift+wheel, zooms on Ctrl+wheel and can be
drag-panned, and the left gestures do not touch it; linked again it follows the
left window. Because its `data-frame` is one paint behind, the wheel assertions
flush it with two window resizes before reading (see the Wave 18 section). A window resize keeps the right canvas
backing store in step with its css box, and closing removes `.split-view`,
keeps the toolbar and leaves `#wave-surface` as the only wave pane.

For the linked/unlinked checks the right pane publishes its last paint on
`.split-pane`: `data-frame="view=<axis>:<size> cursors=<c1>/<c2> scroll=<px>
dpr=<ratio> rows=<n> first=<i> end=<t> w=<px>"` and
`title="window <axis> @ <size>"` (plus `aria-label="Split pane: <name>"`), so
a freeze/re-sync assertion can compare the right pane's window directly
instead of relying only on the canvas pixels.

### Wave 18 behaviour

The mouse wheel follows Verdi now. Over the wave canvas the mapping is:

| gesture | main (interactive) pane | split right pane (unlinked) |
|---|---|---|
| plain wheel / trackpad swipe | pans the time axis forward/back (wheel down = forward in time; `deltaMode` pixels/lines/pages resolved, one event clamped to ±240 css px); the rows do not scroll | pans that pane's own time axis; its rows do not scroll |
| Ctrl+wheel | zooms at the pointer (unchanged factors) | zooms at that pane's pointer |
| Shift+wheel | scrolls the row list (`.wave-body` `scrollTop`), no time change | scrolls that pane's rows |
| wheel over the name column | scrolls the row list (browser native, unchanged) | n/a (the pane has no label column) |

`zoomPan` asserts the mapping through `window.__waveDebug` (`x_axis`, `size`,
`scroll_top`) and `.wave-body.scrollTop`: a pan moves `x_axis` and repaints while
`size` and the list stay put, Ctrl+wheel changes `size` and leaves the list
alone, Shift+wheel moves the list and leaves `x_axis` where it was (the old
assertions had plain and Shift+wheel swapped). It loads `many_signals.vcd` with
20 rows in a smaller window so the row scroll is observable, and `traceEnd` now
exercises the end clamp with the plain-wheel pan (before Wave 18 that gesture
scrolled the rows, so the clamp block was not actually panning).

`valueColumn` covers the new cursor value column. It is the LAST element of the
label cell (DOM order: name → ↗ → ⋯ → value cursor 1 → value cursor 2 →
waveform), so the two hover-only action icons can no longer leave an invisible
strip in front of the trace. Before a cursor no value element exists and the pane
publishes `--wave-value-ch: 0` (no reserved 54 px slot); after cursor 1 every
label cell carries exactly one `.wave-value`, after cursor 2 exactly two, and all
columns share one width computed once per render from the visible rows' formats.
The scenario also measures geometry: every value's right edge is 0..14 px left of
the canvas start (6 px cell padding + separator/slack) and no `.wave-more` icon
sits to the right of a value.

`formats` asserts the wave view prints bare digits: `pc` at the probed cursor
is `73` (Dec/Signed), `49` (Hex), `111` (Oct), `01001001` (Bin) and `I`
(ASCII), and no value carries a leading `'` radix prefix — the prefixed form
remains the data form used by the editor inlay hints, export and find.
`rendererLabels` asserts the canvas band labels through the additive
`window.__waveDebug` seam (`labels`, `labels_drawn`, `labels_truncated`,
`labels_hidden`): no label carries a prefix, narrow bands are truncated rather
than dropped (`labels_truncated > 0` at a Bin zoom where the old renderer drew
nothing) and every label containing `…` has exactly one. `rendererAlign` is the
1-bit alignment pixel probe: it puts cursor 1 exactly on a clk edge (`goto`
575), finds the cursor column by its colour and asserts the blue edge ink at
dx = -1 and dx = +1 around that column is within 20 % of itself, with the case
proven discriminating (the raw edge sits 0.25 px off the cursor column, which is
what made the old one-sided ink visible).

`reload` covers the Wave 18 reload fix. A dump's identity is file name + byte
size, so re-opening a re-simulated file used to focus the existing tab and keep
showing the OLD waveform. Two same-name/same-size dumps are generated at run
time from `wave.vcd` under `qa/artifacts/reload-qa/` (A is a verbatim copy, B
rewrites every `pc` value to `8'b10101010` without changing any line length,
and the scenario asserts the two byte sizes match): it opens A, adds `pc` and
`clk`, places a cursor, overwrites the picked path with B, picks it again and
asserts one tab, the row list kept, a repainted canvas, the cursor value `aa`
from B and the status `waveform reloaded — …`; picking the same bytes once more
gives `waveform loaded — …` again with no new tab and no repaint (the
unchanged-file dedupe contract). The fixture pair is generated, not committed,
because `qa/artifacts` is gitignored.

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
from the button and from Ctrl/Cmd+Enter. The scenario asserts the
scope add, the partial batch (`added N signals (M already in the wave)`), the
fully deduped batch (`no new signals: M already in the wave`), that one batch
re-renders the shell at most once, and the 2000-signal cap refusal
(`too many signals to add at once: N (limit 2000) - select fewer modules`) with
no rows added. The Wave 13 discoverability layer is covered too: every
`.tree-add` is always visible (computed opacity 1), a right-click / Shift+F10 on
a row opens `.row-menu[role=menu][aria-label="Row actions"]` (items
"Add all signals in this scope (N)", "Select for bulk add", "Select all visible
(M)", "Clear bulk selection"; Escape closes it), `.tree-select-all` (text
"Select all", aria-label "Select all visible modules") selects every visible row,
and the sticky `.bulk-bar[role=status][aria-label="Bulk selection"]` (count "N
signals selected", Add selected, Clear, the Ctrl/Shift hint) appears as a footer
so the rows do not move (the scenario asserts the first row's bounding top is
unchanged when the selection appears). Above 2000 signals `.tree-add-selected`
gains `.over-cap` + `aria-disabled="true"` and the bulk bar Add selected is
still clickable by design, so the actionable refusal is asserted from it.

### Wave 19 signal-list interaction

Since Wave 19a (released in 0.22.0) the SIGNALS sidebar list follows
the module tree's pattern instead of adding a signal on a plain row click
(`signalsSelect` is the regression):

| gesture | effect |
|---|---|
| plain click on a `.sig-row` | selects that row only - **no** wave row (the old click-to-add) |
| Ctrl/Cmd+click | toggles the row in the selection |
| Shift+click | selects the range from the anchor over the **filtered** order |
| double click on a row | adds that one signal (status `added <path> to the waveform`) |
| per-row `+` (`button.sig-add`, aria-label `Add <name> to the wave`) | adds that one signal |
| Enter on the focused row | adds the list's selection, or that row when nothing is selected |
| Signals head `Select all` (`.sig-select-all`) | selects every row of the filtered list |
| Signals head/footer `Add selected (N)` (`.sig-add-selected` / `.sig-bulk-add`) | adds the whole selection as **one** `RowsAdded` batch |
| footer `Clear` (`.sig-bulk-clear`) | empties the selection and hides the sticky `.sig-bulk-bar` |

Selection markers are `.sig-row.selected` and `aria-selected="true"`; the
header `Add selected (N)` is disabled at 0 and above the 2000-signal cap it
gains `.over-cap` + `aria-disabled="true"` with the "select fewer signal rows"
title, while the add is still clickable and refused with
`too many signals to add at once: N (limit 2000) — select fewer signals`.
The selection is pruned when the filter hides a selected row (it is never
added invisibly), cleared by a scope change and never persisted; adding does
not clear it, so a second Add selected reports the dedupe.

The page helper `window.__qa.clickSignal(name)` kept its old "add this one
signal" contract for the ~40 scenarios that use it: it now presses the row's
real `+` (`.sig-add`) button instead of the row, still returning `ok` /
`no-element:<name>`. The new helpers are `__qa.selectSignal(name, ctrl, shift)`
(a real row click that only selects), `__qa.dblclickSignal(name)` and
`__qa.signalSelection()` (row markers, header texts/labels/disabled/over-cap,
the footer bar count/buttons/hint and the logical header/bar counts - the
DOM only holds the virtualized window).

### Wave 19 block add (Verdi-style "add this code block")

Wave 19c adds every signal mentioned by a selected block of source code. The
`blockAdd` scenario is the regression:

| gesture | focus / target | effect |
|---|---|---|
| Ctrl+W, non-empty editor selection | the Monaco editor | adds the block's resolvable signals as **one** `RowsAdded` batch, status `added N signals (M already in the wave) — from the code selection`; the source tab is **not** closed |
| Ctrl+Shift+W | the Monaco editor | the same add (web-safe alias) |
| editor context menu | selection present | **"Add signals in selection to the wave"** (Monaco action id `wave-insight.addSelectionToWave`, group `navigation`, precondition `editorHasSelection`) - this is the guaranteed path in a browser |
| Ctrl+W, no selection | the Monaco editor | nothing happens (no add, no close: the editor owns its key) |
| Ctrl+W / Ctrl+F4 | anywhere else in the page | closes the active source tab (pre-Wave-19 behaviour, unchanged) |
| Ctrl+W | inside `.dump-tabs` | the dump strip still owns it (closes the active dump, `multiDump` unchanged) |

**Browser caveat:** a real browser reserves Ctrl+W for closing its own tab
before the page can `preventDefault` it; the page still receives the keydown and
the app routes it, and that listener path is what the matrix exercises through a
CDP `Input.dispatchKeyEvent` with the Ctrl modifier (the desktop WebView2 build
is where the shortcut is meant to be used). The context-menu action is the
reliable browser path and is asserted directly.

The scenario uses the app's own page seam
`window.__waveInsightEditorSelection()` -> `{file, startLine, startColumn,
endLine, endColumn, truncated, lines}` or `null` (the same Rust reader the
Ctrl+W handler uses, so the assertions and the shortcut cannot disagree) and the
new page helpers `__qa.editorSelection()`, `__qa.selectEditorBlock(...)`,
`__qa.editorAction(id)` and `__qa.blurEditor()`. Asserted behaviour:

* a top.sv declaration block (lines 9-11: `pc`, `data_out`, `pc16`) adds
  exactly 3 rows in one batch (render delta <= 2, measured 1) and the tab stays;
* Ctrl+Shift+W on `assign result[7:0] = data_out;` adds `result` and reports the
  dedupe (`added 1 signals (1 already in the wave) — from the code selection`;
  the "1 signals" plural is the pre-existing reducer wording);
* Ctrl+W with a collapsed cursor in the editor adds and closes nothing;
* the context-menu action on a cpu.sv block whose names are instantiated twice
  (`next_pc`, `pc_reg`, `pc`) adds nothing and reports
  `... ambiguous between instances (...) — click one in the source and press
  Ctrl+Enter to pick it` (ambiguous names are never guessed; the same plan also counts
  identifiers the elaborated design does not know, and a selection with no
  identifier at all says "no signal names");
* a comment-only block says `... has no signal names ...`;
* Ctrl+W outside the editor still closes the active source tab;
* no page exceptions.

The scan is word-based: comments and string literals are masked out, Verilog
keywords/backtick macros/escaped identifiers/system tasks/based literals are
skipped, the first/last selected line are clamped to their columns, a single
selection is capped at 500 lines / 4000 identifiers (`truncated`) and a resolved
batch above the shared 2000-row cap is refused with an actionable error tone.

### Wave 20: drop anywhere, Ctrl+Enter add, Delete focus, leaf labels

Four user-reported behaviours are covered by the existing scenarios (no new
scenario names, the runner's list is unchanged):

* **Drag & drop anywhere** (`drop`): a `dragenter` carrying files dispatched on
  the wave area shows the full-window `.drop-overlay` ("Drop to load"), a
  `dragleave` hides it again, and a drop away from the sidebar loads the dump -
  the sidebar tile is no longer the only accepted target. The scenario then
  closes the drawer (the top-bar button), re-checks `input[type=file]` is still
  mounted, and drops a source on `document.body`: the source tab appears, which
  proves the window listeners and the read queue survived the drawer close (the
  loader used to be unmounted with the sidebar).
* **Click selects, Ctrl+Enter adds** (`clickToAdd`): a plain click on `pc_reg`
  adds no row, opens no picker and names `Ctrl+Enter` on the status line;
  `Ctrl+Enter` then opens the two-candidate picker (monaco binds Ctrl+Enter to
  "insert line below", so the app claims it in the capture phase and the
  scenario asserts the editor line count did not move); picking a candidate adds
  the row. `deepEditor`, `mismatch` and `extras` follow the same two-step flow.
* **Delete removes the wave row, not source text** (`clickToAdd` and
  `navigation`): a real mouse click on a wave row moves the keyboard focus into
  the pane (`document.activeElement.id === "wave-body"`), then `Delete` removes
  the selected row while the monaco model value/line count stay byte-identical -
  the old bug left the focus in monaco's hidden textarea and deleted the source.
* **Leaf name by default, "Full path" checkbox** (`clickToAdd`,
  `persistence`): the drawn label (`.wave-name-text.innerText`) is the leaf
  signal name, its `title` carries the full instance path, the dock-tab
  `.dock-option input` switches the drawn label to the full path and back, and
  the option is persisted with the layout (`show_path` in
  `wave-insight.layout.v1`) so it survives a reload.

The `state()` seam reports `waveNames` as the row identity (the full path, i.e.
the label's `title`) so the pre-Wave-20 assertions keep their meaning, and
`waveLabels` as the text actually drawn; label assertions use `waveLabels`.

### Wave 19 desktop-only behaviour (not drivable by this matrix)

The Wave 19b desktop features - a webview reload (Ctrl+Shift+R) reopening the
dumps that were open, and SpinalHDL companion RTL autoload from
`simWorkspace/<design>/rtl/` (also `generated/`/`verilog/`, `.v`/`.sv`
only) - exist only in the Tauri build (`cfg(feature = "tauri")`). The browser
build has no dump path and no filesystem, so the browser matrix cannot drive
them and deliberately asserts nothing about them; the coverage is the Rust
host tests (`cargo test -p front-end --bins`, `cargo test -p app` for the
companion scanner/registry, `cargo check -p app` for the shell) plus the
manual Windows desktop checklist in `qa/artifacts/wave19-desktop.md`. No
scenario fakes desktop coverage.

`denseDump` covers the dense waveform shape that blew the desktop app up (the
user's >100 MB FST): a clock that changes on every time step, so the decoded
history is huge while the file stays moderate. The fixture is generated at run
time by `fixtures/generators/gen-dense.js` into `qa/artifacts/dense/`
(gitignored): 200k steps by default (~5 MB, 400k changes),
`QA_DENSE_STEPS=2000000` opts into a bigger one. The scenario asserts the row
paints, that at rest `decodedBytes` stays inside the documented budget
(64 MB + 8 MB slack) with `overBudget` false, that a visible dense row decodes
only a bounded window (the 2 MB regression bound a full-history decode cannot
meet), that three zoom cycles neither grow the decoded history nor the wasm
monotonically, that a second dense row does not double the decoded history, and
that a large FST (when available) stays inside the same policy -
`QA_LARGE_FST=<path>` adds that opt-in case.

With `QA_LARGE_FST=<path>` the scenario also runs the same contract on a real
FST: it loads (or is refused), adds the first row, bulk-adds a whole scope
(the user's likely path: many dense rows at once), scrolls through the added
rows, and asserts the decoded history and the wasm stay inside the same policy
after every step. The 97.5 MiB `large.fst` / 3.05 MiB `dense.fst` synthetic
fixtures come from `fixtures/generators/fst-gen/` (gen.c + build/regenerate
recipe; the files are not committed).

The FST reports could not be reproduced with a real FST here: no
`vcd2fst`/`gtkwave` is installed, the msys64 `iverilog`/`vvp` crash with
STATUS_ENTRYPOINT_NOT_FOUND, verilator 4.024 predates `--trace-fst` and the
network search for a legal public sample is unavailable in this sandbox, so the
dense VCD is an explicit proxy for the full-history decode path. Pre-fix
numbers (v0.16.0 build) for the record: a 259 MB / 10M-change dense VCD loaded
at wasm 511 MB; adding the clock row decoded `decodedBytes` 240 MB with
`overBudget` true and grew the wasm to 1.09 GB, a second dense row to 1.52 GB
and 480 MB decoded - per dense row ~+0.5 GB wasm / +229 MB decoded, no
eviction, which is the 3+ GB the user hit with several dense signals.

Real-FST measurements on the same pre-fix build (large.fst: 3,025 signals,
58,002,967 changes): load 1 s / wasm 205 MB; adding `top.clk` decodes 24 MB
(1M changes) - still under the budget; bulk-adding 3,024 signals and scrolling
through them peaks at decodedBytes ~67 MB (inside the 64 MB + slack budget) and
wasm 355 MB. So the FST's *size* alone is not the trigger: the renderer only
decodes visible rows and the scroll cache evicts. The blow-up happens when
visible rows are *dense* (changes per row): the dense proxy's 10M-change row
decodes to 240 MB with overBudget true and grows the wasm by ~0.5 GB per such
row - a handful of those is the user's 3+ GB.

With the bounded-window decode (v0.17.0) the same shapes decode only the
visible window: the 200k-change dense row drops to `decodedBytes` 148,000 B
(was 4,800,024 B), a second dense row no longer doubles it, and the zoom cycles
stay flat; on the real `large.fst` the `top.clk` row drops to 148,000 B (was
24,000,000 B), a 250-row bulk add settles at 5,328,000 B and scrolling over the
added rows at 10,656,000 B - all inside the budget with `overBudget` false.

Baseline on the Wave 13 build (2026-11-02, 58 scenarios):

```
=== summary: 524 passed, 0 failed, 0 pending (R2 layout, required), 0 deferred (R2 step 4), 2 retired (canvas rewrite) ===
```

Baseline on the Wave 18 build (2026-11-11, 0.21.0, 61 scenarios):

```
=== summary: 575 passed, 0 failed, 0 pending (R2 layout, required), 0 deferred (R2 step 4), 2 retired (canvas rewrite) ===
```

Baseline on the Wave 19 build (2026-11-11, 0.22.0, 63 scenarios):
`signalsSelect` and `blockAdd` are new (+24 and +11 assertions); the
`clickSignal` helper and the six raw sidebar-row clicks were migrated, with no
other assertion count change.

```
=== summary: 610 passed, 0 failed, 0 pending (R2 layout, run with QA_LAYOUT=1 to enforce), 0 deferred (R2 step 4), 2 retired (canvas rewrite) ===
```

Baseline on the Wave 20 build (2026-11-12, unreleased): the four user-reported
behaviours are covered by the updated `drop`, `clickToAdd`, `mismatch`,
`deepEditor`, `extras`, `navigation` and `persistence` scenarios (+27
assertions over the Wave 19 baseline; see **Wave 20** above). The full matrix ran
against a debug `trunk build` with headless Edge/Chromium through
`CHROME=<msedge.exe> node qa/run.mjs`:

```
=== summary: 637 passed, 0 failed, 0 pending (R2 layout, run with QA_LAYOUT=1 to enforce), 0 deferred (R2 step 4), 2 retired (canvas rewrite) ===
```

and `node qa/run.mjs --layout --scenarios empty,vcd,layout,layoutRules,waveTabs,tierA`
printed `=== summary: 50 passed, 0 failed, 0 pending (R2 layout, required), 0
deferred (R2 step 4), 0 retired (canvas rewrite) ===` - the always-mounted
sidebar does not change the layout rules.

The Wave 19 line comes from `node qa/run.mjs --dist front-end/dist-qa19`
(exit 0); `node qa/run.mjs --layout --dist front-end/dist-qa19` makes the R2
layout rules required and printed
`=== summary: 610 passed, 0 failed, 0 pending (R2 layout, required), 0 deferred
(R2 step 4), 2 retired (canvas rewrite) ===`, and
`node qa/run.mjs --offline --dist front-end/dist-qa19` passed 9/9 (editor
mounted, shipped loader, no external request, no failed load, no 404, no
exception). The two new scenarios ran twice in a row on the same build
(`--scenarios signalsSelect,blockAdd`) with identical green results. Wave 19 adds 35 assertions over the Wave 18 baseline (24 for
`signalsSelect`: plain click selects / ctrl toggle / shift range / explicit
add paths / filter pruning / cap refusal; 11 for `blockAdd`: helper + Ctrl+W
batch / Ctrl+Shift+W alias / no-selection / context-menu action / ambiguous and
no-name statuses / close-tab fallback).

The Wave 18 line comes from `node qa/run.mjs --layout --dist front-end/dist-qa`;
the plain matrix (`node qa/run.mjs --dist front-end/dist-qa`, no `--layout`)
printed the same 575 passed / 0 failed with "(R2 layout, run with QA_LAYOUT=1 to
enforce)" and exited 0, and `node qa/run.mjs --offline --dist front-end/dist-qa`
passed 9/9 (editor mounted, shipped loader, no external request, no failed load,
no 404, no exception). Wave 18 adds 51 assertions over the Wave 13/17 baseline
(526 records): the three new scenarios (27) and the rewritten wheel, value-column,
split-view, trace-end, format and find assertions (24).

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
node qa/fixtures/generators/gen-dense.js   # dense fixtures (written to qa/artifacts/dense at run time)

`fixtures/generators/fst-gen/` is a recipe rather than a fixture: `gen.c` +
`README.md` build a deterministic synthetic FST with GTKWave's libfst (clone,
gcc line, generate commands, sizes and tuning table). The generated
`large.fst` (102,273,753 B, 3,025 signals, 1M steps, 58,002,967 changes) and
`dense.fst` (3,203,089 B, 30k steps, 1,742,967 changes) live outside the repo
(e.g. `%TEMP%\wi-fst`) and are passed to the denseDump scenario with
`QA_LARGE_FST=<path>`.
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
- Wave 18 adds the band-label seam and the layout/alignment probes:
  `labels`, `labels_drawn`, `labels_truncated`, `labels_hidden` on
  `window.__waveDebug` (`__qa.bandLabels()`), `__qa.waveValueLayout()` for the
  label-cell/value-column geometry, `__qa.splitPaneFrame()` for the parsed
  split-pane `data-frame`, `__qa.dumpSnapshot()` for the reload scenario and
  `__qa.alignProbe()` for the cursor-column/edge-ink pixel probe.
- Wave 19 adds the waveform-side signal-list seams used by `signalsSelect`:
  `__qa.signalRow(name)`, `__qa.selectSignal(name, ctrl, shift)`,
  `__qa.dblclickSignal(name)` and `__qa.signalSelection()`; the legacy
  `__qa.clickSignal(name)` now clicks the row's `.sig-add` button (Wave 19a
  made a plain row click a selection, not an add).

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
