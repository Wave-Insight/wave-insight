# Roadmap

A living plan, updated at each wave boundary. It says what is being worked on and
in what order — not when it will ship. Design background lives in
[`rebuild-plan.md`](rebuild-plan.md) (R1–R3) and
[`state-store.md`](state-store.md) (the R2 store); this file links to them and
does not duplicate them.

## Where we are

The last tagged release is **0.7.0** (2026-11-02): one-canvas waveform
renderer, R2 state store, bus-bit expansion, Verdi-style navigation, CI gate,
fixed-point display, search options, CSV/TXT export, chunked large-dump loading
with a size guard, LSP diagnostics in the editor, the browser QA suite in `qa/`,
constant-bound generate unrolling + arrayed instances, sidebar virtualization,
layout/row display persistence, and the accessibility/keyboard pass. **Wave 4** is
landing in this wave on `main` (commit follows 0.7.0): elaboration diagnostics
merged into the editor's LSP diagnostics, criterion benchmarks plus an advisory
bench workflow, a cargo-deny supply-chain gate, per-dump session restore of the
wave rows, and MAT export. The `Unreleased` section of
[`CHANGELOG.md`](../CHANGELOG.md) is the authoritative list of what is on
`main` beyond 0.7.0.

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
| **2** | released in 0.6.0 | export signal values to CSV/TXT; large-dump loading UX (progress, cancel, 512 MB guard); LSP diagnostics in the editor + top bar; browser QA suite in `qa/` with an advisory CI smoke job |
| **3** | released in 0.7.0 | constant-bound generate unrolling + arrayed instances (Verdi-flat paths); `unresolved-name`/`width-mismatch` driver/load diagnostics; sidebar virtualization above 80 rows; layout + per-signal display persistence; accessibility/keyboard/tier-A-ruler pass; tauri file browser home directory; data-URI favicon |
| **4** | shipped in this wave | elaboration diagnostics merged into the editor's LSP diagnostics (deduplicated, republished on top-module change); criterion benchmarks (six groups / twenty benchmarks) + advisory bench workflow; `deny.toml` + `cargo deny` audit workflow + dependabot grouping and the tokio 1.54.1 security bump; per-dump session restore of the wave rows; MAT export |
| **5** | chosen for the next wave, not started | R2 step 3 + the R3 legacy deletion (carried over, see `rebuild-plan.md`/`state-store.md`); yew 0.21 + tauri 2 migration; multiple dumps / split view |
| **6** | later candidates | signal bundles (AXI, Avalon, …); SpinalHDL support; inline python over signal values |

Statuses are honest: *released* means a tagged release, *shipped in this wave*
means committed on `main` and waiting for the next release, *chosen* means the
next wave's agreed top items (not started), and *later candidates* are unordered.
Items can move between waves when the tree says a wave should be smaller.

Wave 5's top items, in order: the R2 step 3 + R3 legacy deletion (see
[`rebuild-plan.md`](rebuild-plan.md) and [`state-store.md`](state-store.md));
then the yew 0.21 + tauri 2 migration (the dependabot ignores expect it); then
multiple dumps / split view.

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