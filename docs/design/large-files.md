# Memory, very large dumps and mmap

Engineers open waveform dumps that are routinely hundreds of megabytes and
sometimes several gigabytes. Memory is the first-order constraint of a wave
viewer: a tool that crashes on a large dump is not usable for the job it exists
for. This note records the current model, what Waves 12-17 change (the footprint work,
the bounded windowed renderer, the native mmap backend and renderer, and the
streaming value lookup), and the two honest paths to true memory-mapped access
(desktop native backend, browser worker with a lazy byte source).

Status: **current model documented; footprint work (Wave 12), bounded windowed
rendering (Wave 13), the native mmap backend foundation (Wave 14), the desktop
native renderer MVP (Wave 15), native parity (Wave 16) and the streaming
value-lookup root-cause fix (Wave 17) are in. Next: the remaining native parity
(export, split/multi-dump, lazy hierarchy), a batched values API, and the browser
worker/OPFS lazy byte source.**

## 1. Why it is hard in this architecture

The web build runs the whole engine (wellen parsing, hierarchy, decode, the
canvas renderer, the LSP) inside one wasm32 module on the browser main thread.
The desktop build embeds exactly the same wasm bundle in a Tauri webview.

- **No mmap in the browser.** There is no `mmap` for a file the page picked; the
  closest primitives are the File System Access API / OPFS plus synchronous
  access handles, and those are only available in a dedicated worker.
- **wellen needs random access to the source bytes.** The `Waveform` keeps the
  file bytes and re-reads the body for windowed queries (`stream_changes`) and
  for the lazily decoded signals. Dropping the bytes means either pre-decoding
  everything (much more memory) or losing the windowed decode path.
- **wasm32 has a hard address space.** The module can grow to 4 GiB; an
  allocation failure traps (the allocator aborts, `catch_unwind` cannot help
  because the release profile is `panic = "abort"`). A crash is therefore a
  blank viewer, not an error message - the refusal path matters as much as the
  footprint fixes.
- **copies add up.** The picked `File` is read in 8 MiB slices; the slices are
  copied into one growing `Vec<u8>` in wasm memory (up to ~2x the file size
  transiently while it reallocates), then wellen parses from it. On top of that
  come the hierarchy (path strings, one per signal), the packed change storage
  and the decoded cache (historically 64 MiB per open dump).

## 2. What Waves 12-13 do (footprint and bounded decode, not architecture)

1. **Measure, then fix**: a memory readout (`__waveDebug.memory = {
   wasmBytes, fileBytes, decodedBytes, budget }`) and a status chip; measured
   large generated dumps instead of guesses.
2. **One global decode budget** across open dumps (LRU across waveforms) instead
   of 64 MiB per dump; closing a dump or a split pane frees its decoded data
   immediately.
3. **Fewer copies**: reserve the file vector at its exact size (no reallocation
   copies), release the transient chunk buffers, and drop the source bytes of a
   dump as soon as the engine no longer needs them (format dependent: FST and
   VCD both stream from the bytes today, so this is documented rather than
   faked).
4. **String interning** in the hierarchy: shared `Arc<str>` names / a path arena
   with parent links instead of one full path allocation per signal.
5. **Pre-flight estimate + actionable refusal**: a file whose estimated
   footprint cannot fit the wasm budget is refused with a message that says what
   to do (use the desktop build once it has the native backend, convert the dump
   to FST, or split it) - never an allocator abort.
6. **Bounded windowed decode (Wave 13)**: the renderer no longer materialises a
   row's full history. It calls `window_changes_many` for a capped window (one
   stream pass for all visible rows, a fair per-row cap of roughly one change per
   pixel, early exit) and keeps the windows in a per-row cache inside the global
   64 MiB budget.
7. **Streaming value lookup (Wave 17)**: cursor readouts, inlay hints and the
   native backend ask `stream_value_before` for one change with O(1) retained
   memory instead of materialising the random-access body; the FST summary reads
   its time range from the header block.

