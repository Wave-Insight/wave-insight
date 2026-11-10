//! Waveform backend: read VCD, FST and GHW through one API.
//!
//! This is the only waveform reader in the crate: the hand written VCD parser
//! and the `Module`/`ModuleValue` model it fed are gone (see R3 in
//! `docs/rebuild-plan.md`).
//!
//! # What you get
//!
//! * [`Waveform`] - open by path, by bytes or from any [`std::io::Read`]; the
//!   format is detected from the magic bytes (VCD starts with `$`, FST with a
//!   header block, GHW with a gzip wrapper) and then parsed by wellen, so no
//!   extension guessing is involved.
//! * [`HierarchyView`] - scopes and variables as owned data: full path
//!   (`top.u_cpu.pc`), name, bit width, declared index range (`[7:0]`), variable
//!   type and payload kind. Hierarchy resolution only touches the file header.
//! * [`WaveValue`] - owned values that keep `x`/`z`/`h`/`u`/`w`/`l`/`-` apart
//!   and can also be a real, a string or an event.
//! * [`Waveform::time_range_hint`] - the trace's start/end without reading the
//!   body (see [Time range](self#time-range)).
//! * Value queries, three tiers (see [Choosing a value query](self#choosing-a-value-query)):
//!   * [`Waveform::decoded_changes`] / [`DecodedSignal`] - cached per signal
//!     history, the per frame path for a renderer,
//!   * [`Waveform::value_at`] - one time stamp, `O(log n)`, no history,
//!   * [`Waveform::changes_in_window`] / [`Waveform::stream_changes`] - streamed
//!     window, nothing kept, re-reads the body on every call.
//!
//! # Time range
//!
//! Opening a dump must not read its body, but the wave view needs a time axis to
//! draw the first frame. [`Waveform::time_range_hint`] answers that cheaply and
//! is memoized:
//!
//! * **VCD** - the first `#<number>` token is found with a bounded forward scan
//!   (the header is a few KiB in practice) and the last one by reading only the
//!   tail of the file, so the cost is `O(header + tail)` instead of `O(file)`
//!   no matter how big the dump is.  `$date`/`$version`/`$comment` blocks are
//!   skipped because they may contain anything; a `#` after whitespace is
//!   accepted (VCD is a token format).  No value is decoded, no per-signal
//!   allocation happens.  Anything the bounded scans cannot decide falls back to
//!   a full scan, so the hint is never worse than before.
//! * **FST** - the header already carries the time table, so the body handle is
//!   opened (hierarchy + time table only, no signal data) and kept for later.
//! * **GHW** - `None`: wellen only knows the time range after reading the whole
//!   body. Callers fall back to [`Waveform::end_time`], which is exact and
//!   expensive.
//!
//! The hint is *best effort*: a truncated VCD reports the last time stamp it
//! actually contains, and a header that never closes a command block reports
//! `None` instead of a garbage number. [`Waveform::start_time`] and
//! [`Waveform::end_time`] are the exact answers; they read the body for
//! VCD/GHW and are cheap for FST.
//!
//! # Choosing a value query
//!
//! | what you are doing | call | cost |
//! |---|---|---|
//! | drawing the visible rows (recommended) | [`Waveform::window_changes_many`] | one pass, fair per-row cap, O(rows * cap) memory |
//! | drawing after a cached decode | [`Waveform::decoded_changes_many`], then [`DecodedSignal::window`] per row | one pass over the body, full histories in memory |
//! | drawing one row | [`Waveform::decoded_changes`] + [`DecodedSignal::window`] | decode once per signal, then binary search |
//! | cursor readout, tooltips | [`Waveform::stream_value_before`] | one change, O(1) retained memory (FST seeks; VCD forward scan) |
//! | value when the body is already warm | [`Waveform::value_at`] | one value out of the random-access cache |
//! | one-off query over a huge signal | [`Waveform::changes_in_window`] | re-reads the body, `O(window)` memory |
//! | export / tests / small dumps | [`Waveform::all_changes`] | converts the history on every call |
//!
//! The drawing path for large dumps is [`Waveform::window_changes_many`]: one
//! streamed pass for all visible rows, a fair per-row cap and no full-history
//! decode, so a dense clock cannot materialise gigabytes (FST seeks through its
//! time table; VCD scans forward to the window start, which is the format
//! limitation, not an extra decode).  Its `before` field is the value carried
//! into the left edge when the cache already holds the signal.
//!
//! A renderer must not call [`Waveform::changes_in_window`] per frame: it
//! re-streams the file on every call (that is what made the old per-row SVG
//! redraw slow). [`Waveform::decoded_changes`] is the accessor that makes a
//! redraw independent of the file size: it hands out a cheap handle that derefs
//! to `&[Change]`, so the visible slice is a binary search and no value is
//! cloned. The handle keeps working after [`Waveform::unload_signal`], because
//! it owns its snapshot.
//!
//! Ask for a whole screen of rows in **one** call
//! ([`Waveform::decoded_changes_many`]): wellen reads a set of signals per pass
//! over the body, so decoding rows one by one re-scans the file once per row.
//! The size of that win depends on the reader and the dump: it was originally
//! measured on an 8 MB FST (30 signals: 1.04 s sequentially, 73 ms batched),
//! which the checked-in 1.6 KB FST fixture is far too small to reproduce;
//! `benches/waveform.rs` (`decode/sequential_8` vs `decode/batched_8`) measures
//! the current numbers on whatever is checked in.
//!
//! # Memory behaviour
//!
//! The deleted `ModuleValue` model stored, per signal, a `Vec<i32>` of time
//! stamps plus one `(u8, u8)` pair per *eight* bits of every change, and it
//! allocated a separate `Vec` for each change: a 1-bit clock with 100 000
//! toggles ended up at several MiB although its actual payload is 200 KiB. On
//! top of that it materialised the whole dump during load, before the UI could
//! draw a first frame, and it did that for *every* signal whether it was
//! displayed or not.
//!
//! This backend does *not* do that:
//!
//! 1. Opening a file reads the header only. Hierarchy, widths and time scale
//!    are resolved once; values are not touched.
//! 2. [`Waveform::stream_changes`] re-reads the body through wellen's streaming
//!    API and only decodes the requested signals inside the requested window.
//!    Peak memory is proportional to the window, not to the trace.
//! 3. [`Waveform::value_at`] and [`Waveform::all_changes`] materialise per
//!    signal and on demand: wellen keeps the decoded signal in its packed
//!    encoding (2 or 4 bits per bit) inside a cache you can empty with
//!    [`Waveform::unload_signal`]. For a wide bus the payload is comparable to
//!    the old model; the win is that narrow signals no longer pay per-change
//!    allocation overhead and that nothing is decoded twice.
//! 4. [`Waveform::decoded_changes`] keeps a second, converted copy of a signal
//!    that a renderer draws: `size_of::<Change>()` (40 bytes on 64 bit) per
//!    change plus bus payloads, so 500k changes of one signal are ~20 MB. That
//!    is the price of a redraw that never touches the file. It is built per
//!    displayed signal on first use and [`Waveform::unload_signal`] /
//!    [`Waveform::unload_all_signals`] free it again (the precise byte counts
//!    are in [`CacheStats`]). A signal that is too big to hold should be drawn
//!    with [`Waveform::changes_in_window`] instead.
//! 5. [`Waveform::end_time`] is the only call that materialises the whole body
//!    on purpose (VCD/GHW); the UI never needs it, [`Waveform::time_range_hint`]
//!    is what the ruler uses.
//! 6. The hierarchy does not duplicate strings: scope/variable/type names are
//!    interned (one allocation per distinct name), every signal in a scope
//!    shares that scope's path prefix and component vector, and full signal
//!    paths are built lazily on first `path()` instead of at open.  A 200k
//!    signal / 100 scope hierarchy therefore costs ~164 bytes per signal
//!    (32.9 MB) instead of ~394 bytes (78.8 MB).  [`Waveform::signal_by_path`]
//!    walks the scope tree (O(path depth + signals of the scope)) instead of
//!    keeping one hash-map-full of path copies.
//! 7. [`Waveform::memory_stats`] reports the library-side accounting: retained
//!    source bytes, the hierarchy breakdown and the random access cache
//!    (time table, packed decodes, decoded histories and their payloads).
//!    [`CacheStats::total_bytes`] plus a sum over every open [`Waveform`] is the
//!    number a global memory budget should use.
//!
//! # Source ownership and byte sources
//!
//! A [`WaveformSource::Memory`] keeps its `Arc<[u8]>` alive for the lifetime of
//! the [`Waveform`] (streaming queries re-read it), so
//! [`MemoryStats::source_retained_bytes`] reports the whole file.  A
//! [`WaveformSource::Path`] keeps no bytes: streaming queries and the FST body
//! reader open the file fresh through [`ByteSource`], and the VCD/GHW bodies
//! are decoded into wellen's own in-memory representation.
//!
//! On native builds [`WaveformSource::Mmap`] / [`Waveform::open_path_mmap`] map
//! the file with memmap2 instead: readers serve pages straight from the mapping,
//! [`MemoryStats::source_retained_bytes`] stays 0 and
//! [`MemoryStats::source_mapped_bytes`] reports the file length (the OS page
//! cache, not the heap, holds it).  The mapping keeps the file handle open, so
//! on Windows the dump cannot be deleted or renamed until the waveform is
//! dropped; the mmap module is not compiled for the wasm build at all, which
//! keeps the browser path byte-identical.
//!
//! # Limitations
//!
//! * GHW can be opened and queried eagerly, but wellen cannot stream GHW;
//!   [`Waveform::stream_changes`] falls back to materialising the requested
//!   signals for GHW (correct, but slow and memory hungry). GHW also has no
//!   cheap time range.
//! * wellen may elide a change whose value repeats the previous one; a dump is
//!   not guaranteed to have one change per VCD line.
//! * `wellen`/`fst-reader` contain `assert!`/`todo!()` on input they consider
//!   impossible, so a hand crafted or half written file can reach one.
//!   [`Waveform`] makes a best effort to catch those panics and report
//!   [`WaveformError::Corrupt`] instead (the default panic hook still prints the
//!   message to stderr).  That only works in builds where Rust can unwind: the
//!   shipped release profile sets `panic = "abort"` (for wasm size), so in the
//!   released web/desktop builds such a panic aborts the app/webview instead.
//!   A panic raised by your own [`Waveform::stream_changes`] visitor is
//!   re-raised, not swallowed.

