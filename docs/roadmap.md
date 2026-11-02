# Roadmap

A living plan, updated at each wave boundary. It says what is being worked on and
in what order — not when it will ship. Design background lives in
[`rebuild-plan.md`](rebuild-plan.md) (R1–R3) and
[`state-store.md`](state-store.md) (the R2 store); this file links to them and
does not duplicate them.

## Where we are

The last tagged release is **0.19.0** (2026-11-03): one-canvas waveform
renderer, R2 state store, Verdi-style navigation, CI gate, fixed-point display,
search options, CSV/TXT/MAT export, chunked large-dump loading, LSP diagnostics
and completion, the browser QA suite in `qa/`, generate/arrayed elaboration,
sidebar virtualization, persistence, the accessibility/keyboard pass, the Verdi
find-value flow, indexed LSP/design queries, the R3 legacy deletion, the Tauri 2
desktop shell, the waveform hardening, multi-dump tabs, the Yew 0.21 front end,
the split view (polished), inspector bit ranges, bulk add, the memory-first
large-dump work, the bounded windowed renderer, the desktop native backend
foundation and the desktop native renderer MVP. **Wave 16** lands in this wave:
native-mode parity - the ruler find box streams `dump_find` over IPC (measured
98.8 ms forward near the start and 198.1 ms near the end of the real 97.5 MB FST,
debug), rows fetch `dump_window` per display-format group so row formats change
the drawn values and value column, and cursor values outside the fetched window
come from batched/cached `dump_value_at` calls. The remaining native gaps are
export, split/multi-dump and the eager hierarchy. The `Unreleased` section of
[`CHANGELOG.md`](../CHANGELOG.md) is the authoritative list of what is on
`main` beyond 0.19.0.

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
| **12** | released in 0.16.0 | bulk add from the module tree; memory-first large dumps (global 64 MiB decode budget + free plan, fallible read buffer, pre-flight estimate/refusals, `__waveDebug.memory`, release on close, load peaks -31..-44%); library interning/lazy paths/`memory_stats`/`ByteSource`; structural VCD validation fixing the conflicting-width crash; `docs/design/large-files.md`; QA 57 scenarios / 503 assertions |
| **13** | released in 0.17.0 | CI green again (clippy placeholder, root-relative `dist-qa` bundle + Node 22 for QA, FST internal-count validation); bounded windowed rendering (`window_changes_many`/`WindowedSignal`, the 3+ GB dense-row crash fixed); desktop raw IPC; bulk-add discoverability; `denseDump`/`bulkAdd` QA -> 58 scenarios / 524 assertions |
| **14** | released in 0.18.0 | native mmap backend foundation: host-only `memmap2` byte source (`Waveform::open_path_mmap`/`WaveformSource::Mmap`, `mapped_bytes`, `MemoryStats::source_mapped_bytes`, 0 retained heap bytes; 9.27 ms vs 10.18/30.31 ms on the 97.5 MiB FST; equivalence/corrupt/drop tests + `mmap` bench group) and the Tauri native dump registry + IPC queries (`open_dump`/`dump_hierarchy`/`dump_window`/`dump_value_at`/`close_dump`; monotonic ids, 32-dump cap, files never cross IPC; hierarchy page 4096 rows 1.82 ms, 10 dense signals 139 ms debug, value_at 690/175 ms; native tests) plus the `test-app` CI job |
| **15** | released in 0.18.0 | desktop native renderer MVP: dumps over 64 MiB open through the native backend and are drawn read-only from bounded `dump_window` IPC queries (wasm fallback for smaller files and native errors); badge/status line, component reuse, single and bulk add, eager paged hierarchy (400k cap), one native dump, no split/multi-dump, export refused; 0.18.1 fixed the in-app dialog path so it reaches native mode too |
| **16** | shipped in this wave | native-mode parity: `dump_find` streaming value search (0x/0b/0o, case-insensitive, x/z as text; 98.8 ms forward near the start / 198.1 ms near the end of the 97.5 MB FST debug; no match = full scan, not cancellable), native find box wiring (jump + `found …`/`no match` status), per-display-format `dump_window` groups so row formats change drawn values and the value column, and batched/cached `dump_value_at` cursor readout outside the window (<=64 requests per batch, 512-entry cache) |
| **17** | chosen for the next wave, not started | the rest of native parity: export, split and multi-dump on the native path, lazy hierarchy loading; then the browser worker + OPFS lazy byte source (the browser's mmap analogue) and completion packages as qualifiers |
| **18** | later candidates | signal bundles (AXI, Avalon, …); SpinalHDL support; inline python over signal values |

Statuses are honest: *released* means a tagged release, *shipped in this wave*
means committed on `main` and waiting for the next release, *chosen* means the
next wave's agreed top items (not started), and *later candidates* are unordered.
Items can move between waves when the tree says a wave should be smaller.

Wave 17 completes the desktop native mode (export, split/multi-dump, lazy
hierarchy) and then takes the browser worker/OPFS lazy byte source plus
completion packages. The design note
[`docs/design/large-files.md`](design/large-files.md) records the native protocol,
the measured timings and the remaining limits; the multi-dump note covers the
split-view remainder.

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