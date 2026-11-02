# Multi-dump and split view

Design note for opening several dumps at once and for the later split view. It
builds on the store described in [state-store.md](../state-store.md): when this
note was written, exactly one dump was open and the wave rows, the viewport and
the derived tree belonged to it implicitly. This note makes that relationship
explicit so a second dump can be added without forking the whole UI.

Status: **steps 1-2 shipped in Wave 8; step 3 shipped as an MVP in Wave 10 and
polished in Wave 11.** The per-dump model, the dump tab strip and the side-by-side
split landed; the right pane now scrolls/zooms/pans on its own in unlinked mode,
linked mode freezes when the toggle turns unlinked, and the divider is draggable
and persisted. The remaining limits are listed here: unlinked-mode cursor
independence, a visible row scrollbar, drag-pan captured by the window (it stops
at the pane edge), and whether the split is open is not persisted across reloads.

Implementation result (Wave 8), where reality differed from the design below:
`State.dumps` (in `front-end/src/state/dumps.rs`) owns one `DumpEntry` per tab -
handle, time range, rows, viewport/cursors, inspector focus and session-restore
state. The no-dump, sources-only workflow (load sources, add signals, no
waveform yet) keeps the old `rows`/`viewport`/`focus` fields as **scratch**
slices; the selectors prefer the active dump entry when there is one, so the
panes did not have to change shape. `DumpAdded`/`DumpTabActivated`/`DumpClosed`
drive the tabs (dedupe a re-open by `DumpId`, close activates a neighbour), the
reducer enforces the 1.5 GiB open-dumps budget, and the tab strip is part of the
shell (one tab per open dump; the last close returns to the empty state). The
shell subscribes a tab-only `DumpsTabsSlice` (name/size per tab, active index,
total bytes) whose version is bumped only when the tab list or the active index
changes, so a viewport/cursor commit changes the active dump's slices without
re-rendering the shell.

Implementation result (Wave 10), the split view: `State.split` holds
`Option<Split { right, linked }>` (`right` is always a dump other than the active
one) with `SplitOpened`/`SplitClosed`/`SplitRightChanged`/`SplitLinkedToggled`;
the selectors can read a specific dump (`rows_of`, `dump_name_of`, the pane
handle), so the panes are per-dump. `WaveShow`'s `SplitPane`
(`front-end/src/wave_show/split.rs`) is a read-only canvas pane: same machinery,
decodes only the visible rows, owns no editing surface and does not publish
`window.__waveDebug`. Linked mode uses the pure `linked_view`/`linked_cursor`
mapping in `state/split.rs`: fraction of the left trace onto the right trace,
clamped, with the cursors drawn at the mapped times. The toolbar renders only
with two or more dumps (a single dump has no split DOM), switching tabs swaps the
panes, and closing a dump remaps the right pane or closes the split when fewer
than two remain.

Implementation result (Wave 11), split polish: the unlinked pane owns its view and
commits it through `ViewCommittedFor { dump, view }` to that dump's entry (the
shell never subscribes that change); it zooms at the pointer with Ctrl+wheel,
scrolls the virtualized row window (clamped by `state::split::max_row_scroll`) and
drag-pans, while the linked pane ignores gestures and stops following the left
viewport when it is unlinked. A `ResizeObserver` repaints at the new css box so
the canvas backing store tracks window resizes and divider moves.
`.split-separator` is a `role=separator` with aria values, 2% arrow steps and
Home/End; `SetSplitRatio` clamps to 0.25-0.75 and persists `Layout.split_ratio`
(default 0.5, serde-defaulted so an old saved layout loads).

## 1. Why

Comparing two runs of the same testbench (before/after a fix, two seeds, RTL vs
gate level) is a daily Verdi workflow: open both dumps, look at the same signals,
switch between them, later show them side by side on one time axis. When this
note was written the app could not do it: opening a second dump replaced the
first one, and the row list of the first was only recoverable by reopening the
file (session restore).

## 2. What is per-dump today

| Slice | Today | Must become per-dump |
|---|---|---|
| `State.dump` (`DumpHandle`, identity, time range) | one | one entry per open dump |
| `State.rows` (`Rc<Vec<Row>>` + version) | global | per dump |
| `State.viewport` (zoom/pan + cursors) | global | per dump (each dump has its own time axis) |
| sidebar tree | derived from the open dump's hierarchy | derived from the active dump |
| `State.sessions` (remembered row lists) | keyed by `DumpId` | unchanged — it already is per dump |
| `State.row_prefs` (display settings by signal path) | global by path | unchanged — the same signal keeps its format across dumps |
| `State.focus.signal` | one | per dump, or global with the active dump deciding what it means |
| LSP `top` | derived from the open dump's root | follows the active dump |
| `window.__waveDebug` counters | active renderer | unchanged (they describe the visible surface) |

## 3. Target model

