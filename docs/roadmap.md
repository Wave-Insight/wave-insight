# Roadmap

A living plan, updated at each wave boundary. It says what is being worked on and
in what order — not when it will ship. Design background lives in
[`rebuild-plan.md`](rebuild-plan.md) (R1–R3) and
[`state-store.md`](state-store.md) (the R2 store); this file links to them and
does not duplicate them.

## Where we are

The last tagged release is **0.6.0** (2026-11-02): one-canvas waveform
renderer, R2 state store, bus-bit expansion, Verdi-style navigation, CI gate,
fixed-point display, search options, CSV/TXT export, chunked large-dump loading
with a size guard, LSP diagnostics in the editor, and the browser QA suite in
`qa/`. **Wave 3** is landing in this wave on `main` (commit follows 0.6.0):
constant-bound generate unrolling and arrayed instances with Verdi-style flat
paths, `unresolved-name`/`width-mismatch` driver/load diagnostics, sidebar
virtualization above 80 rows, layout + per-signal display persistence, the
accessibility/keyboard pass, the tauri file browser starting in the home
directory, and a data-URI favicon. The `Unreleased` section of
[`CHANGELOG.md`](../CHANGELOG.md) is the authoritative list of what is on
`main` beyond 0.6.0.

## How a wave works

A wave is a set of improvements with **disjoint scopes** (own crates or file
areas) so several slices can land in parallel without fighting over the same
files. Every slice is verified before it is committed:

- the host tests that cover it (`cargo test --workspace --exclude front-end
  --exclude app` and, for the UI's non-DOM code, `cargo test -p front-end
  --bins`);
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets -- -D
  warnings` — both are CI gates, not suggestions;
- the browser QA matrix (headless Chrome against the built `dist`), which is the
  only proof for user-visible behaviour;

A wave is committed to `main` when its slices are done and green. Releases are
cut when the accumulated waves are coherent for users; no wave promises a date.

## Waves

| Wave | Status | Scope (intent) |
|---|---|---|
| **1** | released in 0.6.0 | CI gate; documentation truth pass; fixed-point display; search options; GHW time axis; LSP port connections; library panic hardening |
| **2** | released in 0.6.0 | export signal values to CSV/TXT (MAT is *not* implemented); large-dump loading UX (progress, cancel, 512 MB guard); LSP diagnostics in the editor + top bar; browser QA suite in `qa/` with an advisory CI smoke job |
| **3** | shipped in this wave | constant-bound generate unrolling + arrayed instances (Verdi-flat paths); `unresolved-name`/`width-mismatch` driver/load diagnostics; sidebar module-tree/signal-list virtualization above 80 rows; layout + per-signal display persistence; accessibility/keyboard/tier-A-ruler pass; tauri file browser starts in the home directory; data-URI favicon |
| **4** | chosen for the next wave, not started | forward elaboration diagnostics through the LSP (closes the gap this wave left); R2 step 3 + the R3 legacy deletion (carried over, see `rebuild-plan.md`/`state-store.md`); yew 0.21 + tauri 2 migration; multiple dumps / split view |
| **5** | later candidates | `.mat` export; benchmarks (the `nightly-bench` gate still enables nothing); signal bundles (AXI, Avalon, …) |

Statuses are honest: *released* means a tagged release, *shipped in this wave*
means committed on `main` and waiting for the next release, *chosen* means the
next wave's agreed top items (not started), and *later candidates* are unordered.
Items can move between waves when the tree says a wave should be smaller.

Wave 4's top items, in order: forward the elaboration diagnostics
(`unresolved-name`, `width-mismatch`, non-constant generate bounds) through the
language server so they reach the editor; then the R2 step 3 + R3 legacy deletion
(see [`rebuild-plan.md`](rebuild-plan.md) and [`state-store.md`](state-store.md));
then yew 0.21 + tauri 2 migration; then multiple dumps / split view.

## Invariants every wave keeps

- The browser QA matrix is green; no assertion is deleted without an equivalent
  one replacing it (a `RETIRED` assertion must state what covers the behaviour).
- No user-visible regression in a shipped feature; performance of a large dump
  is part of that contract.
- The README's *Known limitations* stay true — a wave that changes one of them
  updates the README.
- CI stays green: formatting, clippy with warnings denied, host and front-end
  tests, and the wasm size budget.
- Docs and `CHANGELOG.md` move with the code, not after it.
- Work is sliced into an owner and a write scope; overlapping scopes are ordered
  rather than run concurrently.

## References

- [`rebuild-plan.md`](rebuild-plan.md) — the R1 renderer lists, R2 state/shell
  and R3 legacy-deletion plan, with their current status.
- [`state-store.md`](state-store.md) — the R2 store design and what is
  implemented (steps 1–2, first increment of step 3).
- [`CHANGELOG.md`](../CHANGELOG.md) — what shipped in each release and what is
  on `main` now.
- [`readme.md`](../readme.md) — user-facing behaviour and the current Known
  limitations.