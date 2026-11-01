# Rebuild plan

The project just went from a viewer to a Verdi-like debugger, and the front end
carries the shape of the old one. This is what gets rebuilt and what stays.

## Status (at the time of writing)

- **R1 — done except the truly windowed read.** The wave body is one `<canvas>`,
  device pixel ratio aware, drawing only the visible rows and decimating the
  decoded history to one sample per pixel; the decode cache is budgeted and a row
  is unloaded when it scrolls out of view. The signal-name column, the sidebar
  module tree and the signal list are all virtualized (only the rows around the
  viewport exist in the DOM; lists of 80 or fewer keep the plain DOM). Open: the
  renderer still decodes a row's whole history (`decoded_changes_many`) instead of
  only the visible time window as the plan above describes.
- **R2 — steps 1–2 done, 3–4 open.** `AppStore` with typed actions, slice
  subscriptions and tier-A splitters is in (`docs/state-store.md`); the wave pane
  and time bar are subscribers and gestures re-render nothing, and an optional
  persister now writes the layout and the per-signal display settings to
  `localStorage`. Step 3 is only at its first increment (the dump handle lives in
  the store); the sources/design/focus slices and the LSP/editor effects are not
  written yet, and step 4 is partial (layout rules in, legacy shell teardown
  open).
- **R3 — the desktop build already parses in wasm; the deletion pass is open.**
  `src-tauri` only opens the file dialog and returns bytes, and both builds use
  the same `Waveform::open_bytes` reader, so the "tauri uses the backend directly"
  half is done. What is left is deleting the old `data_struct`/legacy shells,
  `SharedDesign` and the `nav/` glue (tracked as R2 step 4 and R3).
- **wasm-opt and the size budget landed in CI (0.6.0).** The Rust workflow pins
  trunk 0.22.1 (so the wasm-bindgen/wasm-opt defaults cannot drift with a release)
  and fails the build when the release wasm exceeds **10 MiB**
  (`WASM_SIZE_BUDGET`); the size task is therefore no longer only an Order item.

## Keep (it is tested and it is the hard part)

- `wave-insight-lib::waveform` — wellen based, VCD/FST/GHW, streamed value queries
- `wave-insight-lib::design` — sv-parser front end, elaboration, spans, drivers/loads
- `wave-insight-lib::lsp` — the language server core, desktop and wasm
- the text based transports are not worth keeping; see "drop"

## Rebuild

### R1. Renderer and lists (biggest win)

Today every signal row is its own `<svg>` with a `<polyline>` built from the
signal's **entire** history, and every list renders all its rows. That does not
survive a real dump.

- one `<canvas>` for the wave body, redrawn on scroll/zoom/pan; only the rows in
  view are drawn, and only the changes inside the visible time window are read
  (`changes_in_window` seeded by `value_at`)
- device pixel ratio aware, so lines stay 1px on a hidpi screen
- name column, signal list and module tree become virtualized DOM lists
- cursors, drag-to-zoom, `z`/`f`, format switching and the settings dialog keep
  working, on top of the new renderer
- needs one thing from the library: a cached per-signal accessor, because
  `changes_in_window` currently re-streams the file on every call

### R2. State and shell

- one `AppStore` (`Rc<RefCell<State>>` + typed actions) instead of passing
  `Rc<RefCell<ModuleValue>>` through four layers of props
- the editor stops depending on a CDN: ship a pinned monaco build with the app
- the four panes become components that each own their slice of state

### R3. Drop the legacy path

- remove `data_struct::Module` / `ModuleValue` and everything that only exists
  to feed them (`legacy_module`, `to_legacy`, the tauri `get_value` round trip)
- the desktop build uses the waveform backend directly, like the browser does
- the server transport is either rebuilt on the new API or deleted; it has been
  a demo since 2023 and is currently broken

## Order

1. R1 renderer (it also removes most of the current bug surface) — done except the
   sidebar list/tree virtualization
2. R2 store + self hosted editor — store steps 1–2 done, steps 3–4 open
3. R3 deletion pass — open; the `wasm-opt`/size-budget part is already landing in
   CI (see Status)

## Non-negotiables

- `cargo test -p wave-insight-lib -p back-end` stays green throughout
  (CI also runs the front end's native tests, formatting and clippy)
- every UI change is verified by driving the built app in a browser, not by
  `cargo check` alone
- one commit per finished step, with the reasoning in the body