These make today's large-but-finite dumps work and turn the cliff into a
message. They do not remove the fundamental limit: the browser build still keeps
the whole file in wasm memory - but a drawn row costs a window and a readout
costs one change, not a full history.

### 2b. Why windowing was the real fix

The Wave 12 work reduced the footprint of a load; it did not stop the renderer
from materialising the complete change history of every visible row. Dense
*visible* rows, not file size, were the trigger: a real 97.5 MiB / 3,025-signal /
58M-change FST loaded and scrolled inside the budget, while one visible
10M-change row decoded 229 MB and grew the wasm by ~0.5 GB per row (a handful of
those is the user's 3+ GB crash). With `window_changes_many` a drawn row decodes
only the changes in the visible time window plus a small `before` prefix, so the
cost is bounded by the pixels on screen. The `window_many` benchmark shows the
difference: 2.33 ms for eight rows over the full trace, against 99.6 ms for the
full-history decode of the same rows (which also materialises them).

### 2c. The root cause of the value-readout peak (Wave 17)

The windowed renderer fixed drawing, but two other paths still went through the
random-access body: `Waveform::time_range_hint()` (called to fill a dump
summary) and `Waveform::value_at()` (called for cursor readouts and editor inlay
hints). For FST both silently materialised the body just to answer: an
8-bytes-per-time-step time table plus wellen's packed decodes. A 20M-time-step
FST allocated 160 MB of time table and 100 MB of packed data and peaked at
~623 MB merely to show its summary; a real 100+ MB FST with many steps could
reach multiple gigabytes, and the native backend's `value_at` path had the same
shape.

The fix is in the library:

- **`time_range_hint` for FST reads start/end from the header block**, so the
  body counters (time table, packed decodes) stay 0 after open. Only gzip-wrapped
  or incomplete files fall back to reading the body.
- **`stream_value_before(signal, time)`** returns the last change at or before
  `time` with O(1) retained memory: FST probes exponentially growing backward
  windows through `stream_changes` (capped probe list; a sparse signal degrades
  to one prefix scan), VCD does one forward scan (the format cannot seek), and
  GHW keeps an honest materialised fallback (wellen cannot stream GHW). Answers
  are memoised in a 64-entry LRU keyed by `(signal, time)`; the batch form
  `values_before` shares one stream pass for a screen full of rows. The LRU is
  visible as `lookup_cache_entries`/`lookup_cache_bytes` in `MemoryStats`.
- **`MemoryStats`** gained `time_table_bytes`, `packed_bytes`,
  `decoded_struct_bytes`, `decoded_payload_bytes` and the lookup-cache counters,
  so "did this action materialise the body?" is visible instead of inferred.
- **Native diagnostics:** `dump_memory_stats(id)` returns that accounting for
  the open native dump and `process_memory()` returns
  `{ current_bytes, peak_bytes }` from the OS (GetProcessMemoryInfo on Windows,
  `/proc/self/status` VmHWM on Linux); the front end publishes both under
  `__waveDebug.memory` (`memory.native` in the desktop build), and the native
  backend now uses `stream_value_before` with a per-signal last-answer cache
  instead of `value_at`. Editor inlay hints use the streaming lookup too, in a
  bounded per-pass loop.

Measured (release-relevant, from the task reports): on a 20M-time-step fixture
the retained heap after one lookup dropped from 260,019,306 B to
15,876-19,235 B and the process peak from 623 MB to 196 MB (including the
120 MB mapped file); on the 1M-step `large.fst` `value_at` went from 812 ms to
241-345 ms, the peak from 36.5 MB to 16.4-18 MB, retained after open from
13.6 MB to 0, and the open summary no longer allocates the 8 MB time table.

Residual costs, recorded so nobody promises more than this: a sparse FST lookup
can still take hundreds of milliseconds (the probe list is capped, so the worst
case is one full prefix scan); a VCD lookup is O(prefix); the GHW lookup is not
memory-flat; and each FST streaming call allocates a small transient buffer. The
body is still materialised on demand by the queries that genuinely need it
(export, the eager value path, GHW).

## 3. Desktop: native backend with a memory map

The desktop build can have both halves: the Rust engine runs natively (it
already compiles for the host; the LSP backend is native today) and only the
rendered data crosses into the webview.

```
FE (wasm)                     tauri backend (native)
canvas  <-- windows  <-- IPC commands
                                  Waveform::open_path_mmap(path)
                                  memmap2::Mmap  (read-only, no copy)
                                  hierarchy/header parsed natively
```

Key points:

- `memmap2` maps the file read-only; the OS page cache decides what is resident,
  so a 4 GB FST is fine on a machine with 8 GB of RAM.
- The backend answers the queries the renderer needs: open (summary), hierarchy
  pages, decoded changes in a time window (bounded by pixels, not by file size),
  value at time, and the design/LSP work can stay wasm or move too.
- The existing `Waveform` API is the contract; the `ByteSource` trait (Wave 12
  groundwork) lets the same code read from `Vec<u8>` (browser) or `Mmap`
  (desktop) without duplication.
- This is a real architectural step. The IPC protocol, the registry and the
  mmap source shipped in Wave 14; a remote dump handle in the store plus
  error/timeout/timeout handling is the renderer work that follows. The web build
  keeps the in-memory path and stays the low-install option.

Implementation result (Wave 14), the foundation:

- **Library: host-only mmap source.** `Waveform::open_path_mmap` opens a file
  through `WaveformSource::Mmap` (a `memmap2::Mmap` behind an `Arc`); readers
  serve pages straight from the mapping, so no file bytes are retained as heap
  memory. `Waveform::mapped_bytes` and
  `MemoryStats::source_mapped_bytes` report the mapping length (the OS page
  cache's resident cost, not the process heap). The mmap module is
  `#[cfg(not(target_arch = "wasm32"))]` - the wasm build does not compile it and
  keeps the in-memory `Arc<[u8]>` source. On the real 97.5 MiB / 3,025-signal /
  58M-change FST: `open_path_mmap` 9.27 ms, the existing file reader 10.18 ms,
  reading the file into memory 30.31 ms, with **0 heap bytes retained** by the
  mmap source. Tests cover byte-for-byte answer equivalence with the file and
  memory sources, `Corrupt` errors through the mapping, and releasing the
  mapping when the waveform drops; the `mmap` benchmark group keeps the numbers
  honest.
- **Desktop: native dump registry + IPC queries.** `src-tauri/src/dumps.rs`
  keeps a registry of natively opened dumps (monotonic ids, 32-dump cap) and
  answers the dump/query commands over Tauri IPC (plus the dialog-parts variant
  of the open and two diagnostics commands):
  `open_dump(path) -> DumpSummary` (absolute path - drag-and-drop and launch
  arguments; format/time range/signal+scope counts, retained/mapped bytes, open
  time), `open_dump_parts(parts) -> DumpSummary` (the in-app dialog's navigation
  parts, resolved against the dialog's base directory with the same rules as the
  other dialog commands - `open_dump` resolves against the process cwd, so the
  parts variant is what makes a dialog-opened large dump stay native),
  `dump_hierarchy(id, offset, limit) ->
  HierarchyPage` (pre-order flattened scope/signal rows, pages of at most 4096),
  `dump_window(id, signals, start, end, maxChangesPerSignal?, show?) ->
  [WindowDto]` (values already rendered to text, per-signal cap, `before`
  context), `dump_value_at(id, signal, time, show?) -> String | null`,
  `dump_find(id, signal, query, from, backwards, show) -> { time, value } |
  null` (a streaming value search: one pass with early exit, rendered-text
  matching, `0x`/`0b`/`0o` radix prefixes, case-insensitive, x/z as text; a
  backwards search keeps the last match and the worst case is a full trace scan,
  not cancellable) and `close_dump(id)`. Two diagnostics commands expose the
  native accounting: `dump_memory_stats(id)` (the library `MemoryStats` for the
  open dump, including the lookup cache) and `process_memory()`
  (`{ current_bytes, peak_bytes }` from the OS). `dump_value_at` is backed by the
  streaming lookup plus a per-signal last-answer cache (Wave 17), so a cursor
  readout no longer materialises the body. The file bytes never cross IPC; the
  webview receives only rendered windows. `close_dump` drops the mapping, and `open_dump` falls back
  to the read path when a map cannot be created. Measured: a 4096-row hierarchy
  page 1.82 ms; 10 dense signals over the full trace 139 ms (debug build); first
  `dump_value_at` 690 ms, warm 175 ms; `dump_find` on the real 97.5 MB FST
  (debug) 98.8 ms forward near the start and 198.1 ms near the end, 14/46/97.5 ms
  on the dense FST. Native tests cover the registry, the query round-trip, the
  find semantics (directions and radix queries) and the dialog-parts resolver. On Windows the mapping keeps the file handle alive for the
  life of the open dump, so deleting/renaming/replacing it while open can fail
  until `close_dump`.
- **CI: the native backend is tested.** A new `test-app` job runs the backend
  tests on `ubuntu-24.04` with the GTK/webkit prerequisites and the
  `front-end/dist` placeholder (the host test job excludes `app` because of
  those prerequisites).
- **Renderer MVP (Wave 15): the desktop large-file mode.** The front-end half
  lives in `front-end/src/remote/`: a `RemoteSession` keeps the dump summary and
  turns the paged hierarchy into the same `SidebarTree` the sidebar always draws,
  plus a path -> signal index so rows resolve without the wasm waveform. In the
  desktop build a dump **larger than 64 MiB** (`NATIVE_THRESHOLD_BYTES`) opens
  through the native backend - drag-and-drop passes the absolute path to
  `open_dump`, the in-app dialog its navigation parts to `open_dump_parts` - and
  is drawn read-only from bounded `dump_window` queries over IPC; smaller files
  and every native error fall back to the wasm path, so normal dumps behave
  exactly as before. Parity features landed in Wave 16: the ruler find box
  (Enter/Shift+Enter) searches the native dump through `dump_find`, moves cursor
  1 to the match and reports `found <query> in <row> at t=<time> (<value>)` (or a
  no-match status); the pane fetches `dump_window` once per on-screen display
  format (hex/uint/sint/oct/bin/ascii), so row format settings change the drawn
  values and the value column; and a cursor outside the fetched window gets its
  value from batched, cached `dump_value_at` calls (at most 64 requests per
  redraw, a 512-entry (row, time) cache) instead of a per-row-per-frame call. Wave 17 made the readouts memory-flat: the
  backend's cursor values and the editor inlay hints use `stream_value_before`
  (the backend keeps a per-signal last-answer cache), and the debug hook publishes
  the library accounting and the process working set under
  `__waveDebug.memory` (`memory.native` in the desktop build). Native
  open/hierarchy errors for possibly-large files no longer fall back to the wasm
  byte path (that copy would reproduce the peak the mode avoids); small dumps
  still fall back. The mode is visible: a
  `.dock-tab.native-badge` chip ("native (read-only, mmap)", with a
  path/size/signals/scopes/mapped tooltip) and the status line
  `waveform loaded (native read-only, mmap) — <FMT> <N> MB, range <start>..<end>
  in <ms> ms, mapped <M> MB, <signals> signals in <scopes> scopes`. The sidebar,
  rows and canvas reuse the existing components/classes, and single add and bulk
  add work against the native hierarchy. Remaining native limits, recorded
  deliberately: one native dump at a time (adopting another closes the previous),
  no split or multi-dump tabs in native mode, the hierarchy is fetched eagerly
  (paged in 4096-row requests, capped at 400,000 rows), and export is refused
  with an actionable message pointing at the wasm path (files up to 64 MB).
- **Next: the rest of native parity and the browser path.** Export, split and
  multi-dump support, and lazy hierarchy loading instead of the eager paged fetch
  are the remaining desktop gaps. The browser still has no mmap, so its analogue
  is the worker + OPFS lazy byte source described in section 4.
- **Runtime status.** The desktop path is compile-verified in this repository
  (there is no desktop GUI here): real IPC round trips, drag-and-drop of a large
  file, the in-app dialog's part resolution against its base directory (host
  tested for `open_dump_parts`, but the full GUI path is unverified), the mmap
  open/lock/close lifecycle, `dump_window`/`dump_find` latency and zoom smoothness
  on huge dumps must be validated on a desktop machine (the release checklist for
  the desktop build); the find timings above are debug-build measurements.

## 4. Browser: worker + lazy byte source (mmap-like)

If large web sessions become a requirement, the analogue of mmap is:

1. copy the picked file into OPFS (streaming, so memory stays flat),
2. run the waveform engine (same wasm) in a dedicated worker that opens the
   OPFS file with a synchronous access handle,
3. answer main-thread queries with ranges: the worker reads only the byte
   windows a query touches, and keeps an LRU of decoded results,
4. the main thread keeps only the rendered slices.

Trade-offs: OPFS quota is a fraction of disk space, the first copy costs the
file size in `navigator.storage`, workers add a message protocol and latency to
every query, and the wasm module is instantiated twice (or moved entirely into
the worker). It removes the 4 GiB heap constraint for the raw bytes but not the
latency/duplication complexity, which is why the desktop native backend comes
first.

## 5. Practical guidance (also in the readme)

- **FST is the memory-friendlier format** (compressed, compact indices); prefer
  it over VCD for very large dumps.
- The browser budget is about the *sum* of open dumps; keep one or two dumps
  open in the browser and use the desktop build for multi-GB sessions.
- Dense rows are bounded by the window now, but a very wide window or many
  simultaneously visible dense rows still consume the soft 64 MiB decode budget;
  the memory readout in the status bar/debug hook is the ground truth: if a
  planned session exceeds it, convert or switch build, do not gamble.

## 6. Verification

- Generated large dumps (30/60/120 MB VCD) loaded through the real app in the QA
  matrix: load-or-actionable-refusal, the memory readout within budget, memory
  released on close, split counts both dumps.
- Library unit tests/benches for the accounting and interning, plus the
  `window_many` benchmark comparing the capped path with the full-history decode;
  the byte-source abstraction keeps the browser path byte-identical.
- The `denseDump` QA scenario: dense VCD proxy plus an opt-in real FST
  (`QA_LARGE_FST=<path>`), asserting that a visible dense row decodes only a
  bounded window, that zoom cycles do not grow the decoded history/wasm and that
  a second dense row does not double the history.
- The native renderer is compile-verified and host-tested where possible
  (`front-end/src/remote/` and the wave-pane window path); the runtime checks are
  listed in the section 3 "Runtime status" note.
- The streaming-lookup tests (`wave-insight-lib/tests/waveform_stream_value.rs`):
  FST answers match `value_at` at the same positions, the VCD forward scan and
  the GHW fallback answer correctly, the LRU accounting moves, and
  `time_range_hint` leaves the body counters at 0; the 20M-step fixture
  reproduces the retained-heap and peak numbers above.
- The native process-memory command has a host test (a positive working set and
  no invented peak), and the native backend's `dump_memory_stats` path is covered
  by the registry tests.
- The native backend tests: mmap/file/memory answer equivalence, corrupt input
  through the mapping, mapping release on drop (library) and the Tauri registry
  round-trip/arguments/ids/cap/close (src-tauri), run by the new `test-app` CI job
  on ubuntu-24.04 with the GTK prerequisites.