mod decoded;
mod error;
mod format;
mod hierarchy;
mod query;
mod render;
mod show_type;
mod source;
mod timescan;
mod value;

use std::collections::HashMap;
use std::fmt;
use std::io::Seek;
use std::path::Path;
use std::sync::Mutex;

use wellen::{LoadOptions, Signal, SignalRef, SignalSource, Time};

pub use decoded::{Change, DecodedSignal};
pub use error::{Result, WaveformError};
pub use format::{TimeUnit, Timescale, WaveFormat};
pub use hierarchy::{HierarchyStats, HierarchyView, ScopeNode, SignalInfo, SignalKind};
pub use query::ValueQuery;
// R3: the display preference lives with the waveform types now (the render API
// needs it); the legacy shared-types module it used to live in is gone.
pub use show_type::ShowType;
#[cfg(not(target_arch = "wasm32"))]
pub use source::MmapSource;
pub use source::{ByteReader, ByteSource, WaveformSource};
pub use value::{BitValue, Bits, WaveValue};

use error::catch_parse;
use source::{detect_format, validate_vcd};

/// Options used for every wellen read.
///
/// `multi_thread` is off: a single user opening a dump should not spawn a rayon
/// pool per query, and deterministic decoding makes bugs reproducible.
/// Empty scopes are folded away because they produce unusable paths
/// (`top..x`) in the viewer.
fn load_options() -> LoadOptions {
    LoadOptions {
        multi_thread: false,
        remove_scopes_with_empty_name: true,
    }
}

/// First and last time stamp of a trace, in wave units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimeRange {
    /// Time of the first change.
    pub start: Time,
    /// Time of the last change.
    pub end: Time,
}

impl TimeRange {
    /// Number of time units between the first and the last change.
    pub fn span(&self) -> Time {
        self.end.saturating_sub(self.start)
    }
}

/// The bounded, windowed change list of one signal (see
/// [Waveform::window_changes_many]).
#[derive(Clone, Debug, PartialEq)]
pub struct WindowedSignal {
    /// Changes with start <= time <= end, in time order and capped at the
    /// requested per-signal limit (delta-cycle duplicates preserved).
    pub changes: Vec<Change>,
    /// Value in effect at or just before the window start, when the backend can
    /// provide it without extra decoding (the signal is already in the
    /// materialised cache).  None on a cold cache; the renderer should then draw
    /// from the first in-window change and document the approximation.
    pub before: Option<WaveValue>,
    /// True when the per-signal cap stopped the collection (the signal has more
    /// changes in the window than were returned).
    pub capped: bool,
}

/// An opened waveform file.
///
/// Cheap to open (header only), `Send + Sync`, and all value queries take
/// `&self`, so it can be shared behind `&` or a mutex/`tauri::State`.
///
/// See the [module documentation](self) for the memory model and for what
/// [`Waveform::time_range_hint`] costs per format.
pub struct Waveform {
    source: WaveformSource,
    format: WaveFormat,
    hierarchy: HierarchyView,
    date: String,
    version: String,
    timescale: Option<Timescale>,
    /// Random access cache. `None` until an eager query needs the body; the
    /// header was already parsed when the file was opened.
    body: Mutex<Option<EagerBody>>,
    /// Cached answer of [`Waveform::time_range_hint`]: `None` = not asked yet.
    time_hint: Mutex<Option<Option<TimeRange>>>,
    /// Tiny LRU of recent [`Waveform::stream_value_before`] answers, so a cursor
    /// readout or inlay hint for the same (signal, time) does not rescan.
    lookups: Mutex<LookupCache>,
}

impl fmt::Debug for Waveform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Waveform")
            .field("format", &self.format)
            .field("source", &self.source)
            .field("signals", &self.hierarchy.len())
            .field("timescale", &self.timescale)
            .field(
                "body_loaded",
                &self.body.lock().map(|b| b.is_some()).unwrap_or(false),
            )
            .finish()
    }
}

