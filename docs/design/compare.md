# Comparing waveforms: per-tab member dumps and series rows

Status: shipped (unreleased). This note records why the feature is shaped the
way it is: what a comparison *is* in this store, why rows stayed keyed by their
path, how a member's signals stay reachable, and what is deliberately left out.

## The user's request

After a dump is loaded, load a second (and third, ...) dump to compare against
it. The add-signal list should show, after every signal name, coloured numbers
`1 2 3` saying which of the loaded dumps contain that signal, and adding a
signal should add it from every dump that has it, each trace in that dump's
colour.

## What a comparison is here

A dump tab owns its rows (`state::DumpEntry`). A comparison is a **per-tab
set** (`state::compare::CompareSet`) of up to seven extra dumps, called
*members*, with stable slots:

- slot 1 is the tab's own dump (the *base*), drawn in the row's own
  `setting.color` exactly as before;
- slots 2..=8 are members, each with a fixed palette colour
  (`COMPARE_COLORS[slot - 1]`) and the file name it was loaded from;
- a member is not a tab: it never appears in the dump strip, has no rows,
  viewport or cursors of its own, and is never persisted.

Slots are **not renumbered** when a member is removed: 2, 4, 5 stay 2, 4, 5 and
the next add takes the smallest free slot. Renumbering would move both the
sidebar badges and the trace colours underneath the user in the middle of a
comparison, which is exactly what a badge is supposed to prevent.

## Why rows stayed keyed by their path

The obvious alternative - rows that belong to one dump, with a "compare" that
duplicates them - would have doubled the row list and given every duplicated row
its own settings, order and selection. Instead a row is still
`Row { path, name, width, setting, ... }` and the renderer resolves that path
against every loaded dump:

- base: `SignalItem::index/width/values` (as before);
- each member: `SignalItem::series` - one `Series { slot, color, index,
  width, is_bool, values }` per member that has the path.

That is what makes "adding a signal adds it from every dump that has it" fall
out of the model instead of being a special case: one add creates one row, and
the row simply has more series. A member that lacks the signal contributes a
series with no index, draws nothing and costs no decode.

A signal that only a *member* has is addable too: the single add and the batch
add fall back to the member's hierarchy for the width instead of refusing the
path ("not in the loaded waveform"), and the row then draws only that member's
series.

## The badges and the union tree

`SignalRow` carries a `members` bitmask (bit 0 = base, bit `slot - 1` =
member) built once per tree by `SidebarTree::from_hierarchies`: one path lookup
per signal per member while a comparison is loaded, and none when it is not
(`from_hierarchy` is still the no-comparison path). The signal row renders one
`.sig-slot` per set bit, in slot order, with the slot's colour as a CSS custom
property - the same colour the trace draws in.

A signal only a member has would be invisible in the base dump's tree, so the
tree is the **union**: the base hierarchy first, then one synthetic root per
member, labelled `compare 2 — run2.vcd` and keyed under a reserved path prefix
(`#compare2`, which no Verilog hierarchy can declare). A member's scopes are
re-keyed under that prefix so `top` in the base and `top` in the member are two
rows; its *signal* paths are not rewritten, because the same dotted path is what
makes one wave row draw every dump's series.

## Drawing and the memory budget

The pane keeps its one bounded window fetch per dump: the base rows in one
`window_changes_many` pass, then one pass per member for the rows that resolved
in it. `WindowRequest`/cap/`carry_values_before` are unchanged, and the window
cache, `trim_windows` and the global decode budget count every series
(`item_bytes` sums the base window and the series windows; `row_bytes` is the
test seam for that arithmetic).

`canvas.rs` draws the extra series after the base trace on the same row: 1-bit
series as thinner step lines on the same level, bus series as outlined bands
inset by `SERIES_INSET` per member (no value labels - several overlapping label
sets would be unreadable), analog series as overlay polylines. The analog scale
is shared by the base and every series of that row, so the overlay does not lie
about relative magnitude. `PaintStats::series` counts the extra traces and is
published in `__waveDebug.paint` for the QA harness.

## Loading and limits

The toolbar's picker is a plain `<input type=file>` (works in the browser and
in the desktop webview) and the bytes go through `Waveform::open_bytes` - the
same path a wasm dump takes - with a 128 MB cap because this is a side feature
and the dump is parsed into the same wasm memory. A read or parse failure is a
status line that names the file (`compare::open_error`).

Deliberate MVP limits, all documented in the readme:

- **Base value column only.** The cursor readout shows the base dump's value;
  the comparison values are visible as their traces.
- **Export/find on the base dump.** They are whole-history operations on one
  waveform; extending them to series is a separate design.
- **No persistence.** Members are not in `localStorage` (rows are, as before).
- **No native mode.** The desktop native path draws one mmap'd dump over IPC and
  has no second reader; the toolbar explains it instead of pretending.
- **Time axis by tick.** Every series is drawn on the base dump's axis; a
  differing timescale is reported once when the member is loaded, because the
  traces are aligned by tick, not by seconds.
- **Seven members, 128 MB per picker read.**

## Files

- `front-end/src/state/compare.rs` - palette, slots, mask, status texts (pure,
  host tested).
- `front-end/src/state/{action,reduce,select}.rs` - the three actions, the
  per-tab set, the tree rebuild that keeps the module selection, the selectors.
- `front-end/src/nav/tree.rs` - `SignalRow::members`, `SlotLabel`,
  `from_hierarchies` (union tree, reserved root prefix).
- `front-end/src/wave_show/util/signal_things.rs` - `Series`, the per-member
  resolution, the series windows and the budget arithmetic.
- `front-end/src/wave_show/canvas.rs` - the series drawing and `PaintStats::series`.
- `front-end/src/wave_show/compare_bar.rs` - the toolbar (chips, picker,
  removal, status lines).
- `front-end/src/{app,module_struct/module,module_struct/signal_struct/signal,
  module_struct/signal_struct/signal_component}.rs` - the prop chain that gets
  the slot labels to the signal rows, the send of the compare set to the pane,
  and the waveform-aware add paths.
