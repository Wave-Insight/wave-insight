//! Waveform backend: read VCD, FST and GHW through one API.
//!
//! This module is the replacement for the hand written VCD parser in
//! [`crate::parser`] (**that parser is legacy**: it only understands VCD, it
//! eagerly expands the whole dump, and it has no idea about real/string/event
//! values. Do not add features to it; port callers to this module instead).
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
//! * Value queries, three tiers (see [Choosing a value query](self#choosing-a-value-query)):
//!   * [`Waveform::decoded_changes`] / [`DecodedSignal`] - cached per signal
//!     history, the per frame path for a renderer,
//!   * [`Waveform::value_at`] - one time stamp, `O(log n)`, no history,
//!   * [`Waveform::changes_in_window`] / [`Waveform::stream_changes`] - streamed
//!     window, nothing kept, re-reads the body on every call.
//! * [`Waveform::legacy_module`] / [`Waveform::legacy_module_value`] /
//!   [`Waveform::to_legacy`] - an adapter that still produces the old
//!   [`Module`] + [`ModuleValue`] pair so the current front end keeps working
//!   unchanged. It expands everything (see below), so treat it as a bridge.
//!
//! # Choosing a value query
//!
//! | what you are doing | call | cost |
//! |---|---|---|
//! | drawing the visible rows | [`Waveform::decoded_changes_many`], then [`DecodedSignal::window`] per row | one pass over the body for the whole screen |
//! | drawing one row | [`Waveform::decoded_changes`] + [`DecodedSignal::window`] | decode once per signal, then binary search |
//! | cursor readout, tooltips | [`Waveform::value_at`] | one value, no history |
//! | one-off query over a huge signal | [`Waveform::changes_in_window`] | re-reads the body, `O(window)` memory |
//! | export / tests / small dumps | [`Waveform::all_changes`] | converts the history on every call |
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
//! over the body, so decoding rows one by one re-scans the file once per row. On
//! an 8 MB FST dump, 30 signals took 1.04 s sequentially and 73 ms batched.
//!
//! # Memory behaviour
//!
//! [`ModuleValue`] stores, per signal, a `Vec<i32>` of time stamps plus one
//! `(u8, u8)` pair per *eight* bits of every change, and it allocates a separate
//! `Vec` for each change. The payload looks small (2 bytes per 8 bits), but
//! every single change also pays a 24 byte `Vec` header and an allocator block:
//! a 1-bit clock with 100 000 toggles ends up at several MiB although its actual
//! payload is 200 KiB. On top of that the old [`crate::parser::vcd_parser`]
//! materialises the whole dump during load, so the cost is paid before the UI
//! can draw a first frame, and it is paid for *every* signal whether it is
//! displayed or not.
//!
//! The new backend does *not* do that:
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
//!    displayed signal on first use and [`Waveform::unload_signal`] frees it
//!    again; [`Waveform::cache_stats`] reports what is held. A signal that is
//!    too big to hold should be drawn with [`Waveform::changes_in_window`]
//!    instead.
//! 5. [`Waveform::legacy_module_value`] materialises **everything** by design:
//!    it is the compatibility path for the current UI and is only suitable for
//!    small dumps. For VCD the body also has to be parsed into wellen's in
//!    memory representation first, which is what makes it the slowest path.
//!
//! # Migrating the front end
//!
//! The old flow is `vcd_parser(text, &mut Module::new()) -> (Module,
//! ModuleValue)`, then the UI walks `Module::sub_module` / `Module::signal` and
//! calls `ModuleValue::get_bool(&signal.value_key)` /
//! `get_bits(&signal.value_key)`, where `value_key` is the VCD identifier code
//! (`!`, `"`, ...).
//!
//! The new flow, step by step:
//!
//! 1. Open once when a file is selected:
//!    ```no_run
//!    # use wave_insight_lib::waveform::{Waveform, WaveValue};
//!    # fn open() -> Result<(), Box<dyn std::error::Error>> {
//!    let waveform = Waveform::open_path("dump.fst")?;
//!    for signal in waveform.signals() {
//!        println!("{} ({:?}, {} bit, {:?})", signal.path(), signal.kind(), signal.width(), signal.index_range());
//!    }
//!    # Ok(())
//!    # }
//!    ```
//! 2. Replace the tree of [`Module`]s with [`HierarchyView::root`] and walk it
//!    with [`ScopeNode::children`] / [`ScopeNode::signals`]; the signal data is
//!    [`HierarchyView::signal`]. Every row of the wave view then holds a
//!    `signal.index()` instead of a `value_key` string.
//! 3. Draw from the lazy path - one call per visible window, only for the
//!    signals that are on screen:
//!    ```no_run
//!    # use wave_insight_lib::waveform::{Waveform, WaveValue, ValueQuery};
//!    # fn draw(waveform: &Waveform, signal: usize) -> Result<(), Box<dyn std::error::Error>> {
//!    for (time, value) in waveform.changes_in_window(signal, 1_000, Some(2_000))? {
//!        let text = value.to_bit_string().unwrap_or_default();
//!        println!("{time}: {text}");
//!    }
//!    // or fully streaming, without a `Vec` in between:
//!    waveform.stream_changes(&ValueQuery::for_signal(signal), |signal, time, value| {
//!        let _ = (signal, time, value);
//!        true // `false` stops early
//!    })?;
//!    # Ok(())
//!    # }
//!    ```
//!    A change list alone cannot draw a window: the signal may not change
//!    inside it. Use [`Waveform::value_at`] to get the value in effect at the
//!    window start, then paint from there.
//! 4. The cursor/table view wants the value at the cursor time:
//!    `waveform.value_at(signal.index(), cursor_time)?` ->
//!    `Option<WaveValue>` (values render with [`Bits::bit_string`] or
//!    convert with [`Bits::as_u64`]). While the UI still uses the old
//!    `BitsData`/`BoolData` renderers, [`WaveValue::to_bits_data`] and
//!    [`WaveValue::to_bool_data`] hand the value over unchanged
//!    (`to_bits_data().to_string(width, &ShowType::Hex)` keeps working).
//! 5. Delete the `GetValue`/`WsSend` round trips and the `ModuleValue` inside
//!    the `Rc<RefCell<_>>`: value data no longer travels as a whole-trace blob,
//!    it is queried from `&Waveform` (which is `Send + Sync`, so it can live in
//!    a `tauri::State`).
//!
//! While the UI is not migrated, step 1 plus
//! `let (module, values) = waveform.to_legacy()?;` is a drop-in replacement:
//! the tree keeps its shape, `end_clock` is set on the root module only (like
//! the old parser did) and `get_bool`/`get_bits` keep working. Two things
//! change: `value_key` is now the full hierarchical path (the VCD identifier
//! code is not preserved by wellen), and time stamps are clamped to `i32`
//! because [`ModuleValue`] stores `i32`.
//!
//! # Limitations
//!
//! * GHW can be opened and queried eagerly, but wellen cannot stream GHW;
//!   [`Waveform::stream_changes`] falls back to materialising the requested
//!   signals for GHW (correct, but slow and memory hungry).
//! * The legacy adapter skips [`SignalKind::Real`], [`SignalKind::String`] and
//!   [`SignalKind::Event`] variables: the old model has no representation for
//!   them.
//! * wellen may elide a change whose value repeats the previous one; the old
//!   parser recorded all of them. Do not rely on "one change per VCD line".
//! * `wellen`/`fst-reader` contain `assert!`/`todo!()` on input they consider
//!   impossible, so a hand crafted or half written file can reach one.
//!   [`Waveform`] catches those panics and reports [`WaveformError::Corrupt`]
//!   instead (the default panic hook still prints the message to stderr); a
//!   panic raised by your own [`Waveform::stream_changes`] visitor is re-raised,
//!   not swallowed.