impl Waveform {
    /// Open a waveform file from disk. The file is read twice: once for the
    /// header (now) and once for the body (on the first value query), because
    /// wellen's streaming interface is re-entered per query.
    pub fn open_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_source(WaveformSource::from_path(path.as_ref().to_path_buf()))
    }

    /// Open a waveform from a memory mapped file (native builds only).
    ///
    /// This is the desktop path for multi-GB dumps: the file is mapped instead
    /// of read into the heap, so [`MemoryStats::source_retained_bytes`] stays 0
    /// (the pages are in the OS page cache, backed by the file) and
    /// [`MemoryStats::source_mapped_bytes`] reports the mapping size.  All
    /// validation and value queries read through the same [`ByteSource`]
    /// interface, so a corrupt dump still answers Corrupt.
    ///
    /// Fails with [`WaveformError::Io`] when the map cannot be created (missing
    /// file, empty file, unsupported filesystem); callers can fall back to
    /// [`Waveform::open_path`].
    ///
    /// On Windows the mapping keeps the file handle open for the lifetime of the
    /// waveform, so the dump cannot be deleted or renamed until the
    /// [`Waveform`] is dropped.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path_mmap(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_source(WaveformSource::from_path_mmap(path)?)
    }

    /// Open a waveform that is already in memory.
    ///
    /// Useful for `include_bytes!` fixtures, for files that were read by the
    /// caller, or when the file is not on the local file system.
    pub fn open_bytes(bytes: impl Into<Vec<u8>>) -> Result<Self> {
        Self::open_source(WaveformSource::from_bytes(bytes.into()))
    }

    /// Open from anything readable.
    ///
    /// The reader does not have to be seekable, but it is drained into memory
    /// because wellen needs `Seek`; for big dumps prefer [`Waveform::open_path`].
    pub fn open_reader(mut reader: impl std::io::Read) -> Result<Self> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes)?;
        Self::open_bytes(bytes)
    }

    /// Open a known source (used by the constructors above).
    pub fn open_source(source: WaveformSource) -> Result<Self> {
        //our own magic check first: it keeps garbled input away from wellen's
        //FST probe, which would otherwise panic on it
        let mut reader = source.reader()?;
        let detected = detect_format(&mut reader)?;
        // VCD is decoded lazily (per signal, on first draw); a body value wider
        // than its declared variable would panic inside wellen then, which is an
        // unrecoverable abort in the release/wasm build.  Validate the whole VCD
        // structurally now, while we can still return a Corrupt error.
        if detected == WaveFormat::Vcd {
            reader.seek(std::io::SeekFrom::Start(0))?;
            validate_vcd(&mut reader)?;
        }

        let options = load_options();
        let header = catch_parse(detected, "read header", || {
            Ok(wellen::viewers::read_header(reader, &options)?)
        })?;
        let format =
            WaveFormat::from_wellen(header.file_format).ok_or(WaveformError::UnknownFormat)?;
        // Resolving the hierarchy walks wellen's arena and recurses into nested
        // scopes; a corrupt header with dangling handles or absurd nesting must
        // become a Corrupt error, not a panic/stack overflow.
        let hierarchy = catch_parse(format, "read hierarchy", || {
            HierarchyView::build(&header.hierarchy).map_err(|detail| WaveformError::Corrupt {
                format,
                operation: "read hierarchy",
                detail,
            })
        })?;
        let date = header.hierarchy.date().to_string();
        let version = header.hierarchy.version().to_string();
        let timescale = header
            .hierarchy
            .timescale()
            .and_then(Timescale::from_wellen);
        Ok(Self {
            source,
            format,
            hierarchy,
            date,
            version,
            timescale,
            body: Mutex::new(None),
            time_hint: Mutex::new(None),
            lookups: Mutex::new(LookupCache::new()),
        })
    }

    /// Where the data came from.
    pub fn source(&self) -> &WaveformSource {
        &self.source
    }

    /// Path of the file, if it was opened from disk (mapped or not).
    pub fn path(&self) -> Option<&Path> {
        self.source.path()
    }

    /// Number of bytes memory mapped by this waveform (native builds only);
    /// 0 for the file and memory sources.  See
    /// [`MemoryStats::source_mapped_bytes`] for the same number in the full
    /// accounting.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn mapped_bytes(&self) -> usize {
        self.source.mapped_len()
    }

    /// Detected file format.
    pub fn file_format(&self) -> WaveFormat {
        self.format
    }

    /// `$date` of a VCD header, creation date of an FST/GHW file (may be empty).
    pub fn date(&self) -> &str {
        &self.date
    }

    /// `$version` of a VCD header, writer name of an FST/GHW file (may be empty).
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Time unit of all time stamps; `None` if the file does not declare one.
    pub fn timescale(&self) -> Option<Timescale> {
        self.timescale
    }

    /// The resolved variable hierarchy.
    pub fn hierarchy(&self) -> &HierarchyView {
        &self.hierarchy
    }

    /// All variables in depth first declaration order.
    pub fn signals(&self) -> &[SignalInfo] {
        self.hierarchy.signals()
    }

    /// Number of variables.
    pub fn signal_count(&self) -> usize {
        self.hierarchy.len()
    }

    /// `true` if the file declares no variables.
    pub fn is_empty(&self) -> bool {
        self.hierarchy.is_empty()
    }

    /// Look a variable up by its dense index.
    pub fn signal(&self, index: usize) -> Option<&SignalInfo> {
        self.hierarchy.signal(index)
    }

    /// Look a variable up by its full path, e.g. `top.u_cpu.pc`.
    pub fn signal_by_path(&self, path: &str) -> Option<&SignalInfo> {
        self.hierarchy.signal_by_path(path)
    }

    /// First/last time stamp from the FST header block, without opening the
    /// random-access body.
    ///
    /// The FST header block is [type][section length][HEADER_LENGTH][start][end]
    /// (the section length equals HEADER_LENGTH, 329).  Returns None for a gzip
    /// wrapped stream (the inner blocks are opaque) and for incomplete files
    /// whose header stamps are unset; the caller falls back to the body there.
    fn fst_header_time_range(&self) -> Result<Option<TimeRange>> {
        use std::io::{Read, Seek, SeekFrom};

        let mut reader = self.source.reader()?;
        reader.seek(SeekFrom::Start(0))?;
        let mut block_header = [0u8; 9];
        if reader.read_exact(&mut block_header).is_err() {
            return Ok(None);
        }
        if block_header[0] != 0 {
            // GZipWrapper (254) or an unknown first block: need the body.
            return Ok(None);
        }
        let mut block_len_bytes = [0u8; 8];
        block_len_bytes.copy_from_slice(&block_header[1..9]);
        let block_len = u64::from_be_bytes(block_len_bytes);
        // The block length equals the header section length (fst-reader asserts
        // 329).  The first u64 at the payload start is that same section length
        // (read_header re-reads it); start_time and end_time follow.
        let mut payload = [0u8; 16];
        if block_len < 24 || reader.read_exact(&mut payload).is_err() {
            return Ok(None);
        }
        let start = u64::from_be_bytes(payload[0..8].try_into().expect("8 bytes"));
        let end = u64::from_be_bytes(payload[8..16].try_into().expect("8 bytes"));
        if start == 0 && end == 0 {
            return Ok(None); // incomplete FST: the body infers the range
        }
        Ok(Some(TimeRange { start, end }))
    }

    /// The trace's start and end without reading the body, when that is cheap.
    ///
    /// This is what a viewer needs to draw its first frame: the ruler and the
    /// fit-to-trace zoom. It is memoized, and its cost depends on the format:
    ///
    /// * **VCD**: a bounded forward scan finds the first `#<number>` token and a
    ///   tail read finds the last one, so the cost is `O(header + tail)` and
    ///   independent of the body size (8.3 MB / 500k changes: ~0.7 ms, was
    ///   ~13.6 ms when the whole body was scanned). `$date`/`$version`/
    ///   `$comment` blocks are skipped (they are free text and may contain
    ///   anything); if the bounded scans cannot decide, the exhaustive scan is
    ///   used instead.
    /// * **FST**: start and end time are read from the header block without
    ///   opening the body (no time table allocation); only gzip-wrapped or
    ///   incomplete files fall back to materialising the body.
    /// * **GHW**: `None`. wellen only knows the range after reading the body;
    ///   fall back to [`Waveform::end_time`] and accept the cost.
    ///
    /// The answer is a hint: a truncated VCD reports the last time stamp it
    /// contains, and a header with an unterminated `$command` block reports
    /// `None` rather than a number from its free text. [`Waveform::end_time`]
    /// is exact.
    pub fn time_range_hint(&self) -> Result<Option<TimeRange>> {
        let mut slot = self
            .time_hint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = *slot {
            return Ok(cached);
        }
        let hint = match self.format {
            WaveFormat::Vcd => {
                let mut reader = self.source.reader()?;
                timescan::scan_vcd_time_range(&mut reader)?
            }
            WaveFormat::Fst => {
                // The FST header block carries the first and last time stamp;
                // read them without opening the body, so a dump tab summary
                // does not materialise the whole time table (8 B per step).
                // Gzip-wrapped or incomplete files fall back to the body.
                match self.fst_header_time_range()? {
                    Some(range) => Some(range),
                    None => {
                        let mut guard = self.lock_body();
                        let body = self.ensure_body(&mut guard)?;
                        let start = body.time_table.first().copied();
                        let end = body.time_table.last().copied();
                        start.zip(end).map(|(start, end)| TimeRange { start, end })
                    }
                }
            }
            WaveFormat::Ghw => None,
        };
        *slot = Some(hint);
        Ok(hint)
    }

    /// Last time stamp that has any change in the file. **Exact, but reads the
    /// body** for VCD/GHW (cheap for FST, whose time table is in the header).
    ///
    /// The viewer uses [`Waveform::time_range_hint`] instead; this is for
    /// exports and for tests that need the authoritative answer.
    pub fn end_time(&self) -> Result<Option<Time>> {
        let mut guard = self.lock_body();
        let body = self.ensure_body(&mut guard)?;
        Ok(body.time_table.last().copied())
    }

    /// First time stamp that has any change in the file. Same cost as
    /// [`Waveform::end_time`].
    pub fn start_time(&self) -> Result<Option<Time>> {
        let mut guard = self.lock_body();
        let body = self.ensure_body(&mut guard)?;
        Ok(body.time_table.first().copied())
    }

    /// Every change of one variable, in chronological order.
    ///
    /// This decodes the whole history of that variable, converts it into owned
    /// values and **does not keep the result**: the next call converts again.
    /// Use it for tests, exports and small signals, not in a draw loop - a
    /// renderer wants [`Waveform::decoded_changes`] (cached) or
    /// [`Waveform::changes_in_window`] (streamed).
    pub fn all_changes(&self, signal: usize) -> Result<Vec<(Time, WaveValue)>> {
        let signal_ref = self.signal_ref(signal)?;
        let mut guard = self.lock_body();
        let body = self.ensure_body(&mut guard)?;
        body.ensure_loaded(signal_ref)?;

        let format = body.format;
        let time_table = &body.time_table;
        let packed = body.packed(signal_ref).ok_or_else(|| {
            WaveformError::Backend(format!("signal `{}` did not decode", self.path_of(signal)))
        })?;
        // One change per packed element: reserve up front so a 100k change
        // history does not reallocate and copy its way to the final size.
        let mut changes = Vec::with_capacity(packed.time_indices().len());
        for (time_index, value) in packed.iter_changes() {
            changes.push((
                change_time(format, time_table, time_index)?,
                WaveValue::from_wellen(value),
            ));
        }
        Ok(changes)
    }

    /// The decoded history of one variable, cached and shared.
    ///
    /// This is the per frame accessor for a renderer: the signal is decoded and
    /// converted **once**, the result stays in the per signal cache, and every
    /// later call returns the same allocation (see
    /// [`DecodedSignal::shares_storage_with`]) without touching the file again.
    ///
    /// The handle derefs to `&[Change]`, so slicing it is free:
    ///
    /// ```no_run
    /// # use wave_insight_lib::waveform::{Waveform, WaveValue};
    /// # fn draw(waveform: &Waveform, signal: usize) -> Result<(), Box<dyn std::error::Error>> {
    /// let decoded = waveform.decoded_changes(signal)?;
    /// // value in effect at the left edge of the visible window
    /// let seed: Option<&WaveValue> = decoded.value_at(1_000);
    /// // only the transitions that are actually on screen
    /// for (time, value) in decoded.window(1_000, Some(2_000)) {
    ///     let _ = (seed, time, value);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Memory
    ///
    /// The cache keeps the whole history of the signal: `size_of::<Change>()`
    /// (40 bytes on 64 bit) per change plus the heap payload of bus values, so
    /// 500k changes of one signal are ~20 MB. That is the price of never
    /// re-reading the file for a redraw. It is paid per *displayed* signal, on
    /// demand, and it is given back by [`Waveform::unload_signal`] (call it when
    /// a row scrolls out of view). [`Waveform::cache_stats`] reports what is
    /// currently held.
    ///
    /// If a signal is too big to hold, [`Waveform::changes_in_window`] streams
    /// just the visible window (`O(window)` memory, but it re-reads the body),
    /// and [`Waveform::value_at`] answers a single time stamp without decoding
    /// any history at all.
    pub fn decoded_changes(&self, signal: usize) -> Result<DecodedSignal> {
        let signal_ref = self.signal_ref(signal)?;
        let mut guard = self.lock_body();
        let body = self.ensure_body(&mut guard)?;
        body.decoded(signal_ref)
    }

    /// The decoded histories of several variables, decoded in **one** pass.
    ///
    /// This is the call a renderer should use for the rows of a frame: wellen
    /// reads a set of signals per pass over the body, so decoding 60 signals one
    /// by one re-scans the file 60 times while this does it once. On a large FST
    /// dump that is the difference between a ~20 ms and a ~1 s frame when the
    /// visible rows change.
    ///
    /// Signals that are already cached are not decoded again; duplicates in
    /// `signals` are decoded once. The returned handles are in the order of
    /// `signals` (duplicates included) and share storage with the handles
    /// [`Waveform::decoded_changes`] would return.
    pub fn decoded_changes_many(&self, signals: &[usize]) -> Result<Vec<DecodedSignal>> {
        if signals.is_empty() {
            return Ok(Vec::new());
        }
        let mut refs = Vec::with_capacity(signals.len());
        for &index in signals {
            refs.push(self.signal_ref(index)?);
        }

        let mut guard = self.lock_body();
        let body = self.ensure_body(&mut guard)?;
        body.ensure_loaded_many(&refs)?;

        let mut decoded = Vec::with_capacity(refs.len());
        for &signal_ref in &refs {
            decoded.push(body.decoded(signal_ref)?);
        }
        Ok(decoded)
    }

    /// What the per signal cache currently holds.
    ///
    /// Cheap (`O(cached signals)`) and side effect free: it never reads the body
    /// itself, so calling it before the first query reports zeros. Meant for
    /// diagnostics and for UI code that wants to prune the cache
    /// ([`Waveform::unload_signal`]) when it grows.
    pub fn cache_stats(&self) -> CacheStats {
        let guard = self.lock_body();
        match guard.as_ref() {
            Some(body) => body.cache_stats(),
            None => CacheStats::default(),
        }
    }

    /// Library-side memory accounting of this waveform.
    ///
    /// Sums the retained source bytes (only for [`WaveformSource::Memory`]; file
    /// and memory-mapped sources keep no heap bytes), the mapped length (native
    /// mmap source only), the resolved hierarchy (names, paths, lookup tables)
    /// and the random access cache (time table, wellen packed decodes, decoded
    /// histories including bus payloads).
    ///
    /// The account is a heap estimate, not RSS: allocator overhead, the OS page
    /// cache behind a mapping and wellen's own copy of the hierarchy inside
    /// [`EagerBody`] are not counted.
    pub fn memory_stats(&self) -> MemoryStats {
        let hierarchy = self.hierarchy.memory_stats();
        let source_retained_bytes = self.source.retained_bytes();
        #[cfg(not(target_arch = "wasm32"))]
        let source_mapped_bytes = self.source.mapped_len();
        #[cfg(target_arch = "wasm32")]
        let source_mapped_bytes = 0;
        let mut stats = MemoryStats {
            source_retained_bytes,
            source_mapped_bytes,
            hierarchy_bytes: hierarchy.total_bytes(),
            hierarchy,
            ..MemoryStats::default()
        };
        let guard = self.lock_body();
        if let Some(body) = guard.as_ref() {
            let cache = body.cache_stats();
            stats.packed_signals = cache.loaded_signals;
            stats.packed_bytes = cache.packed_bytes;
            stats.decoded_signals = cache.decoded_signals;
            stats.decoded_changes = cache.decoded_changes;
            stats.decoded_struct_bytes = cache.decoded_struct_bytes;
            stats.decoded_payload_bytes = cache.decoded_payload_bytes;
            stats.time_table_bytes = cache.time_table_bytes;
            stats.cache_bytes = cache.total_bytes();
        }
        {
            let cache = self
                .lookups
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            stats.lookup_cache_entries = cache.entries.len();
            stats.lookup_cache_bytes = cache.memory_bytes();
        }
        stats.total_bytes = stats.hierarchy_bytes
            + stats.source_retained_bytes
            + stats.cache_bytes
            + stats.lookup_cache_bytes;
        stats
    }

    /// The value one variable had at `time`.
    ///
    /// Returns the last change at or before `time`, `None` if the variable has
    /// not changed yet (or has no changes at all). This reuses the same per
    /// signal cache as [`Waveform::decoded_changes`]: it never builds a history,
    /// it reads a single value, and if the signal was already decoded by
    /// [`Waveform::decoded_changes`] it answers out of that slice.
    pub fn value_at(&self, signal: usize, time: Time) -> Result<Option<WaveValue>> {
        let signal_ref = self.signal_ref(signal)?;
        let mut guard = self.lock_body();
        let body = self.ensure_body(&mut guard)?;
        body.value_at(signal_ref, time)
    }

    /// The last value change at or before \`time\`, without materialising the
    /// random-access body.
    ///
    /// This is the memory-flat lookup for cursor readouts and editor inlay
    /// hints: unlike [\`Waveform::value_at\`] it never calls \`ensure_body\`, so no
    /// FST time table (8 bytes per time step) and no packed signal decodes are
    /// retained.  It streams the source:
    ///
    /// * FST: exponentially growing backward windows through
    ///   [\`Waveform::stream_changes\`], which uses the FST time table to seek.
    ///   Dense signals are answered by the first small window; a signal that
    ///   never changed degrades to a full prefix scan, still O(1) memory.
    /// * VCD: one forward scan from 0 to \`time\` (the format cannot seek): O(time)
    ///   decode time but O(1) memory.
    /// * GHW: wellen cannot stream GHW; the honest fallback materialises the
    ///   requested signal like [\`Waveform::value_at\`] does (correct, but not
    ///   memory-flat).  Callers that care should keep GHW rows on the body path.
    ///
    /// The returned pair is (change time, value); a change exactly at \`time\` is
    /// returned, and \`None\` means the signal has no change at or before \`time\`.
    /// Answers are memoised in a small per-waveform LRU (see
    /// [\`MemoryStats::lookup_cache_bytes\`]), so asking the same (signal, time)
    /// again - the usual case for a cursor readout redraw - costs nothing.
    pub fn stream_value_before(
        &self,
        signal: usize,
        time: Time,
    ) -> Result<Option<(Time, WaveValue)>> {
        // Validate the index even when the answer is cached: an unknown signal
        // must stay an error.
        let _ = self.signal_ref(signal)?;
        {
            let cache = self
                .lookups
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(cached) = cache.get(signal, time) {
                return Ok(cached);
            }
        }
        let result = if self.format == WaveFormat::Ghw {
            // wellen cannot stream GHW: the materialised fallback is the same
            // path value_at uses (documented in the method docs).
            self.stream_last_change(signal, 0, time)?
        } else if self.format == WaveFormat::Vcd {
            // Forward-only format: one scan from the beginning to the requested
            // time, keeping only the last value (O(1) memory).
            self.stream_last_change(signal, 0, time)?
        } else {
            // Seekable format (FST): probe a few exponentially growing backward
            // windows; each wellen stream call re-reads the relevant section's
            // time table, so the probe list is capped and one prefix scan is the
            // final answer for sparse/constant signals.
            let mut result = None;
            for span in [1u64, 1024, 1_048_576] {
                let start = time.saturating_sub(span.saturating_sub(1));
                result = self.stream_last_change(signal, start, time)?;
                if result.is_some() || start == 0 {
                    break;
                }
            }
            if result.is_none() {
                result = self.stream_last_change(signal, 0, time)?;
            }
            result
        };
        {
            let mut cache = self
                .lookups
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.insert(signal, time, result.clone());
        }
        Ok(result)
    }

    /// Batch form of [Waveform::stream_value_before] for a screen full of rows.
    ///
    /// Returns one entry per unique requested signal, in request order (duplicates
    /// collapse like [Waveform::window_changes_many]).  All signals share the same
    /// requested time, so the backward search runs as one stream pass with a signal
    /// filter instead of one pass per signal - on FST the relevant section time
    /// table is then read once per probe for the whole batch.  The probe strategy,
    /// the per-signal LRU and the accounting are the same as for the single-signal
    /// call; every answer is also stored in the LRU, so an identical batch (or a
    /// single [Waveform::stream_value_before] for one of the rows) is free.
    ///
    /// GHW cannot be streamed by wellen and falls back to materialising the
    /// requested signals, as documented on [Waveform::stream_value_before].
    pub fn values_before(
        &self,
        signals: &[usize],
        time: Time,
    ) -> Result<Vec<Option<(Time, WaveValue)>>> {
        let mut unique: Vec<usize> = Vec::with_capacity(signals.len());
        let mut slot_by_signal: Vec<Option<usize>> = vec![None; self.hierarchy.len()];
        for &signal in signals {
            let Some(slot) = slot_by_signal.get(signal).copied() else {
                return Err(WaveformError::UnknownSignal(format!("#{signal}")));
            };
            if slot.is_some() {
                continue;
            }
            let _ = self.signal_ref(signal)?;
            slot_by_signal[signal] = Some(unique.len());
            unique.push(signal);
        }
        let mut answers: Vec<Option<Option<(Time, WaveValue)>>> = vec![None; unique.len()];
        if unique.is_empty() {
            return Ok(Vec::new());
        }

        // Answers already memoised (per signal LRU) are free.
        {
            let cache = self
                .lookups
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for (slot, &signal) in unique.iter().enumerate() {
                if let Some(cached) = cache.get(signal, time) {
                    answers[slot] = Some(cached);
                }
            }
        }

        // Signals whose answer is still unknown.
        let mut pending: Vec<usize> = unique
            .iter()
            .enumerate()
            .filter(|(slot, _)| answers[*slot].is_none())
            .map(|(_, &signal)| signal)
            .collect();
        if !pending.is_empty() {
            if self.format == WaveFormat::Ghw || self.format == WaveFormat::Vcd {
                // Forward-only / non-streamable: one pass from 0 to time.
                let results = self.stream_last_changes(&pending, 0, time)?;
                for (&signal, result) in pending.iter().zip(results) {
                    answers[slot_by_signal[signal].expect("requested signal")] = Some(result);
                }
                pending.clear();
            } else {
                // Seekable (FST): the same capped probes as the single lookup, but
                // one stream pass for all still-missing signals at a time.
                for span in [1u64, 1024, 1_048_576] {
                    let start = time.saturating_sub(span.saturating_sub(1));
                    let results = self.stream_last_changes(&pending, start, time)?;
                    let mut still_missing = Vec::new();
                    for (&signal, result) in pending.iter().zip(results) {
                        let slot = slot_by_signal[signal].expect("requested signal");
                        if result.is_some() || start == 0 {
                            answers[slot] = Some(result);
                        } else {
                            still_missing.push(signal);
                        }
                    }
                    pending = still_missing;
                    if pending.is_empty() {
                        break;
                    }
                }
                if !pending.is_empty() {
                    let results = self.stream_last_changes(&pending, 0, time)?;
                    for (&signal, result) in pending.iter().zip(results) {
                        answers[slot_by_signal[signal].expect("requested signal")] = Some(result);
                    }
                }
            }
        }

        // Memoise everything (including None answers) and return request order.
        {
            let mut cache = self
                .lookups
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for (slot, &signal) in unique.iter().enumerate() {
                if let Some(result) = answers[slot].as_ref() {
                    cache.insert(signal, time, result.clone());
                }
            }
        }
        Ok(answers
            .into_iter()
            .map(|answer| answer.unwrap_or(None))
            .collect())
    }

    /// Streaming primitive for the batch lookup: the last change of each signal
    /// inside [start, end], in the order of signals.
    fn stream_last_changes(
        &self,
        signals: &[usize],
        start: Time,
        end: Time,
    ) -> Result<Vec<Option<(Time, WaveValue)>>> {
        let mut lasts: Vec<Option<(Time, WaveValue)>> = vec![None; signals.len()];
        if signals.is_empty() {
            return Ok(lasts);
        }
        let mut slot_by_variable: Vec<Option<usize>> = vec![None; self.hierarchy.len()];
        for (slot, &signal) in signals.iter().enumerate() {
            slot_by_variable[signal] = Some(slot);
        }
        let query = ValueQuery::for_signals(signals.iter().copied()).between(start, Some(end));
        self.stream_changes(&query, |variable, time, value| {
            if let Some(slot) = slot_by_variable.get(variable).copied().flatten() {
                lasts[slot] = Some((time, value));
            }
            true
        })?;
        Ok(lasts)
    }
    /// Last change of one signal inside [start, end]; the streaming primitive
    /// behind [Waveform::stream_value_before].
    fn stream_last_change(
        &self,
        signal: usize,
        start: Time,
        end: Time,
    ) -> Result<Option<(Time, WaveValue)>> {
        let query = ValueQuery::window(signal, start, Some(end));
        let mut last = None;
        self.stream_changes(&query, |_, time, value| {
            last = Some((time, value));
            true
        })?;
        Ok(last)
    }

    /// Drop the decoded data of one variable from the random access cache.
    ///
    /// Frees both the packed decode and the decoded history; the hierarchy and
    /// the time table stay. Handles already obtained from
    /// [`Waveform::decoded_changes`] keep their snapshot alive (they own it), the
    /// next call decodes again. Does nothing if the body was never materialised.
    pub fn unload_signal(&self, signal: usize) -> Result<()> {
        let signal_ref = self.signal_ref(signal)?;
        let mut guard = self.lock_body();
        if let Some(body) = guard.as_mut() {
            body.signals.remove(&signal_ref);
        }
        Ok(())
    }

    /// Drop every decoded signal from the cache, keeping the parsed body.
    ///
    /// Use it when the visible row set changes as a whole (new filter, new
    /// module selected) to give the memory back without paying for the body
    /// again. [`Waveform::unload_signal`] is the fine grained variant.
    pub fn unload_all_signals(&self) {
        let mut guard = self.lock_body();
        if let Some(body) = guard.as_mut() {
            body.signals.clear();
        }
    }

    /// Throw away the whole random access cache and go back to the state right
    /// after opening: the next value query re-reads the file.
    ///
    /// This frees the parsed body (for VCD that is wellen's in-memory
    /// representation of the whole trace), the time table and every decoded
    /// signal, which together are the biggest block the backend holds. Handles
    /// obtained from [`Waveform::decoded_changes`] keep their snapshot alive.
    pub fn release_cache(&self) {
        *self.lock_body() = None;
    }

    /// Report value changes without materialising the whole trace.
    ///
    /// Only the changes matching `query` are decoded; `visit` is called with
    /// `(signal index, time, value)` in time order and returns `false` to stop
    /// early. For VCD and FST the body is streamed, so memory use is bounded by
    /// the window - this is the intended path for drawing.
    ///
    /// GHW cannot be streamed by wellen; the same result is produced by
    /// materialising the requested signals one by one (documented fallback).
    ///
    /// # Aliased variables
    ///
    /// Several variables can share one signal (the same VCD identifier or FST
    /// handle).  The visitor is called once per *requested* variable: when
    /// [`ValueQuery::signals`] names one alias, the callback receives exactly
    /// that index; when the query covers every variable ([`ValueQuery::all`]) a
    /// change of an aliased signal is reported for every alias, in declaration
    /// order.  The stream itself still decodes the signal only once.
    pub fn stream_changes<F>(&self, query: &ValueQuery, mut visit: F) -> Result<()>
    where
        F: FnMut(usize, Time, WaveValue) -> bool,
    {
        if self.format == WaveFormat::Ghw {
            return self.stream_changes_materialised(query, visit);
        }

        let signal_refs = match &query.signals {
            None => None,
            Some(indices) => {
                let mut refs = Vec::with_capacity(indices.len());
                for &index in indices {
                    refs.push(self.signal_ref(index)?);
                }
                Some(refs)
            }
        };

        let filter = wellen::stream::Filter {
            start: query.start,
            end: query.end,
            signals: signal_refs.as_deref(),
        };
        let options = load_options();
        let reader = self.source.reader()?;

        let mut emitted = 0usize;
        let limit = query.max_changes;
        //a panic inside wellen while decoding must not escape as a crash, but a
        //panic from the caller's visitor must
        let in_visitor = std::cell::Cell::new(false);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut streamed = wellen::stream::read(reader, &options)?;
            let result = streamed.stream_changes(
                filter,
                |time, signal_ref, value| -> std::result::Result<(), ()> {
                    let Some(variables) = self.hierarchy.variables_of_signal(signal_ref) else {
                        //a signal without a variable (an internal signal)
                        return Ok(());
                    };
                    //belt and braces: some backends only use the filter as a hint
                    if !query.includes_time(time) {
                        return Ok(());
                    }
                    // One change per variable that aliases this signal, so a
                    // query for an alias reports the alias (not the first
                    // variable of the shared signal).  The value is a Copy view
                    // into the decoder, so this does not copy the payload.
                    for &variable in variables {
                        if !query.includes_signal(variable) {
                            continue;
                        }
                        emitted += 1;
                        in_visitor.set(true);
                        let keep_going = visit(variable, time, WaveValue::from_wellen(value));
                        in_visitor.set(false);
                        if !keep_going || limit.map(|limit| emitted >= limit).unwrap_or(false) {
                            return Err(());
                        }
                    }
                    Ok(())
                },
            );
            Ok::<_, wellen::WellenError>(result)
        }));

        match outcome {
            Ok(Ok(Ok(()))) => Ok(()),
            //the visitor asked to stop
            Ok(Ok(Err(wellen::stream::StreamError::Callback(())))) => Ok(()),
            Ok(Ok(Err(wellen::stream::StreamError::Wellen(error)))) => Err(error.into()),
            Ok(Err(error)) => Err(error.into()),
            Err(payload) => {
                if in_visitor.get() {
                    //the visitor panicked: let the panic continue its way up
                    std::panic::resume_unwind(payload)
                } else {
                    Err(WaveformError::Corrupt {
                        format: self.format,
                        operation: "stream values",
                        detail: error::panic_detail(payload.as_ref()),
                    })
                }
            }
        }
    }

    /// Changes of one variable between `start` and `end` (both inclusive).
    ///
    /// For VCD and FST this is the streaming path: nothing outside the window is
    /// decoded and nothing is kept afterwards. `end == None` means "until the end
    /// of the trace", which is equivalent to (but gentler than)
    /// [`Waveform::all_changes`]. GHW cannot be streamed, see
    /// [`Waveform::stream_changes`] for that fallback.
    pub fn changes_in_window(
        &self,
        signal: usize,
        start: Time,
        end: Option<Time>,
    ) -> Result<Vec<(Time, WaveValue)>> {
        let query = ValueQuery::window(signal, start, end);
        let mut changes = Vec::new();
        self.stream_changes(&query, |_, time, value| {
            changes.push((time, value));
            true
        })?;
        Ok(changes)
    }

    /// Bounded window query for drawing: the last value changes of several
    /// signals inside a time window, with a FAIR per-signal cap and a single
    /// pass over the body.
    ///
    /// This is the rendering path for large dumps: unlike
    /// [Waveform::decoded_changes_many] it never materialises the full history
    /// of a row, and unlike a global max_changes query the cap is per signal, so
    /// one dense clock cannot starve the other visible rows.  The stream stops
    /// as soon as every requested signal hit the cap.
    ///
    /// * signals: variable indices (duplicates are collapsed, order kept);
    /// * start/end: inclusive window, None means until the end of the trace;
    ///   start > end returns empty lists without touching the file;
    /// * max_changes_per_signal: a cap of 0 returns empty lists with
    ///   capped = true and no stream pass.
    ///
    /// before is best effort: it is only filled when the signal is already in
    /// the materialised decode cache (no second full-history scan is done to
    /// obtain it).  FST and VCD are streamed, so memory is O(signals * cap);
    /// GHW cannot be streamed by wellen and falls back to materialising the
    /// requested signals (values are the same, memory is not O(window)).
    ///
    /// Aliases: a requested alias reports its own index (same semantics as
    /// [Waveform::stream_changes]); the shared signal is decoded once.
    pub fn window_changes_many(
        &self,
        signals: &[usize],
        start: Time,
        end: Option<Time>,
        max_changes_per_signal: usize,
    ) -> Result<Vec<WindowedSignal>> {
        let mut indices: Vec<usize> = Vec::with_capacity(signals.len());
        let mut refs: Vec<SignalRef> = Vec::with_capacity(signals.len());
        let mut slot_by_variable: Vec<Option<usize>> = vec![None; self.hierarchy.len()];
        for &index in signals {
            let Some(slot) = slot_by_variable.get(index).copied() else {
                return Err(WaveformError::UnknownSignal(format!("#{index}")));
            };
            if slot.is_some() {
                continue; // duplicate request: keep the first slot
            }
            let signal_ref = self.signal_ref(index)?;
            slot_by_variable[index] = Some(indices.len());
            indices.push(index);
            refs.push(signal_ref);
        }
        let mut output: Vec<WindowedSignal> = indices
            .iter()
            .map(|_| WindowedSignal {
                changes: Vec::new(),
                before: None,
                capped: false,
            })
            .collect();
        if output.is_empty() {
            return Ok(output);
        }
        if max_changes_per_signal == 0 {
            for row in &mut output {
                row.capped = true;
            }
            return Ok(output);
        }
        if let Some(end) = end {
            if start > end {
                return Ok(output);
            }
        }

        let query = ValueQuery {
            signals: Some(indices),
            start,
            end,
            max_changes: None,
        };
        let mut capped_count = 0usize;
        self.stream_changes(&query, |variable, time, value| {
            let Some(slot) = slot_by_variable.get(variable).copied().flatten() else {
                return true;
            };
            let row = &mut output[slot];
            if row.changes.len() < max_changes_per_signal {
                row.changes.push((time, value));
                if row.changes.len() == max_changes_per_signal {
                    row.capped = true;
                    capped_count += 1;
                }
            }
            capped_count < output.len()
        })?;

        // Fill the left-edge value from the already materialised cache only:
        // never load or decode anything new for it (that would defeat the
        // bounded streaming path).
        let needs_before: Vec<(usize, SignalRef)> = output
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                row.changes
                    .first()
                    .map(|change| change.0 > start)
                    .unwrap_or(true)
            })
            .map(|(slot, _)| (slot, refs[slot]))
            .collect();
        if !needs_before.is_empty() {
            let guard = self.lock_body();
            if let Some(body) = guard.as_ref() {
                for (slot, signal_ref) in needs_before {
                    if let Some(value) = body.cached_value_at(signal_ref, start) {
                        output[slot].before = Some(value);
                    }
                }
            }
        }
        Ok(output)
    }

    /// GHW fallback: materialise the requested signals one at a time.
    fn stream_changes_materialised<F>(&self, query: &ValueQuery, mut visit: F) -> Result<()>
    where
        F: FnMut(usize, Time, WaveValue) -> bool,
    {
        let indices: Vec<usize> = match &query.signals {
            Some(indices) => indices.clone(),
            None => (0..self.hierarchy.len()).collect(),
        };
        let mut emitted = 0usize;
        for index in indices {
            for (time, value) in self.all_changes(index)? {
                if !query.includes_time(time) {
                    continue;
                }
                emitted += 1;
                if !visit(index, time, value) {
                    return Ok(());
                }
                if query
                    .max_changes
                    .map(|limit| emitted >= limit)
                    .unwrap_or(false)
                {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// Resolve a public signal index to wellen's handle.
    fn signal_ref(&self, signal: usize) -> Result<SignalRef> {
        self.hierarchy
            .signal(signal)
            .map(|info| info.signal_ref)
            .ok_or_else(|| WaveformError::UnknownSignal(format!("#{signal}")))
    }

    /// Path of a signal index, for error messages.
    fn path_of(&self, signal: usize) -> String {
        self.hierarchy
            .signal(signal)
            .map(|info| info.path().to_string())
            .unwrap_or_else(|| format!("#{signal}"))
    }

    /// Lock the body cache.
    ///
    /// Poisoning is recovered from: the cache is rebuilt from scratch after a
    /// `wellen` panic (see [`catch_parse`]), so it cannot be left half written.
    fn lock_body(&self) -> std::sync::MutexGuard<'_, Option<EagerBody>> {
        self.body
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Lazily read the body into the random access cache.
    fn ensure_body<'a>(&self, guard: &'a mut Option<EagerBody>) -> Result<&'a mut EagerBody> {
        if guard.is_none() {
            *guard = Some(EagerBody::open(&self.source, self.format)?);
        }
        guard
            .as_mut()
            .ok_or_else(|| WaveformError::Backend("waveform body could not be loaded".to_string()))
    }
}

/// Number of entries in the streaming-lookup LRU (bounded so the cache is
/// O(1)-ish and visible in [MemoryStats::lookup_cache_entries]).
const LOOKUP_CACHE_CAPACITY: usize = 64;

/// One cached [Waveform::stream_value_before] answer.
struct LookupEntry {
    signal: usize,
    time: Time,
    /// The change at or before the requested time, or None when the signal has
    /// no change before it.
    result: Option<(Time, WaveValue)>,
}

/// Bounded LRU for the memory-flat value lookup.
struct LookupCache {
    entries: std::collections::VecDeque<LookupEntry>,
}

impl LookupCache {
    fn new() -> Self {
        Self {
            // Lazily allocated: a waveform that never asks for a streaming
            // lookup must not pay a cache allocation up front.
            entries: std::collections::VecDeque::new(),
        }
    }

    fn get(&self, signal: usize, time: Time) -> Option<Option<(Time, WaveValue)>> {
        self.entries
            .iter()
            .find(|entry| entry.signal == signal && entry.time == time)
            .map(|entry| entry.result.clone())
    }

    fn insert(&mut self, signal: usize, time: Time, result: Option<(Time, WaveValue)>) {
        if let Some(position) = self
            .entries
            .iter()
            .position(|entry| entry.signal == signal && entry.time == time)
        {
            self.entries.remove(position);
        } else if self.entries.len() >= LOOKUP_CACHE_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(LookupEntry {
            signal,
            time,
            result,
        });
    }

    /// Heap bytes held by the cache (struct entries plus value payloads).
    fn memory_bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<LookupEntry>()
            + self
                .entries
                .iter()
                .filter_map(|entry| entry.result.as_ref())
                .map(|(_, value)| value.heap_bytes())
                .sum::<usize>()
    }
}

