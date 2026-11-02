# Roadmap

A living plan, updated at each wave boundary. It says what is being worked on and
in what order — not when it will ship. Design background lives in
[`rebuild-plan.md`](rebuild-plan.md) (R1–R3) and
[`state-store.md`](state-store.md) (the R2 store); this file links to them and
does not duplicate them.

## Where we are

The last tagged release is **0.16.0** (2026-11-02): one-canvas waveform
renderer, R2 state store, Verdi-style navigation, CI gate, fixed-point display,
search options, CSV/TXT/MAT export, chunked large-dump loading, LSP diagnostics
and completion, the browser QA suite in `qa/`, generate/arrayed elaboration,
sidebar virtualization, persistence, the accessibility/keyboard pass, the Verdi
find-value flow, indexed LSP/design queries, the R3 legacy deletion, the Tauri 2
desktop shell, the waveform hardening, multi-dump tabs, the Yew 0.21 front end,
the split view (polished), inspector bit ranges, bulk add from the module tree and
the memory-first large-dump work (global decode budget, pre-flight refusals,
`__waveDebug.memory`, interning and the `ByteSource` groundwork). **Wave 13** is
landing in this wave on `main` (commit follows 0.16.0): the CI green fix (commit
5fa59d1: clippy placeholder on a clean checkout, root-relative `dist-qa` bundle
and Node 22 for the QA job, hardened FST validation), bounded windowed rendering
(the real fix for the 3+ GB dense-row crash), the desktop raw-IPC fix, bulk-add
discoverability (row menu, Select all, sticky bulk bar, over-cap state) and the
`denseDump`/`bulkAdd` QA scenarios; the matrix grows to 58 scenarios / 524
assertions. The `Unreleased` section of [`CHANGELOG.md`](../CHANGELOG.md) is the
authoritative list of what is on `main` beyond 0.16.0.

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
| **3** | released in 0.7.0 | constant-bound generate unrolling + arrayed instances; driver/load diagnostics; sidebar virtualization; layout + per-signal display persistence; accessibility/keyboard/tier-A-ruler pass; tauri file browser home directory; data-URI favicon |
| **4** | released in 0.8.0 | elaboration diagnostics merged into the editor's LSP diagnostics; criterion benchmarks + advisory bench workflow; `deny.toml` + `cargo deny` audit workflow + dependabot policy + tokio security bump; per-dump session restore; MAT export |
| **5** | released in 0.9.0 | R2 step 3 (sources/focus/goto/jump_token slices + real LSP/editor `Effect`s); fast VCD open; Verdi find-value; semver-compatible dependency refresh + CI deploy job split out |
| **6** | released in 0.10.0 | R3 legacy data-model deletion; R2 step 3b partial; indexed design/LSP span structure + `bench lsp`; three QA scenarios -> 49 scenarios / 356 assertions; multi-dump/split-view design note |
| **7** | released in 0.11.0 | Tauri 2 migration; R2 step 3b finished; waveform hardening (structural FST validator, hierarchy depth bound, aliased-signal streaming, deterministic fuzz tests) |
| **8** | released in 0.12.0 | multi-dump steps 1-2 (per-tab store model, dump tab strip, dedupe/close semantics, per-tab session restore, 1.5 GiB budget); driver/load bit ranges; browser corruption coverage |
| **9** | released in 0.13.0 | Yew 0.21 migration; language-server completion (server side); browser `bitRanges` QA scenario -> 52 scenarios / 414 assertions |
| **10** | released in 0.14.0 | split-view MVP; Monaco completion provider + member completion; inspector bit ranges; per-run QA staging; weekly full-matrix `qa.yml` -> 55 scenarios / 446 assertions |
| **11** | released in 0.15.0 | split-view polish (per-dump unlinked scroll/zoom/pan, linked freeze, resize observer, draggable persisted ratio); completion v3; completion benchmarks + refreshed baseline; three verifier fixes; QA 55 scenarios / 458 assertions |
| **12** | released in 0.16.0 | bulk add from the module tree (multi-select, Add selected, one `RowsAdded` batch, cap); memory-first large dumps (global 64 MiB decode budget + free plan, fallible read buffer, pre-flight estimate/refusals, `__waveDebug.memory`, release on close, load peaks -31..-44%); library interning/lazy paths/`memory_stats`/`ByteSource`; structural VCD validation fixing the conflicting-width crash; `docs/design/large-files.md`; QA 57 scenarios / 503 assertions |
| **13** | shipped in this wave | CI green again (clippy `front-end/dist` placeholder on a clean checkout, root-relative `dist-qa` bundle + Node 22 for the QA job, FST internal-count validation so a corrupt alias chain is a `Corrupt` error instead of a 5e16-byte allocation); bounded windowed rendering (`window_changes_many`/`WindowedSignal`, per-row window cache in the 64 MiB budget, the 3+ GB dense-row crash fixed; `window_many` 2.33 ms vs 99.6 ms full history for 8 rows); desktop raw IPC (`tauri::ipc::Response` -> one ArrayBuffer copy); bulk-add discoverability (row menu, Select all, sticky bulk bar, over-cap state); `denseDump`/`bulkAdd` QA -> 58 scenarios / 524 assertions |
| **14** | chosen for the next wave, not started | desktop native backend with memory-mapped files (the `ByteSource`/memmap2 plan in [`docs/design/large-files.md`](design/large-files.md)); browser worker + OPFS lazy byte source; completion packages as qualifiers; the remaining split-view limits (unlinked cursors, row scrollbar, window-captured pan, persisted split) |
| **15** | later candidates | signal bundles (AXI, Avalon, …); SpinalHDL support; inline python over signal values |

Statuses are honest: *released* means a tagged release, *shipped in this wave*
means committed on `main` and waiting for the next release, *chosen* means the
next wave's agreed top items (not started), and *later candidates* are unordered.
Items can move between waves when the tree says a wave should be smaller.

Wave 14's top items are the two large-file architectures (desktop native
memory-mapped backend first, then the browser worker/OPFS lazy byte source), then
the completion and split-view leftovers. The design note
[`docs/design/large-files.md`](design/large-files.md) records the current model and
both plans; the multi-dump note covers the split-view remainder.

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