# FSDB support: converter, cache and provenance

Status: the CLI slice is on `main` (unreleased). The vendor-SDK backend is the
next slice. This note records the decision, the seam and what is deliberately
not here.

## The problem

Synopsys Verdi's `.fsdb` is proprietary: there is no public specification and no
independent parser. wellen (the reader behind [`waveform`](../../wave-insight-lib/src/waveform))
implements VCD, FST and GHW and has no FSDB support, so Wave Insight could not
open a Verdi trace at all.

## What the open-source viewers do

| project | FSDB route |
|---|---|
| GTKWave (including the LTS line) | optional `--enable-fsdb` build links Synopsys's free FSDB Reader SDK; a default build cannot read FSDB |
| Vaporview (VS Code) | optional addon built against the same SDK; the extension itself ships no reader |
| `ondas` / `wavepeek` | SDK |
| waveform MCP servers (e.g. `wave-mcp`) | either the SDK or Verdi's `fsdb2vcd` CLI |

The pattern is unambiguous: nobody parses FSDB from the file format alone. The
choices are "link the vendor reader" or "ask the vendor CLI to convert".

## Decision

This slice reads FSDB by **converting it to VCD behind a cache**:

1. the user's `.fsdb` is converted with `fsdb2vcd <in.fsdb> -o <cache>.vcd`
   (Verdi's CLI reads a trace without a licence);
2. the VCD is cached under the system temporary directory;
3. the existing wellen pipeline opens the cached VCD, with the original path,
   format and conversion cost recorded as the waveform's [`Origin`].

Why this and not a direct reader now:

- **No vendor linkage, no rebuild.** The default build gains no header, no
  library, no build flag and nothing in the wasm dependency graph. A user with
  Verdi installed gets FSDB support by putting one tool on `PATH`.
- **One reader for all formats.** Values, time ranges, windowed decoding,
  streaming lookups and the native IPC backend are the wellen paths, already
  tested and measured. A second native reader would duplicate all of it.
- **The cost is paid once per file version.** A conversion is seconds; the
  cached VCD then behaves like any other VCD. The cache key includes the
  canonical path, the file length, the modification time and the converter, so a
  re-simulated dump re-converts instead of showing stale values.
- **The seam is the right shape for the SDK.** Synopsys's reader produces the
  same thing (a reader over the trace), so the next slice implements the same
  `Converter` trait and keeps the cache and provenance.

A **direct native reader** (no VCD intermediate) is the long-term option, but it
requires abstracting `Waveform` from wellen: the type currently exposes wellen's
`Signal`/`SignalSource`/`Hierarchy` in its body cache, and every query (streamed
windows, time ranges, packed decodes) is written against them. That is a
separate, larger refactor and is not hidden inside this slice.

## The seam

[`wave-insight-lib/src/waveform/fsdb/`](../../wave-insight-lib/src/waveform/fsdb):

- `Converter { name, convert(input, output) }` - the only thing a backend has to
  implement.
- `default_converter()` - the test seam (unit tests only), then the
  `#[cfg(feature = "fsdb")]` SDK hook, then the CLI tool discovered from
  `WAVE_INSIGHT_FSDB2VCD` or `PATH`.
- `prepare(path)` / `prepare_with(path, converter)` - cache lookup, lock,
  conversion, validation (`non-empty file`), rename.
- `tool.rs` - the spawn, the 2 KiB stderr tail, missing program vs failed run.
- `cache.rs` - key, directory, `.lock`, `.part`, stale-lock recovery.

`Origin { format, path, converted, converter, convert_ms }` is attached to the
`Waveform`. The logical format (`file_format()`) is `Fsdb`, the parsed format
(`container_format()`) is `Vcd`; time-range and streaming branches use the
container, while `path()`, tabs, companion-RTL discovery and the MCP identity use
the origin path. `source()` stays the data source (the cached VCD) and says so.

A missing or failed conversion is never presented as a read waveform:
`FsdbUnavailable` carries the actionable "put fsdb2vcd on PATH / set
`WAVE_INSIGHT_FSDB2VCD` / build with `--features fsdb`" text, `FsdbConversion`
names the file and the tool's stderr tail.

## Cache design

- Directory: `std::env::temp_dir()/wave-insight-fsdb-cache`, overridable with
  `WAVE_INSIGHT_FSDB_CACHE`. It is a **temporary artifact**: deleting it only
  costs the next conversion.
- Name: `<sanitized stem>-<hash>.vcd`; the hash is a `DefaultHasher` over
  (canonical input path, length, mtime in nanoseconds, converter name).
- Publication: write `<entry>.<pid>.part`, validate it, `fs::rename` it into
  place. A killed conversion leaves a `.part`, never a half-written entry.
- Concurrency: `<entry>.lock` created with `create_new`. A second open polls for
  the entry (100 ms, up to 120 s) instead of converting again. A lock older than
  10 minutes is stale (the owner is gone) and is removed; a live lock is never
  deleted by another process.

## Provenance and user-visible behaviour

- The dump summary reports `format: "FSDB"`, the **original** size, the mapped
  cached VCD size, and `converter`/`convertedBytes`/`convertMs` when converted.
- Native status line: `waveform loaded — FSDB 105 MB converted to VCD in 8.2 s
  (fsdb2vcd), opened in 412 ms, mapped 98 MB, ...`; a cache hit says
  `read from its cached VCD` instead of inventing a duration.
- Pickers and the desktop dialog accept `.fsdb`; the remote/front-end decision
  forces the native path for `.fsdb` at any size.
- The pure browser build refuses `.fsdb` before reading a byte, with the
  desktop/`--features fsdb` advice: copying the bytes into wasm could not open
  them.

## Licensing boundary

- No Synopsys header, library, documentation excerpt or generated binding is in
  the repository, and none is added to the wasm graph.
- No GPL code: GTKWave's FSDB support (and its reader) is GPL; this slice
  invokes a program installed by the user, it does not link or copy it.
- `fsdb2vcd` is executed as an external process. The project ships no vendor
  payload; a user without Verdi gets the actionable error, not a partial reader.

## Verification gap

There is no Verdi installation on the development/CI machine, so there is no
real `.fsdb` fixture (and none may be committed). The pipeline is covered with
an injected converter that writes a small VCD: the FSDB entry point, the
cache/provenance, hierarchy and values, time range, cache reuse and
re-conversion, stale locks and partial files. The tool path is covered for
"program missing", "non-zero exit with a stderr tail" and "wrote nothing"; a real
conversion is unverified here and must be checked on a machine with Verdi
(`cargo test` plus one manual open of a real trace).

## Follow-up

1. `fsdb/sdk.rs` behind `--features fsdb`: implement `Converter` on the FSDB
   Reader SDK using `VERDI_HOME`; the cache key already includes the converter
   name, so both backends can coexist.
2. Decide whether `default_converter` should prefer the SDK or the CLI when both
   exist (the plan is SDK first for the direct path, CLI as fallback).
3. A direct native reader (skipping VCD) only after the body cache is abstracted
   away from wellen's types; that is the point where FSDB stops being "converted"
   and `container_format()` stops being `Vcd`.