/// What [`Waveform::cache_stats`] found in the per signal cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Number of time steps in the body (0 if the body was never read).
    pub time_steps: usize,
    /// Signals decoded from the file (packed, wellen's own representation).
    pub loaded_signals: usize,
    /// Signals with a decoded history (see [`Waveform::decoded_changes`]).
    pub decoded_signals: usize,
    /// Total number of changes in all decoded histories.
    pub decoded_changes: usize,
    /// Bytes of the time table.
    pub time_table_bytes: usize,
    /// Bytes of wellen's packed signal decodes (time indices + payload).
    pub packed_bytes: usize,
    /// Bytes of the decoded history arrays (`decoded_changes * size_of::<Change>()`).
    pub decoded_struct_bytes: usize,
    /// Heap payload of the decoded values (bit vectors, strings).
    pub decoded_payload_bytes: usize,
}

impl CacheStats {
    /// Bytes held by the decoded histories, excluding bus value payloads.
    ///
    /// This is `decoded_changes * size_of::<Change>()`; see
    /// [`CacheStats::total_bytes`] for the full accounting including the packed
    /// decodes and the value payloads.
    pub fn decoded_bytes(&self) -> usize {
        self.decoded_struct_bytes
    }

    /// Every byte the random access cache holds: time table, packed decodes and
    /// decoded histories including their value payloads.
    ///
    /// Summing this over the open [`Waveform`]s gives the global cache budget a
    /// front end needs (together with [`Waveform::memory_stats`] for the
    /// hierarchy and the retained source bytes).
    pub fn total_bytes(&self) -> usize {
        self.time_table_bytes
            + self.packed_bytes
            + self.decoded_struct_bytes
            + self.decoded_payload_bytes
    }
}