mod decoded;
mod error;
mod format;
mod hierarchy;
mod legacy;
mod query;
mod source;
mod value;

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, Mutex};

use wellen::{LoadOptions, Signal, SignalRef, SignalSource, Time};

use crate::data_struct::{Module, ModuleValue};

pub use decoded::{Change, DecodedSignal};
pub use error::{Result, WaveformError};
pub use format::{TimeUnit, Timescale, WaveFormat};
pub use hierarchy::{HierarchyView, ScopeNode, SignalInfo, SignalKind};
pub use query::ValueQuery;
pub use source::WaveformSource;
pub use value::{BitValue, Bits, WaveValue};

use error::catch_parse;
use source::detect_format;

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

/// An opened waveform file.
///
/// Cheap to open (header only), `Send + Sync`, and all value queries take
/// `&self`, so it can be shared behind `&` or a mutex/`tauri::State`.
///
/// See the [module documentation](self) for the memory model and the migration
/// path away from the legacy [`Module`]/[`ModuleValue`] model.
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
}

impl fmt::Debug for Waveform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Waveform")
            .field("format", &self.format)
            .field("source", &self.source)
            .field("signals", &self.hierarchy.len())
            .field("timescale", &self.timescale)
            .field("body_loaded", &self.body.lock().map(|b| b.is_some()).unwrap_or(false))
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

        let options = load_options();
        let header = catch_parse(detected, "read header", || {
            Ok(wellen::viewers::read_header(reader, &options)?)
        })?;
        let format = WaveFormat::from_wellen(header.file_format).ok_or(WaveformError::UnknownFormat)?;
        let hierarchy = HierarchyView::build(&header.hierarchy);
        let date = header.hierarchy.date().to_string();
        let version = header.hierarchy.version().to_string();
        let timescale = header.hierarchy.timescale().and_then(Timescale::from_wellen);
        Ok(Self {
            source,
            format,
            hierarchy,
            date,
            version,
            timescale,
            body: Mutex::new(None),
        })
    }

    /// Where the data came from.
    pub fn source(&self) -> &WaveformSource {
        &self.source
    }

    /// Path of the file, if it was opened from disk.
    pub fn path(&self) -> Option<&Path> {
        self.source.path()
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

    /// Last time stamp that has any change in the file.
    ///
    /// Needs the time table, which for VCD means scanning the body once.
    pub fn end_time(&self) -> Result<Option<Time>> {
        let mut guard = self.lock_body();
        let body = self.ensure_body(&mut guard)?;
        Ok(body.time_table.last().copied())
    }

    /// First time stamp that has any change in the file.
    ///
    /// Needs the time table, which for VCD means scanning the body once.
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

        let time_table = &body.time_table;
        let packed = body
            .packed(signal_ref)
            .ok_or_else(|| WaveformError::Backend(format!("signal `{}` did not decode", self.path_of(signal))))?;
        Ok(packed
            .iter_changes()
            .map(|(time_index, value)| {
                (
                    time_table[time_index as usize],
                    WaveValue::from_wellen(value),
                )
            })
            .collect())
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
                    let Some(variable) = self.hierarchy.variable_of_signal(signal_ref) else {
                        //a signal without a variable; aliases are mapped to their
                        //first variable, so this only affects internal signals
                        return Ok(());
                    };
                    //belt and braces: some backends only use the filter as a hint
                    if !query.includes_time(time) {
                        return Ok(());
                    }
                    emitted += 1;
                    in_visitor.set(true);
                    let keep_going = visit(variable, time, WaveValue::from_wellen(value));
                    in_visitor.set(false);
                    if !keep_going || limit.map(|limit| emitted >= limit).unwrap_or(false) {
                        return Err(());
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

    /// Build the legacy [`Module`] tree of the whole design.
    ///
    /// Only the hierarchy is converted; `value_key` is set to the full
    /// hierarchical path and `size` to the declared bit width. `end_clock`
    /// needs the time table, so this reads the body (for VCD: the whole file).
    pub fn legacy_module(&self) -> Result<Module> {
        let end_clock = self.end_time()?.map(time_to_i32).unwrap_or(0);
        Ok(legacy::build_module(&self.hierarchy, end_clock))
    }

    /// Build the legacy [`ModuleValue`] for the whole trace.
    ///
    /// **This materialises every change of every bit vector signal** in the old
    /// `(u8, u8)` layout, which is what used to make big dumps explode. It
    /// exists so the current UI keeps working; prefer
    /// [`Waveform::changes_in_window`] + [`Waveform::value_at`] for new code.
    ///
    /// Keys are [`SignalInfo::path`] values (the old VCD identifier codes are
    /// not available through wellen). Real, string and event variables are
    /// skipped because the old model cannot represent them.
    pub fn legacy_module_value(&self) -> Result<ModuleValue> {
        legacy::build_module_value(self)
    }

    /// [`Waveform::legacy_module`] plus [`Waveform::legacy_module_value`].
    pub fn to_legacy(&self) -> Result<(Module, ModuleValue)> {
        Ok((self.legacy_module()?, self.legacy_module_value()?))
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
                if query.max_changes.map(|limit| emitted >= limit).unwrap_or(false) {
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

/// Old `Module::end_clock` and `ModuleValue` time stamps are `i32`.
pub(crate) fn time_to_i32(time: Time) -> i32 {
    time.min(i32::MAX as u64) as i32
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
}

impl CacheStats {
    /// Bytes held by the decoded histories, excluding bus value payloads.
    ///
    /// This is `decoded_changes * size_of::<Change>()`; each bus value also owns
    /// its own allocation, and the packed decodes are not counted at all.
    pub fn decoded_bytes(&self) -> usize {
        self.decoded_changes * std::mem::size_of::<Change>()
    }
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
        let time_table = &self.time_table;
        let cached = self.signals.get_mut(&signal).ok_or_else(|| {
            WaveformError::Backend(format!("signal #{} is not cached", signal.index()))
        })?;
        if cached.decoded.is_none() {
            let changes: Arc<[Change]> = cached
                .packed
                .iter_changes()
                .map(|(time_index, value)| {
                    (
                        time_table[time_index as usize],
                        WaveValue::from_wellen(value),
                    )
                })
                .collect::<Vec<Change>>()
                .into();
            cached.decoded = Some(DecodedSignal::new(changes));
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
            ..CacheStats::default()
        };
        for cached in self.signals.values() {
            stats.loaded_signals += 1;
            if let Some(decoded) = &cached.decoded {
                stats.decoded_signals += 1;
                stats.decoded_changes += decoded.len();
            }
        }
        stats
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

/// Sanity check that the shared waveform handle can be put into a Tauri state.
#[allow(dead_code)]
fn assert_send_sync() {
    fn is_send_sync<T: Send + Sync>() {}
    is_send_sync::<Waveform>();
}
