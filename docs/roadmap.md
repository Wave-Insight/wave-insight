# Roadmap

A living plan, updated at each wave boundary. It says what is being worked on and
in what order — not when it will ship. Design background lives in
[`rebuild-plan.md`](rebuild-plan.md) (R1–R3) and
[`state-store.md`](state-store.md) (the R2 store); this file links to them and
does not duplicate them.

## Where we are

The last tagged release is **0.21.0** (2026-11-11): one-canvas waveform
renderer, R2 state store, Verdi-style navigation, CI gate, fixed-point display,
search options, CSV/TXT/MAT export, chunked large-dump loading, LSP diagnostics
and completion, the browser QA suite in `qa/`, generate/arrayed elaboration,
sidebar virtualization, persistence, the accessibility/keyboard pass, the Verdi
find-value flow, indexed LSP/design queries, the R3 legacy deletion, the Tauri 2
desktop shell, the waveform hardening, multi-dump tabs, the Yew 0.21 front end,
the split view (polished), inspector bit ranges, bulk add, the memory-first
large-dump work, the bounded windowed renderer, the desktop native backend
foundation, the native renderer MVP and native-mode parity. **Wave 17** was the
memory root-cause fix - FST `time_range_hint` reads the header instead of
materialising the random-access body, and cursor readouts/inlay hints use the
streaming `stream_value_before` lookup (O(1) retained memory: FST capped
backward probes, VCD forward scan, documented GHW fallback; 64-entry LRU), wired
through the native backend, plus `dump_memory_stats`/`process_memory` and the
library/native/process counters in `__waveDebug.memory`. **Wave 18** was
the first hands-on-feedback pass from the user's test of 0.20.0: Verdi-style
wheel panning, cursor value columns flush against the waveform, bare digits in
the wave view (the data views keep the Verilog radix), `…`-truncated band labels,
crisp 1-bit edge/cursor alignment and in-place reload of a re-simulated dump.
**Wave 19** is on `main` unreleased: the signal list selects like the module
tree with an explicit batch `Add selected`, the Verdi block add (select code,
`Ctrl+W`), the desktop reload reopening the dumps that were open, and the
SpinalHDL companion-RTL auto-load. The remaining native gaps are export,
split/multi-dump, the eager hierarchy and a batched values API. The
`Unreleased` section of [`CHANGELOG.md`](../CHANGELOG.md) is the authoritative
list of what is on `main` beyond 0.21.0.

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
| **16** | released in 0.19.0 | native-mode parity: `dump_find` streaming value search (0x/0b/0o, case-insensitive, x/z as text; 98.8 ms forward near the start / 198.1 ms near the end of the 97.5 MB FST debug; no match = full scan, not cancellable), native find box wiring (jump + `found …`/`no match` status), per-display-format `dump_window` groups so row formats change drawn values and the value column, and batched/cached `dump_value_at` cursor readout outside the window (<=64 requests per batch, 512-entry cache) |
| **17** | shipped in this wave | memory root-cause fix: FST `time_range_hint` reads the time range from the header block (body counters stay 0), `stream_value_before` (and batch `values_before`) answers cursor/inlay/native lookups with O(1) retained memory (FST capped backward probes, VCD forward scan, GHW documented fallback; 64-entry LRU reported in `MemoryStats`), `dump_memory_stats` + `process_memory` and the library/native/process counters in `__waveDebug.memory`; measured 20M-step retained 260,019,306 B -> 15,876-19,235 B, peak 623 -> 196 MB (incl. 120 MB mapped), large.fst 1M-step value_at 812 -> 241-345 ms, peak 36.5 -> 16.4-18 MB; native backend streams lookups with a per-signal cache and no longer silently falls back for possibly-large files |
| **18** | released in 0.21.0 | the user's hands-on feedback on 0.20.0: Verdi wheel pan (plain wheel pans the time axis, Ctrl+wheel zooms at the pointer, Shift+wheel scrolls rows; split pane follows), cursor value columns that end the label cell flush against the waveform (one width per render from the visible rows, no reserved slot before a cursor, action icons moved before the values), bare digits in the wave view (the inlay/export/find data forms keep the Verilog radix), `…`-truncated band labels instead of dropped ones, crisp-column alignment for a cursor on a 1-bit edge, in-place reload of a re-simulated dump with the same name/byte size (FNV-1a content fingerprint), and QA regression scenarios for all of it |
| **19** | on `main`, unreleased | the user's second hands-on list: the SIGNALS list selects like the module tree (plain/Ctrl/Shift + explicit `Add selected (N)`, one batch), the Verdi source-block add (`Ctrl+W` / `Ctrl+Shift+W` / editor context menu `Add signals in selection to the wave`), desktop (Tauri) reload reopens the dumps that were open from remembered absolute paths, and SpinalHDL companion RTL (`simWorkspace/<design>/rtl`) auto-loads next to an opened dump; QA 63 scenarios / 610 assertions |
| **20** | chosen for the next wave, not started | remaining native parity: export, split and multi-dump on the native path, lazy hierarchy loading and a batched values API for native/inlay readouts; then the browser worker + OPFS lazy byte source (the browser's mmap analogue) and completion packages as qualifiers |
| **21** | later candidates | signal bundles (AXI, Avalon, …); SpinalHDL support; inline python over signal values |

Statuses are honest: *released* means a tagged release, *shipped in this wave*
means committed on `main` and waiting for the next release, *chosen* means the
next wave's agreed top items (not started), and *later candidates* are unordered.
Items can move between waves when the tree says a wave should be smaller.

Wave 20 completes the desktop native mode (export, split/multi-dump, lazy
hierarchy, batched values) and then takes the browser worker/OPFS lazy byte
source plus completion packages. The design note
[`docs/design/large-files.md`](design/large-files.md) records the root cause, the
streaming lookup, the measured numbers and the remaining limits; the multi-dump
note covers the split-view remainder. Wave 18's user-visible changes are listed
under [`CHANGELOG.md`](../CHANGELOG.md) 0.21.0.

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