/// Library-side memory accounting of one [`Waveform`].
///
/// All numbers are heap estimates: byte lengths of the owned strings, struct
/// sizes and container capacities.  Allocator overhead, wellen's internal copy
/// of the hierarchy inside the body cache and the loaded library itself are not
/// counted.  Meant for diagnostics, regression tests and a global memory budget
/// (the front end can sum a field such as [`MemoryStats::total_bytes`] or
/// [`CacheStats::total_bytes`] over every open waveform).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryStats {
    /// Source bytes kept alive on the application heap by
    /// [`WaveformSource::Memory`] (0 for file and memory-mapped sources; the OS
    /// page cache is not counted).
    pub source_retained_bytes: usize,
    /// Length of the memory mapping for a native [`WaveformSource::Mmap`]
    /// source, 0 for the other sources.  This is the "mapped but not retained on
    /// the heap" proof for the desktop build.
    pub source_mapped_bytes: usize,
    /// Hierarchy accounting, broken down by category.
    pub hierarchy: HierarchyStats,
    /// Sum of the hierarchy categories.
    pub hierarchy_bytes: usize,
    /// Signals decoded from the file into wellen's packed representation.
    pub packed_signals: usize,
    /// Bytes of those packed decodes.
    pub packed_bytes: usize,
    /// Signals with a decoded history.
    pub decoded_signals: usize,
    /// Changes in all decoded histories.
    pub decoded_changes: usize,
    /// Bytes of the decoded change arrays.
    pub decoded_struct_bytes: usize,
    /// Heap payload of the decoded values (bit vectors, strings).
    pub decoded_payload_bytes: usize,
    /// Bytes of the body time table.
    pub time_table_bytes: usize,
    /// Sum of the random access cache categories (see [`CacheStats::total_bytes`]).
    pub cache_bytes: usize,
    /// Entries in the streaming-lookup LRU (see
    /// [`Waveform::stream_value_before`]).
    pub lookup_cache_entries: usize,
    /// Heap bytes of that LRU, including value payloads.
    pub lookup_cache_bytes: usize,
    /// Everything above: hierarchy + retained source + cache + lookup LRU.
    pub total_bytes: usize,
}