```rust
pub struct Dumps {
    pub items: Rc<Vec<DumpEntry>>,
    pub active: usize,
    /// Bumped on every change, like `Rows.version`, so a subscriber compares one
    /// integer; the per-entry slices compare by their own versions.
    pub version: u64,
}

pub struct DumpEntry {
    pub id: DumpId,              // file name + byte size, what sessions are keyed by
    pub handle: Rc<Waveform>,    // the only copy; decoded data stays in the library cache
    pub time_range: Option<TimeRange>,
    pub range_from_body: bool,
    pub rows: Rows,              // the wave rows of this dump (order, settings, selection)
    pub viewport: Viewport,      // committed zoom/pan + cursors of this dump
    pub focus: Focus,            // inspector selection while this dump is active
}

State { dumps: Dumps, ... } replaces `State { dump, rows, viewport }`.

Rules:

- **Selectors stay the same shape**: `select::rows`, `select::viewport`,
  `select::time_range`, `select::dump_waveform` read the *active* entry. Panes
  subscribe exactly as they do now, so the renderer, the time bar and the wave
  pane need no signature changes.
- **Actions take an index when they may address a non-active dump** (`DumpClosed`,
  `DumpTabActivated`), otherwise they act on the active one. None of the row or
  viewport actions need an index.
- **The row funnel changes**: `State::set_rows` becomes
  `State::set_rows(dump: usize, items: Vec<Row>)`; every row action already goes
  through it, so the persistence hook (`remember_rows`) stays in one place and
  records under that dump's identity.
- **`DumpOpened` becomes `DumpAdded { id, bytes, handle }` + activation**. A
  second open of the same identity focuses the existing tab instead of adding a
  duplicate (the same dedupe rule the sources slice already uses).

## 4. UI

1. A dump tab strip at the top of the wave frame (above the ruler, next to the
   existing `Waveform / Drivers / loads` tabs — which stay). One tab per dump,
   label = file name, a close button, active tab highlighted; the same
   `role=tablist/tab` markup the source tabs use, so the accessibility rules and
   the keyboard handling (Ctrl+W closes the active *source* tab today; with two
   tab strips it closes the focused strip's active tab) carry over.
2. The sidebar tree, the inspector and the wave pane always describe the active
   dump; switching tabs re-renders those three slices only (the store does this
   through the versioned slices it already has).
3. Memory: every open dump keeps its file bytes in wasm memory. The status line
   reports the summed size; opening more than a configured budget (e.g. 1.5 GiB
   of file bytes) is refused with the same actionable message the single-dump
   size guard uses today.

## 5. Split view (second step, same model)

Two wave panes side by side, each showing one dump (or the same dump with
independent zoom). The store gains `split: Option<{ left: usize, right: usize }>`;
each pane renders its own dump's rows from the same `Dumps` slice and owns its
`Viewport` copy while the split is active. The time axis can be linked (one
commit applies to both) or independent; start with linked, behind a toggle.

## 6. Persistence

The session store is already keyed by `DumpId`. With N tabs it becomes N
entries (cap: the sessions cap, e.g. 20 dumps); restoring the rows of a dump
happens when that dump's tab is created, not globally. `row_prefs` (display
settings by signal path) stays global on purpose: the same signal keeps its
format across runs, which is what a user comparing two runs wants.

## 7. Migration order

Each step is independently shippable and verified by the full QA matrix plus the
new scenarios below.

1. **State refactor, one dump**: introduce `Dumps` with a single entry, move
   `rows`/`viewport`/`focus` into the entry, update selectors and the reducer
   tests. No user-visible change; the DOM contract must not move.
2. **Open more than one**: loader adds a tab instead of replacing; tab strip;
   switching; closing (closing the active tab activates its neighbour; closing
   the last one leaves the app in the empty state the start hint describes);
   duplicate identity focuses the existing tab; the memory budget guard.
3. **Split view**: two panes over the same `Dumps` slice, linked time axis first.

## 8. Verification

- Host tests: reducer (add/activate/close/dedupe/budget), selectors always read
  the active entry, session restore per dump tab.
- Browser QA: a new `multiDump` scenario — open `wave.vcd` and `mismatch.vcd`,
  add a row to each, switch tabs and assert rows/tree/status/cursor follow the
  active dump, close a tab and assert its neighbour becomes active, then assert
  the memory-budget message with a faked large size if feasible. A `splitView`
  scenario joins in step 3.
- The existing 330 assertions must stay green; on a single-dump session nothing
  in the DOM changes.

## 9. Risks

- **Memory**: two 105 MB dumps are ~1.8 GiB of wasm memory in practice (the
  renderer cache is budgeted per signal already); the budget guard and a visible
  size readout are not optional.
- **Debug hooks**: `window.__waveDebug` describes the visible surface, so it
  follows the active dump; assertions must not read it while switching tabs.
- **LSP top**: it follows the active dump's root; a switch republishes diagnostics
  (that path already exists for `TopChanged`).
- **Session restore ordering**: tabs are created in open order, so the last
  active tab should be persisted too if we want the workspace back exactly.

## 10. Non-goals

Multi-window, cross-dump signal comparison in one row (two histories in one
band), and comparing dumps by content hash. They can be built on this model later.