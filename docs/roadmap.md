# Roadmap

A living plan, updated at each wave boundary. It says what is being worked on and
in what order — not when it will ship. Design background lives in
[`rebuild-plan.md`](rebuild-plan.md) (R1–R3) and
[`state-store.md`](state-store.md) (the R2 store); this file links to them and
does not duplicate them.

## Where we are

The last tagged release is **0.5.0** (2026-10-22): one-canvas waveform
renderer, R2 state store with tier-A splitters, bus-bit expansion and Verdi-style
navigation (next transition, goto time, draggable ruler, select-all). **Wave 1**
is committed on `main` at `79e0d57` and not released yet: CI gate (tests, fmt,
clippy, 10 MiB wasm budget), documentation truth pass, fixed-point display,
search options (case / whole word / regex), GHW time axis, LSP resolution on port
connections and instance/module names, library panic hardening. **Wave 2** is
landing in this wave on `main` (commit follows Wave 1): export as CSV/TXT,
large-dump loading UX (chunked read, progress, cancel, 512 MB guard), LSP
diagnostics in the editor plus a top-bar problem count, and the browser QA suite
moved into `qa/` with an advisory CI smoke job. [`CHANGELOG.md`](../CHANGELOG.md)
Unreleased is the authoritative list of what is on `main` beyond 0.5.0.

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
| **1** | shipped on `main` at `79e0d57` | CI gate; documentation truth pass; fixed-point display; search options; GHW time axis; LSP port connections; library panic hardening |
| **2** | shipped in this wave | export signal values to CSV/TXT (MAT is *not* in this wave); large-dump loading UX (progress, cancel, 512 MB guard); LSP diagnostics surfaced in the editor + top bar; browser QA harness moved into the repository with an advisory CI smoke job |
| **3** | candidate, not committed | accessibility + keyboard + tier-A ruler; tree/list virtualization (R1 remainder); R2 step 3 and the R3 legacy deletion (see `rebuild-plan.md` and `state-store.md`); driver/load diagnostics; `generate`/arrayed-instance elaboration |
| **4** | candidate, not committed | yew 0.21 + tauri 2 migration; benchmarks (the `nightly-bench` gate currently enables nothing); multiple dumps / split view; signal bundles (AXI, Avalon, …) |

Statuses are honest: *shipped in this wave* means committed on `main` and waiting
for the next release; *candidate* means an agreed direction that is not started.
Items can move between waves when the tree says a wave should be smaller.

Wave 3's top items once Wave 2 is committed (in order): the sidebar list/tree
virtualization (the R1 remainder), the R2 step 3 + R3 legacy deletion (see
[`rebuild-plan.md`](rebuild-plan.md) and [`state-store.md`](state-store.md)),
driver/load diagnostics, and the accessibility/keyboard/tier-A-ruler pass.

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