/// A signal in the cache: the packed decode plus, once someone asked for it, the
/// converted history that a renderer draws from.
struct CachedSignal {
    packed: Signal,
    decoded: Option<DecodedSignal>,
}

/// Wellen's body plus a per signal decode cache.
///
/// This is deliberately *not* `wellen::simple::Waveform`: that type forces
/// `LoadOptions::default()` (multi threaded, empty scopes kept) and would parse
/// the header a second time with different options than the view we expose.
struct EagerBody {
    format: WaveFormat,
    hierarchy: wellen::Hierarchy,
    source: SignalSource,
    time_table: Vec<Time>,
    /// Decoded signals, keyed by wellen's handle index.
    signals: HashMap<SignalRef, CachedSignal>,
}

impl EagerBody {
    /// Read the body. For VCD/GHW this decodes the whole trace into wellen's
    /// packed in-memory format; for FST it only reads the time table, the
    /// signals are decoded on demand by [`EagerBody::ensure_loaded`].
    fn open(source: &WaveformSource, format: WaveFormat) -> Result<Self> {
        let options = load_options();
        catch_parse(format, "read body", || {
            let header = wellen::viewers::read_header(source.reader()?, &options)?;
            let hierarchy = header.hierarchy;
            let body = wellen::viewers::read_body(header.body, &hierarchy, None)?;
            Ok(Self {
                format,
                hierarchy,
                source: body.source,
                time_table: body.time_table,
                signals: HashMap::new(),
            })
        })
    }

    /// Decode `signal` if it is not cached yet.
    fn ensure_loaded(&mut self, signal: SignalRef) -> Result<()> {
        self.ensure_loaded_many(&[signal])
    }

