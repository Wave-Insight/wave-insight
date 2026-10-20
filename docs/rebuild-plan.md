# Rebuild plan

The project just went from a viewer to a Verdi-like debugger, and the front end
carries the shape of the old one. This is what gets rebuilt and what stays.

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

1. R1 renderer (it also removes most of the current bug surface)
2. R2 store + self hosted editor
3. R3 deletion pass, then `wasm-opt` and a size budget in CI

## Non-negotiables

- `cargo test -p wave-insight-lib -p back-end` stays green throughout
- every UI change is verified by driving the built app in a browser, not by
  `cargo check` alone
- one commit per finished step, with the reasoning in the body