    /// Decode every signal in `signals` that is not cached yet, in one pass.
    ///
    /// `wellen` reads a whole set of signals per request, so calling this once
    /// for the visible rows is much cheaper than looping over
    /// [`EagerBody::ensure_loaded`].
    fn ensure_loaded_many(&mut self, signals: &[SignalRef]) -> Result<()> {
        let mut missing: Vec<SignalRef> = Vec::new();
        for &signal in signals {
            if !self.signals.contains_key(&signal) && !missing.contains(&signal) {
                missing.push(signal);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        //wellen unwraps the encoding of every requested signal, refuse instead
        if let Some(bad) = missing
            .iter()
            .find(|signal| self.hierarchy.get_signal_tpe(**signal).is_none())
        {
            return Err(WaveformError::Backend(format!(
                "signal #{} has no encoding information",
                bad.index()
            )));
        }
        //decoding a corrupt body can panic inside wellen/fst-reader
        catch_parse(self.format, "decode signals", || {
            let loaded = self.source.load_signals(&missing, &self.hierarchy, false);
            for packed in loaded {
                self.signals.insert(
                    packed.signal_ref(),
                    CachedSignal {
                        packed,
                        decoded: None,
                    },
                );
            }
            Ok(())
        })?;
        if let Some(bad) = missing
            .iter()
            .find(|signal| !self.signals.contains_key(*signal))
        {
            return Err(WaveformError::Backend(format!(
                "signal #{} could not be decoded",
                bad.index()
            )));
        }
        Ok(())
    }

    /// The packed decode of a signal, if it is cached.
    fn packed(&self, signal: SignalRef) -> Option<&Signal> {
        self.signals.get(&signal).map(|cached| &cached.packed)
    }

    /// The decoded history of a signal, building and caching it on first use.
    fn decoded(&mut self, signal: SignalRef) -> Result<DecodedSignal> {
        self.ensure_loaded(signal)?;
        let format = self.format;
        let time_table = &self.time_table;
        let cached = self.signals.get_mut(&signal).ok_or_else(|| {
            WaveformError::Backend(format!("signal #{} is not cached", signal.index()))
        })?;
        if cached.decoded.is_none() {
            let mut changes: Vec<Change> = Vec::with_capacity(cached.packed.time_indices().len());
            for (time_index, value) in cached.packed.iter_changes() {
                changes.push((
                    change_time(format, time_table, time_index)?,
                    WaveValue::from_wellen(value),
                ));
            }
            cached.decoded = Some(DecodedSignal::new(changes.into()));
        }
        cached
            .decoded
            .clone()
            .ok_or_else(|| WaveformError::Backend("decoded history vanished".to_string()))
    }

    /// The value at `time`, preferring an already decoded history.
    fn value_at(&mut self, signal: SignalRef, time: Time) -> Result<Option<WaveValue>> {
        self.ensure_loaded(signal)?;
        let Some(cached) = self.signals.get(&signal) else {
            return Ok(None);
        };
        //if a renderer already decoded this signal, answer out of that slice:
        //same representation, no second conversion path in the result
        if let Some(decoded) = &cached.decoded {
            return Ok(decoded.value_at(time).cloned());
        }
        let Some(time_index) = self.time_index_at_or_before(time) else {
            return Ok(None);
        };
        let Some(offset) = cached.packed.get_offset(time_index) else {
            return Ok(None);
        };
        //delta cycles can produce several changes with the same time stamp;
        //the last one wins
        let element = offset.elements.saturating_sub(1);
        Ok(Some(WaveValue::from_wellen(
            cached.packed.get_value_at(&offset, element),
        )))
    }

    /// Snapshot of the cache contents.
    fn cache_stats(&self) -> CacheStats {
        let mut stats = CacheStats {
            time_steps: self.time_table.len(),
            time_table_bytes: self.time_table.capacity() * std::mem::size_of::<Time>(),
            ..CacheStats::default()
        };
        for cached in self.signals.values() {
            stats.loaded_signals += 1;
            stats.packed_bytes += cached.packed.size_in_memory();
            if let Some(decoded) = &cached.decoded {
                stats.decoded_signals += 1;
                stats.decoded_changes += decoded.len();
                stats.decoded_struct_bytes += decoded.len() * std::mem::size_of::<Change>();
                stats.decoded_payload_bytes += decoded
                    .iter()
                    .map(|(_, value)| value.heap_bytes())
                    .sum::<usize>();
            }
        }
        stats
    }

    /// Last value at or before `time` from the already cached data, without
    /// loading/decoding anything (used for the best-effort WindowedSignal.before).
    fn cached_value_at(&self, signal: SignalRef, time: Time) -> Option<WaveValue> {
        let cached = self.signals.get(&signal)?;
        if let Some(decoded) = &cached.decoded {
            return decoded.value_at(time).cloned();
        }
        let time_index = self.time_index_at_or_before(time)?;
        let offset = cached.packed.get_offset(time_index)?;
        let element = offset.elements.saturating_sub(1);
        Some(WaveValue::from_wellen(
            cached.packed.get_value_at(&offset, element),
        ))
    }

    /// Index of the last change at or before `time`.
    fn time_index_at_or_before(&self, time: Time) -> Option<u32> {
        let position = self.time_table.partition_point(|&entry| entry <= time);
        if position == 0 {
            None
        } else {
            Some((position - 1) as u32)
        }
    }
}

/// Time stamp of one packed change, guarding the time-table lookup.
///
/// The packed change carries an index into the body time table.  wellen derives
/// both from the same body, so a valid file cannot produce an out-of-range
/// index, but a corrupt or truncated one might.  Indexing the table directly
/// would panic from this crate (and the `catch_parse` guard does not wrap the
/// value conversion), so the lookup is checked and reported like any other
/// corrupt input instead.
fn change_time(format: WaveFormat, time_table: &[Time], time_index: u32) -> Result<Time> {
    time_table
        .get(time_index as usize)
        .copied()
        .ok_or_else(|| WaveformError::Corrupt {
            format,
            operation: "decode value change",
            detail: format!(
                "change time index {} is outside the {} time steps of the body",
                time_index,
                time_table.len()
            ),
        })
}

/// Sanity check that the shared waveform handle can be put into a Tauri state.
#[allow(dead_code)]
fn assert_send_sync() {
    fn is_send_sync<T: Send + Sync>() {}
    is_send_sync::<Waveform>();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wide, header-only VCD: one top scope with many child scopes, each with
    /// the same local signal names (that is exactly the duplication the
    /// hierarchy interning removes).
    fn wide_vcd(scopes: usize, per_scope: usize) -> String {
        let mut vcd = String::with_capacity(scopes * per_scope * 48);
        vcd.push_str("$date today $end\n$version memory-test $end\n");
        vcd.push_str("$timescale 1ns $end\n$scope module top $end\n");
        for scope in 0..scopes {
            vcd.push_str(&format!("$scope module s{scope} $end\n"));
            for signal in 0..per_scope {
                let identifier = scope * per_scope + signal;
                vcd.push_str(&format!(
                    "$var wire 8 v{identifier} sig_{signal} [7:0] $end\n"
                ));
            }
            vcd.push_str("$upscope $end\n");
        }
        vcd.push_str("$upscope $end\n$enddefinitions $end\n#0\n");
        vcd
    }

    // -- VCD structural validation (malformed width/body guard) -------------

    /// The minimal user repro: identifier ! declared first as 64 bits, then as
    /// 8 bits, with a 64 bit body value.  wellen keeps the last declaration and
    /// its deferred body decoder would panic (abort in release/wasm); opening
    /// must reject it as Corrupt instead.
    #[test]
    fn vcd_with_conflicting_widths_for_one_id_is_rejected_at_open() {
        // Byte-for-byte the 298 byte report: $dumpvars holds a short value and
        // the later change is 64 bits wide for the 8 bit second declaration.
        let vcd = format!(
            "$date minimal wellen width-mismatch repro $end\n             $version diag $end\n             $timescale 1ns $end\n             $scope module top $end\n             $var wire 64 ! big [63:0] $end\n             $var wire 8 ! small [7:0] $end\n             $upscope $end\n$enddefinitions $end\n\
             $dumpvars\nb0 !\n$end\n#10\nb{} !\n",
            "1".repeat(64)
        );
        let error = Waveform::open_bytes(vcd.into_bytes()).expect_err("must be rejected");
        let message = error.to_string();
        assert!(
            matches!(error, WaveformError::Corrupt { .. }),
            "expected Corrupt, got {error:?}"
        );
        assert!(
            message.contains("conflicting widths (64 and 8)"),
            "{message}"
        );
        assert!(message.contains("variable id !"), "{message}");
    }

    /// Same-width aliasing (several names for one VCD identifier) is legal and
    /// must keep opening and decoding.
    #[test]
    fn vcd_with_same_width_alias_still_opens_and_decodes() {
        let vcd = "$timescale 1ns $end\n$scope module top $end\n\
                   $var wire 8 ! first [7:0] $end\n\
                   $var wire 8 ! second [7:0] $end\n\
                   $upscope $end\n$enddefinitions $end\n#0\nb10101010 !\n#5\nb11110000 !\n";
        let waveform = Waveform::open_bytes(vcd.as_bytes().to_vec()).expect("alias VCD opens");
        assert_eq!(waveform.signal_count(), 2);
        assert_eq!(
            waveform.signal_by_path("top.first").expect("first").path(),
            "top.first"
        );
        assert_eq!(
            waveform
                .signal_by_path("top.second")
                .expect("second")
                .path(),
            "top.second"
        );
        let first = waveform.decoded_changes(0).expect("decode first");
        assert_eq!(first.len(), 2);
        assert_eq!(first.changes()[0].1.to_u64(), Some(0b1010_1010));
        let second = waveform.decoded_changes(1).expect("decode second");
        assert_eq!(second.len(), 2);
    }

    #[test]
    fn vcd_body_value_wider_than_the_declared_width_is_rejected_at_open() {
        let vcd = "$timescale 1ns $end\n$scope module top $end\n\
                   $var wire 8 ! small [7:0] $end\n\
                   $upscope $end\n$enddefinitions $end\n#0\nb1111111111111111 !\n";
        let error = Waveform::open_bytes(vcd.as_bytes().to_vec()).expect_err("must be rejected");
        let message = error.to_string();
        assert!(
            matches!(error, WaveformError::Corrupt { .. }),
            "expected Corrupt, got {error:?}"
        );
        assert!(message.contains("16 bits"), "{message}");
        assert!(message.contains("declared width is 8"), "{message}");
    }

    #[test]
    fn vcd_body_value_that_cannot_be_extended_is_rejected_at_open() {
        // A short all-h vector cannot be zero/x/z extended by wellen.
        let vcd = "$timescale 1ns $end\n$scope module top $end\n\
                   $var wire 8 ! small [7:0] $end\n\
                   $upscope $end\n$enddefinitions $end\n#0\nbhhh !\n";
        let error = Waveform::open_bytes(vcd.as_bytes().to_vec()).expect_err("must be rejected");
        assert!(
            matches!(error, WaveformError::Corrupt { .. }),
            "expected Corrupt, got {error:?}"
        );
        assert!(error.to_string().contains("can be extended"), "{error}");
    }

    #[test]
    fn well_formed_vcd_with_scalar_and_extended_values_still_opens() {
        let vcd = "$timescale 1ns $end\n$scope module top $end\n\
                   $var wire 1 ! clk $end\n\
                   $var wire 8 & data [7:0] $end\n\
                   $upscope $end\n$enddefinitions $end\n#0\n1!\nb1010 &\n#5\n0!\nbxxxxxxxx &\n";
        let waveform = Waveform::open_bytes(vcd.as_bytes().to_vec()).expect("valid VCD opens");
        let clk = waveform.decoded_changes(0).expect("decode clk");
        assert_eq!(clk.len(), 2);
        let data = waveform.decoded_changes(1).expect("decode data");
        assert_eq!(data.len(), 2);
    }

    #[test]
    fn memory_accounting_of_a_wide_hierarchy() {
        let (scopes, per_scope) = (100usize, 2000usize);
        let input = wide_vcd(scopes, per_scope);
        let input_bytes = input.len();
        let waveform = Waveform::open_bytes(input.into_bytes()).expect("wide VCD opens");
        let stats = waveform.memory_stats();
        eprintln!("WIDE_VCD input_bytes={input_bytes}");
        eprintln!("WIDE_VCD hierarchy={:?}", stats.hierarchy);
        eprintln!("WIDE_VCD stats={stats:?}");
        eprintln!(
            "WIDE_VCD hierarchy_bytes={} total_bytes={}",
            stats.hierarchy_bytes, stats.total_bytes
        );
        assert_eq!(stats.hierarchy.signals, scopes * per_scope);
        assert_eq!(waveform.signal_count(), scopes * per_scope);
        assert_eq!(stats.source_retained_bytes, input_bytes);
        assert!(stats.hierarchy_bytes > 0);
        assert_eq!(stats.cache_bytes, 0, "a header-only open has no cache yet");
        let signal = waveform
            .signal_by_path("top.s42.sig_17")
            .expect("path lookup works");
        assert_eq!(signal.name(), "sig_17");
        assert_eq!(signal.path(), "top.s42.sig_17");
        assert_eq!(signal.scope(), &["top".to_owned(), "s42".to_owned()]);

        // The regression this guards: 200k signals must not keep 200k full path
        // strings, duplicated scope vectors or 200k name allocations.  The same
        // 2000 local names are interned once, so there are only a few thousand
        // unique strings.
        assert!(
            stats.hierarchy.interned_strings < stats.hierarchy.signals / 50,
            "expected interned names, got {} unique strings for {} signals",
            stats.hierarchy.interned_strings,
            stats.hierarchy.signals
        );
        assert!(
            stats.hierarchy.path_bytes < 10_000,
            "full paths must be lazy, found {} path bytes",
            stats.hierarchy.path_bytes
        );
        assert!(
            stats.hierarchy.scope_component_bytes < 100_000,
            "scope components must be shared, found {} bytes",
            stats.hierarchy.scope_component_bytes
        );
        assert!(
            stats.hierarchy_bytes < 40_000_000,
            "hierarchy should stay well below the duplicated layout (~79 MB), got {}",
            stats.hierarchy_bytes
        );

        // Building a path on demand is accounted for, and the lazy cache does
        // not change the identity of the signal.
        let before = waveform.memory_stats().hierarchy.path_bytes;
        let _ = waveform.signal(0).expect("first signal").path();
        let after = waveform.memory_stats().hierarchy.path_bytes;
        assert!(after > before, "lazy path should add to path_bytes");
    }

    #[test]
    fn cache_accounting_and_unload_are_precise() {
        let vcd = "$timescale 1ns $end\n$scope module top $end\n$var wire 8 ! data $end\n$upscope $end\n$enddefinitions $end\n#0\nb00000000 !\n#5\nb11111111 !\n";
        let input_len = vcd.len();
        let waveform = Waveform::open_bytes(vcd.as_bytes().to_vec()).expect("small VCD opens");

        assert_eq!(waveform.cache_stats(), CacheStats::default());
        let memory = waveform.memory_stats();
        assert_eq!(memory.source_retained_bytes, input_len);
        assert!(memory.hierarchy_bytes > 0);
        assert_eq!(memory.cache_bytes, 0);
        assert_eq!(
            memory.total_bytes,
            memory.hierarchy_bytes + memory.source_retained_bytes
        );

        let decoded = waveform.decoded_changes(0).expect("decode the one signal");
        assert_eq!(decoded.len(), 2);
        drop(decoded);

        let stats = waveform.cache_stats();
        assert_eq!(stats.loaded_signals, 1);
        assert_eq!(stats.decoded_signals, 1);
        assert_eq!(stats.decoded_changes, 2);
        assert_eq!(stats.time_steps, 2);
        assert_eq!(
            stats.decoded_struct_bytes,
            2 * std::mem::size_of::<Change>()
        );
        assert_eq!(stats.decoded_bytes(), stats.decoded_struct_bytes);
        assert!(
            stats.decoded_payload_bytes >= 2 * 8,
            "two byte-vector values of 8 bits: {stats:?}"
        );
        assert!(stats.packed_bytes > 0);
        assert!(stats.time_table_bytes >= 2 * std::mem::size_of::<Time>());
        assert_eq!(
            stats.total_bytes(),
            stats.time_table_bytes
                + stats.packed_bytes
                + stats.decoded_struct_bytes
                + stats.decoded_payload_bytes
        );
        let memory = waveform.memory_stats();
        assert_eq!(memory.cache_bytes, stats.total_bytes());
        assert_eq!(memory.decoded_signals, 1);
        assert_eq!(memory.decoded_changes, 2);
        assert_eq!(memory.decoded_struct_bytes, stats.decoded_struct_bytes);
        assert_eq!(memory.decoded_payload_bytes, stats.decoded_payload_bytes);
        assert_eq!(
            memory.total_bytes,
            memory.hierarchy_bytes + memory.source_retained_bytes + memory.cache_bytes
        );

        // unload_signal frees everything it can: packed decode, decoded array and
        // its value payload.  The time table (part of the parsed body) stays.
        waveform.unload_signal(0).expect("unload signal");
        let after = waveform.cache_stats();
        assert_eq!(after.loaded_signals, 0);
        assert_eq!(after.decoded_signals, 0);
        assert_eq!(after.decoded_changes, 0);
        assert_eq!(after.packed_bytes, 0);
        assert_eq!(after.decoded_struct_bytes, 0);
        assert_eq!(after.decoded_payload_bytes, 0);
        assert_eq!(after.time_table_bytes, stats.time_table_bytes);
        assert!(
            after.total_bytes()
                + stats.packed_bytes
                + stats.decoded_struct_bytes
                + stats.decoded_payload_bytes
                >= stats.total_bytes(),
            "unloading must not grow the cache"
        );

        // unload_all_signals is the bulk variant.
        waveform.decoded_changes(0).expect("decode again");
        assert_eq!(waveform.cache_stats().decoded_signals, 1);
        waveform.unload_all_signals();
        let after = waveform.cache_stats();
        assert_eq!(after.loaded_signals, 0);
        assert_eq!(after.decoded_signals, 0);
        assert_eq!(after.packed_bytes, 0);
        assert!(after.time_steps > 0);

        // release_cache drops the body as well; only the hierarchy (and a memory
        // source) remain.
        waveform.release_cache();
        assert_eq!(waveform.cache_stats(), CacheStats::default());
        let memory = waveform.memory_stats();
        assert_eq!(memory.cache_bytes, 0);
        assert_eq!(memory.source_retained_bytes, input_len);
        assert!(memory.hierarchy_bytes > 0);
    }

    #[test]
    fn byte_source_trait_covers_memory_and_file_sources() {
        use std::io::{Read, Seek, SeekFrom};

        let bytes: std::sync::Arc<[u8]> = std::sync::Arc::from(&b"hello waveform"[..]);
        assert_eq!(bytes.byte_len(), Some(14));
        let mut reader = bytes.open().expect("memory reader");
        let mut buffer = [0u8; 5];
        reader.read_exact(&mut buffer).expect("read prefix");
        assert_eq!(&buffer, b"hello");
        reader.seek(SeekFrom::Start(0)).expect("seek");
        let mut first = [0u8; 1];
        reader.read_exact(&mut first).expect("read first byte");
        assert_eq!(&first, b"h");

        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("data")
            .join("io_mode_wave.fst");
        let length = path.byte_len().expect("file length");
        assert!(length > 0);
        let mut file_reader = path.open().expect("file reader");
        let mut magic = [0u8; 1];
        file_reader.read_exact(&mut magic).expect("read file");
        assert_eq!(magic[0], 0, "FST header block starts with block type 0");
    }

    #[test]
    fn memory_accounting_of_the_fst_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/io_mode_wave.fst");
        let waveform = Waveform::open_path(path).expect("FST opens");
        assert_eq!(
            waveform.memory_stats().source_retained_bytes,
            0,
            "a path source retains no bytes"
        );
        let decoded = waveform.decoded_changes(0).expect("decode first signal");
        eprintln!(
            "FST_FIXTURE decoded_changes={} memory={:?}",
            decoded.len(),
            waveform.memory_stats()
        );
        let memory = waveform.memory_stats();
        let stats = waveform.cache_stats();
        assert!(stats.loaded_signals >= 1);
        assert!(stats.decoded_changes == decoded.len());
        assert_eq!(memory.cache_bytes, stats.total_bytes());
        assert_eq!(
            memory.total_bytes,
            memory.hierarchy_bytes + memory.cache_bytes
        );
        assert!(stats.packed_bytes > 0);
        assert!(stats.time_table_bytes > 0);
    }

    // A real corrupt FST whose packed signal points past the body time table is
    // not something the public constructors can build (wellen derives both from
    // the same body today), so the guard is tested directly.  The old code
    // indexed the table and would have panicked in this situation.
    #[test]
    fn an_out_of_range_change_time_is_corrupt_not_a_panic() {
        assert_eq!(change_time(WaveFormat::Vcd, &[0, 10, 20], 2).unwrap(), 20);

        let error = change_time(WaveFormat::Fst, &[0], 3).unwrap_err();
        assert!(
            matches!(
                error,
                WaveformError::Corrupt {
                    format: WaveFormat::Fst,
                    operation: "decode value change",
                    ..
                }
            ),
            "unexpected error: {error:?}"
        );
        assert!(
            error.to_string().contains("outside the 1 time steps"),
            "{error}"
        );

        // An empty time table (a body with no time steps at all) is the same
        // error, not an index panic.
        let error = change_time(WaveFormat::Vcd, &[], 0).unwrap_err();
        assert!(
            matches!(error, WaveformError::Corrupt { .. }),
            "unexpected error: {error:?}"
        );
    }
}
