//! Native dump backend for the desktop build.
//!
//! The desktop app must open very large dumps without copying the file into the
//! webview's wasm memory. This module keeps a registry of files opened natively
//! (memory mapped when the platform allows it) and answers windowed queries
//! from the front end over IPC. The webview only ever receives rendered text
//! changes for the window it is drawing; the file bytes never cross IPC.
//!
//! # IPC protocol sketch (what the front-end owner consumes next wave)
//!
//! Tauri command arguments keep Tauri's default camelCase naming; responses are
//! the serde DTOs below (snake_case fields, u64 ids and times).
//!
//! open_dump(path) -> DumpSummary
//!   { id, name, path, bytes, format: "FST"|"VCD"|"GHW",
//!     time_range: { start, end } | null,
//!     signal_count, scope_count, retained_bytes, mapped_bytes, open_ms }
//!   errors: "unknown waveform format (...)", "<path>: io error: ...",
//!           "too many dumps open (32), close one first"
//!
//! dump_hierarchy(id, offset, limit) -> HierarchyPage
//!   { total, offset, rows: [ { kind: "scope"|"signal", depth, path, name,
//!                             signal?, width?, range?, signal_kind?, var_type? } ] }
//!   rows are a pre-order flattening: a scope, its signals, then its child
//!   scopes. total is the flattened row count.
//!
//! dump_window(id, signals, start, end, maxChangesPerSignal?, show?) -> [WindowDto]
//!   { signal, before: String|null, capped, changes: [ { time, value } ] }
//!   values are already rendered to text in Rust; capped means the per-signal
//!   cap stopped the collection; before is the value in effect at start when the
//!   backend has it cached (null on a cold cache).
//!
//! dump_value_at(id, signal, time, show?) -> String | null
//!   null = the signal had not changed yet.
//!
//! dump_values_at(id, signals: [usize], time, show?) -> [ { signal, time, value } | null ]
//!   One batch pass for the visible rows (Waveform::values_before): one entry
//!   per requested signal in request order (duplicates repeated), null when the
//!   signal had no change at or before time. Same rendered text/cache rules as
//!   dump_value_at; one FST section read per redraw instead of one per row.
//!
//! close_dump(id) -> null
//!   errors for an unknown id.
//!
//! Windows caveat: the mapping keeps the file handle alive for the lifetime of
//! the open dump, so deleting/renaming/replacing the dump while it is open can
//! fail; close_dump drops the mapping (and open_dump falls back to the read
//! path when a map cannot be created).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use wave_insight_lib::waveform::{
    HierarchyView, ScopeNode, ShowType, SignalInfo, SignalKind, TimeRange, ValueQuery, WaveValue,
    Waveform, WaveformError,
};

/// How many dumps one desktop session may keep open at once.
pub const MAX_OPEN_DUMPS: usize = 32;
/// Upper bound for one dump_hierarchy page.
pub const MAX_HIERARCHY_PAGE: usize = 4096;
/// Default per-signal change cap for dump_window.
pub const DEFAULT_MAX_CHANGES_PER_SIGNAL: usize = 4096;
/// Hard upper bound for the per-signal cap.
pub const MAX_MAX_CHANGES_PER_SIGNAL: usize = 1 << 20;

/// Stable handle of one open dump. Never reused within a session.
pub type DumpId = u64;

/// Inclusive time range of a trace, in the file's time units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRangeDto {
    pub start: u64,
    pub end: u64,
}

impl From<TimeRange> for TimeRangeDto {
    fn from(range: TimeRange) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

/// What open_dump returns and what a dump tab needs to render.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DumpSummary {
    pub id: DumpId,
    /// File name (no directory), for the tab strip.
    pub name: String,
    /// Absolute path as passed to open_dump.
    pub path: String,
    /// File size in bytes (the on-disk size, not what is resident).
    pub bytes: u64,
    /// "VCD", "FST" or "GHW".
    pub format: String,
    /// None when the format cannot report it cheaply (GHW).
    pub time_range: Option<TimeRangeDto>,
    pub signal_count: usize,
    pub scope_count: usize,
    /// Heap bytes of the file content kept by the backend (0 for mmap/file).
    pub retained_bytes: usize,
    /// Bytes memory mapped by this dump (0 when the read fallback was used).
    pub mapped_bytes: usize,
    /// Time spent opening the file (header + hierarchy), in milliseconds.
    pub open_ms: f64,
}

/// One row of a dump_hierarchy page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HierarchyRow {
    /// "scope" or "signal".
    pub kind: String,
    /// Nesting depth, 0 for a top level scope.
    pub depth: u32,
    /// Full dotted path (top.u_cpu.pc); the scope path for scope rows.
    pub path: String,
    /// Local name inside the parent scope.
    pub name: String,
    /// Signal index for value queries (signal rows only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<usize>,
    /// Bit width (bit vector signals only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Declared index range, e.g. [7:0].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
    /// "bit_vector", "event", "real" or "string".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal_kind: Option<String>,
    /// wellen variable type, e.g. "Wire", "Logic", "Parameter".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub var_type: Option<String>,
}

/// One page of the flattened hierarchy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HierarchyPage {
    /// Flattened row count of the whole dump.
    pub total: usize,
    pub offset: usize,
    pub rows: Vec<HierarchyRow>,
}

/// One value change, already rendered to text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChangeDto {
    pub time: u64,
    pub value: String,
}

/// The bounded window of one signal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowDto {
    pub signal: usize,
    /// Value in effect at the window start, when the backend had it cached.
    pub before: Option<String>,
    /// The per-signal cap stopped the collection.
    pub capped: bool,
    pub changes: Vec<ChangeDto>,
}

/// One match of a streaming value search (dump_find).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FoundValue {
    /// Time stamp of the matching change.
    pub time: u64,
    /// The matching change's value, rendered with the requested ShowType.
    pub value: String,
}

/// One row of a batch value readout (dump_values_at).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValueAtDto {
    /// The requested signal index.
    pub signal: usize,
    /// Time of the change the value comes from (<= the requested time).
    pub time: u64,
    /// The change's value, rendered with the requested ShowType.
    pub value: String,
}

/// Per-dump memory accounting for diagnostics (dump_memory_stats).
///
/// The first block is the registry's view (file size, mapped/retained source
/// bytes); the rest is the library's `MemoryStats` for the open waveform. The
/// body counters (time_table_bytes, packed_*, decoded_*, cache_bytes) are the
/// ones that must stay 0 on the native streaming path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DumpMemoryStats {
    pub id: DumpId,
    /// File name (no directory).
    pub name: String,
    /// File size on disk.
    pub bytes: u64,
    /// Heap bytes of the file content kept by the backend (0 for mmap).
    pub retained_bytes: usize,
    /// Bytes mapped into the process (0 when the read fallback was used).
    pub mapped_bytes: usize,
    /// Library: source bytes alive on the application heap.
    pub source_retained_bytes: usize,
    /// Library: memory-mapped source length.
    pub source_mapped_bytes: usize,
    /// Library: hierarchy accounting, flat sum.
    pub hierarchy_bytes: usize,
    /// Library: signals decoded into wellen's packed representation.
    pub packed_signals: usize,
    /// Library: bytes of those packed decodes.
    pub packed_bytes: usize,
    /// Library: signals with a decoded history.
    pub decoded_signals: usize,
    /// Library: changes in all decoded histories.
    pub decoded_changes: usize,
    /// Library: bytes of the decoded change arrays.
    pub decoded_struct_bytes: usize,
    /// Library: heap payload of the decoded values.
    pub decoded_payload_bytes: usize,
    /// Library: bytes of the body time table.
    pub time_table_bytes: usize,
    /// Library: entries of the stream_value_before lookup cache.
    pub lookup_cache_entries: usize,
    /// Library: heap bytes of that lookup cache.
    pub lookup_cache_bytes: usize,
    /// Library: random access cache bytes (time table + packed + decoded).
    pub cache_bytes: usize,
    /// Library: hierarchy + retained source + cache.
    pub total_bytes: usize,
}

/// A dump opened natively. Only the hierarchy is parsed; values are streamed on
/// demand, so the retained heap cost is the hierarchy, not the file.
struct NativeDump {
    name: String,
    bytes: u64,
    /// Pre-flattened hierarchy, so a page is a slice instead of a tree walk.
    flat: Vec<FlatEntry>,
    /// Scope path/name pairs referenced by the scope rows.
    scopes: Vec<(String, String)>,
    /// Last stream_value_before answer per signal (dump_value_at cache).
    last_lookup: Mutex<HashMap<usize, CachedLookup>>,
    waveform: Waveform,
}

/// One cached stream_value_before answer (per dump, per signal).
#[derive(Clone, Debug)]
struct CachedLookup {
    /// Time of the last change at (or before) the cached requested time.
    change_time: u64,
    /// The value in effect from `change_time` on.
    value: WaveValue,
    /// The requested time that produced this entry.
    requested_time: u64,
}

/// Upper bound of the per-dump last-lookup cache (one entry per signal).
const LOOKUP_CACHE_SIGNALS: usize = 256;

#[derive(Clone, Copy, Debug)]
enum FlatEntry {
    /// scope indexes NativeDump::scopes.
    Scope {
        depth: u32,
        scope: usize,
    },
    Signal {
        depth: u32,
        signal: usize,
    },
}

impl NativeDump {
    fn hierarchy_row(&self, entry: FlatEntry) -> HierarchyRow {
        match entry {
            FlatEntry::Scope { depth, scope } => {
                let (path, name) = &self.scopes[scope];
                HierarchyRow {
                    kind: "scope".to_string(),
                    depth,
                    path: path.clone(),
                    name: name.clone(),
                    signal: None,
                    width: None,
                    range: None,
                    signal_kind: None,
                    var_type: None,
                }
            }
            FlatEntry::Signal { depth, signal } => match self.waveform.signal(signal) {
                Some(info) => hierarchy_signal_row(depth, info),
                // cannot happen: the flat map was built from the hierarchy
                None => HierarchyRow {
                    kind: "signal".to_string(),
                    depth,
                    path: format!("#{signal}"),
                    name: format!("#{signal}"),
                    signal: Some(signal),
                    width: None,
                    range: None,
                    signal_kind: None,
                    var_type: None,
                },
            },
        }
    }
}

fn hierarchy_signal_row(depth: u32, info: &SignalInfo) -> HierarchyRow {
    HierarchyRow {
        kind: "signal".to_string(),
        depth,
        path: info.path().to_string(),
        name: info.name().to_string(),
        signal: Some(info.index()),
        width: info.is_bit_vector().then(|| info.width()),
        range: info.range_string(),
        signal_kind: Some(signal_kind_name(info.kind()).to_string()),
        var_type: Some(info.var_type().to_string()),
    }
}

fn signal_kind_name(kind: SignalKind) -> &'static str {
    match kind {
        SignalKind::BitVector => "bit_vector",
        SignalKind::Event => "event",
        SignalKind::Real => "real",
        SignalKind::String => "string",
    }
}

#[derive(Default)]
struct RegistryInner {
    next_id: DumpId,
    dumps: HashMap<DumpId, Arc<NativeDump>>,
}

/// The native dump registry, managed by Tauri as application state.
#[derive(Default)]
pub struct DumpRegistry {
    inner: Mutex<RegistryInner>,
}

impl DumpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, RegistryInner> {
        // a panic while holding the registry lock must not take the whole
        // backend down for the rest of the session
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Number of open dumps (tests and diagnostics).
    pub fn open_count(&self) -> usize {
        self.lock().dumps.len()
    }

    /// Open a dump natively and return what a tab needs to render.
    pub fn open(&self, path: &str) -> Result<DumpSummary, String> {
        if path.trim().is_empty() {
            return Err("open_dump: the path is empty".to_string());
        }
        let path = PathBuf::from(path);
        if self.open_count() >= MAX_OPEN_DUMPS {
            return Err(format!(
                "too many dumps open ({MAX_OPEN_DUMPS}), close one first"
            ));
        }
        let metadata = std::fs::metadata(&path)
            .map_err(|error| format!("open_dump: {}: {error}", path.display()))?;
        if !metadata.is_file() {
            return Err(format!("open_dump: {} is not a file", path.display()));
        }

        let started = Instant::now();
        let waveform = open_waveform(&path)?;
        let (flat, scopes) = flatten_hierarchy(waveform.hierarchy());
        let stats = waveform.memory_stats();
        let time_range = match waveform.time_range_hint() {
            Ok(hint) => hint,
            Err(error) => {
                return Err(format!(
                    "open_dump: {}: {}",
                    path.display(),
                    error.message()
                ));
            }
        };
        let time_range = time_range.or_else(|| {
            // GHW cannot answer the hint cheaply; the end of the trace is
            // enough for a tab, and the ruler refines it on the first draw
            let start = waveform.start_time().ok().flatten()?;
            let end = waveform.end_time().ok().flatten()?;
            Some(TimeRange { start, end })
        });
        let open_ms = started.elapsed().as_secs_f64() * 1000.0;

        let mut inner = self.lock();
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1);
        let summary = DumpSummary {
            id,
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            path: path.display().to_string(),
            bytes: metadata.len(),
            format: waveform.file_format().name().to_string(),
            time_range: time_range.map(TimeRangeDto::from),
            signal_count: waveform.signal_count(),
            scope_count: scopes.len(),
            retained_bytes: stats.source_retained_bytes,
            mapped_bytes: stats.source_mapped_bytes,
            open_ms,
        };
        let dump = Arc::new(NativeDump {
            name: summary.name.clone(),
            bytes: summary.bytes,
            flat,
            scopes,
            last_lookup: Mutex::new(HashMap::new()),
            waveform,
        });
        inner.dumps.insert(id, dump);
        Ok(summary)
    }

    /// One page of the flattened hierarchy.
    pub fn hierarchy(
        &self,
        id: DumpId,
        offset: usize,
        limit: usize,
    ) -> Result<HierarchyPage, String> {
        let dump = self.dump(id)?;
        let limit = limit.min(MAX_HIERARCHY_PAGE);
        let total = dump.flat.len();
        let offset = offset.min(total);
        let end = offset.saturating_add(limit).min(total);
        let rows = dump.flat[offset..end]
            .iter()
            .copied()
            .map(|entry| dump.hierarchy_row(entry))
            .collect();
        Ok(HierarchyPage {
            total,
            offset,
            rows,
        })
    }

    /// The bounded value window of several signals, rendered to text.
    pub fn window(
        &self,
        id: DumpId,
        signals: &[usize],
        start: u64,
        end: Option<u64>,
        max_changes_per_signal: Option<usize>,
        show: Option<&str>,
    ) -> Result<Vec<WindowDto>, String> {
        let dump = self.dump(id)?;
        let show = parse_show_type(show)?;
        let max_changes = max_changes_per_signal
            .unwrap_or(DEFAULT_MAX_CHANGES_PER_SIGNAL)
            .min(MAX_MAX_CHANGES_PER_SIGNAL);
        // the library collapses duplicate requests and keeps the first slot, so
        // rebuild the same order to map the answers back to signal indices
        let mut indices: Vec<usize> = Vec::with_capacity(signals.len());
        for &signal in signals {
            if !indices.contains(&signal) {
                indices.push(signal);
            }
        }
        let windowed = dump
            .waveform
            .window_changes_many(&indices, start, end, max_changes)
            .map_err(|error| format!("dump_window: dump {id}: {}", error.message()))?;
        Ok(windowed
            .into_iter()
            .zip(indices)
            .map(|(window, signal)| {
                let width = dump
                    .waveform
                    .signal(signal)
                    .map(|info| info.width())
                    .unwrap_or(0);
                WindowDto {
                    signal,
                    before: window
                        .before
                        .as_ref()
                        .map(|value| render_value(value, width, &show)),
                    capped: window.capped,
                    changes: window
                        .changes
                        .iter()
                        .map(|(time, value)| ChangeDto {
                            time: *time,
                            value: render_value(value, width, &show),
                        })
                        .collect(),
                }
            })
            .collect())
    }

    /// Find the first change of `signal` whose rendered value matches `query`.
    ///
    /// The search streams the signal with the library's ValueQuery filter, so
    /// memory stays O(1) and it stops at the first match. Semantics:
    ///
    /// * forwards (backwards == false): the first matching change with
    ///   time > from (time >= 0 when from == 0);
    /// * backwards (backwards == true): the last matching change with
    ///   time < from (the end of the trace when from == u64::MAX);
    /// * the value is rendered with `show` (the same rendering as dump_window)
    ///   and compared case-insensitively;
    /// * a query that starts with 0x/0b/0o is matched in that radix (letter
    ///   case and leading zeros are ignored), independent of `show`.
    ///
    /// Worst case is a full trace scan (a value that never matches; or a
    /// backwards search, because the stream is forward-only and has to scan
    /// from the start up to `from`). The command cannot be cancelled, so the
    /// front end should search one signal at a time.
    pub fn find(
        &self,
        id: DumpId,
        signal: usize,
        query: &str,
        from: u64,
        backwards: bool,
        show: &ShowType,
    ) -> Result<Option<FoundValue>, String> {
        let dump = self.dump(id)?;
        let matcher = FindMatcher::parse(query)?;
        let width = dump
            .waveform
            .signal(signal)
            .map(|info| info.width())
            .unwrap_or(0);
        if backwards && from == 0 {
            // nothing is strictly before time 0
            return Ok(None);
        }
        let (start, end) = if backwards {
            let end = if from == u64::MAX {
                None
            } else {
                Some(from.saturating_sub(1))
            };
            (0u64, end)
        } else {
            (if from == 0 { 0 } else { from.saturating_add(1) }, None)
        };
        let window = ValueQuery::window(signal, start, end);
        let mut found: Option<FoundValue> = None;
        dump.waveform
            .stream_changes(&window, |_signal, time, value| {
                let rendered = render_value(&value, width, show);
                if !matcher.matches(&rendered, width, show, &value) {
                    return true;
                }
                found = Some(FoundValue {
                    time,
                    value: rendered,
                });
                // forwards: stop at the first match; backwards: keep the last
                backwards
            })
            .map_err(|error| format!("dump_find: dump {id}: {}", error.message()))?;
        Ok(found)
    }

    /// The rendered value of one signal at time.
    ///
    /// This is the memory-flat native path: it never calls `Waveform::value_at`
    /// (which materialises the random-access body - for FST that is the 8 B per
    /// time-step time table plus packed decodes, the multi-GB peak old builds
    /// showed). It uses `Waveform::stream_value_before` (O(1) retained memory)
    /// plus a per-dump, per-signal last-lookup cache:
    ///
    /// * the same (signal, time) again is answered from the cache;
    /// * a later time on the same signal streams only from the cached change
    ///   time forward (FST seeks to that section; VCD still scans from the start
    ///   internally because the format cannot seek - documented worst case);
    /// * a backwards jump calls `stream_value_before` again and may scan (FST
    ///   uses growing backward windows; GHW falls back to materialising the
    ///   requested signal inside the library, as documented there).
    ///
    /// The rendered text is identical to the ShowType formatting used by
    /// `dump_window`.
    pub fn value_at(
        &self,
        id: DumpId,
        signal: usize,
        time: u64,
        show: Option<&str>,
    ) -> Result<Option<String>, String> {
        let dump = self.dump(id)?;
        let show = parse_show_type(show)?;
        let width = dump
            .waveform
            .signal(signal)
            .map(|info| info.width())
            .unwrap_or(0);
        let cached = {
            let cache = dump
                .last_lookup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            cache.get(&signal).cloned()
        };
        if let Some(entry) = &cached {
            if entry.requested_time == time || entry.change_time == time {
                return Ok(Some(render_value(&entry.value, width, &show)));
            }
        }
        let found: Option<(u64, WaveValue)> = match &cached {
            Some(entry) if time > entry.change_time => {
                // forward walk: only what changed after the cached change matters
                let window = ValueQuery::window(signal, entry.change_time, Some(time));
                let mut last: Option<(u64, WaveValue)> = None;
                dump.waveform
                    .stream_changes(&window, |_signal, change_time, value| {
                        last = Some((change_time, value));
                        true
                    })
                    .map_err(|error| format!("dump_value_at: dump {id}: {}", error.message()))?;
                // nothing changed since the cached change: the cached value holds
                last.or_else(|| Some((entry.change_time, entry.value.clone())))
            }
            _ => dump
                .waveform
                .stream_value_before(signal, time)
                .map_err(|error| format!("dump_value_at: dump {id}: {}", error.message()))?,
        };
        let Some((change_time, value)) = found else {
            return Ok(None);
        };
        let rendered = render_value(&value, width, &show);
        {
            let mut cache = dump
                .last_lookup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if cache.len() >= LOOKUP_CACHE_SIGNALS && !cache.contains_key(&signal) {
                cache.clear();
            }
            cache.insert(
                signal,
                CachedLookup {
                    change_time,
                    value,
                    requested_time: time,
                },
            );
        }
        Ok(Some(rendered))
    }

    /// The rendered values of several signals at one time, in ONE batch pass.
    ///
    /// This is the per-redraw call for the native renderer: it uses
    /// `Waveform::values_before`, so the batch performs one stream pass (one FST
    /// section time-table read for the whole visible row set instead of one per
    /// row; measured 6-7x faster than per-signal lookups on a 20M-step FST).
    /// Retained memory stays O(1): no body, no time table, no packed decodes.
    ///
    /// The answer has one entry per requested signal in request order (a
    /// duplicate request repeats its answer), and None when that signal had no
    /// change at or before `time`. Rendering and cache semantics are identical
    /// to `value_at` (the answers also refresh the per-signal last-lookup
    /// cache, so a following single `value_at` is free).
    pub fn values_at(
        &self,
        id: DumpId,
        signals: &[usize],
        time: u64,
        show: Option<&str>,
    ) -> Result<Vec<Option<ValueAtDto>>, String> {
        let dump = self.dump(id)?;
        let show = parse_show_type(show)?;
        // the library collapses duplicate requests and keeps request order;
        // remember the slot of every request so the answer keeps them
        let mut unique: Vec<usize> = Vec::with_capacity(signals.len());
        let mut slots: Vec<(usize, usize)> = Vec::with_capacity(signals.len());
        for (request, &signal) in signals.iter().enumerate() {
            let slot = match unique.iter().position(|&existing| existing == signal) {
                Some(slot) => slot,
                None => {
                    unique.push(signal);
                    unique.len() - 1
                }
            };
            slots.push((request, slot));
        }
        let answers = dump
            .waveform
            .values_before(&unique, time)
            .map_err(|error| format!("dump_values_at: dump {id}: {}", error.message()))?;
        let mut rendered: Vec<Option<ValueAtDto>> = (0..signals.len()).map(|_| None).collect();
        let mut cache = dump
            .last_lookup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (slot, answer) in answers.into_iter().enumerate() {
            let Some((change_time, value)) = answer else {
                continue;
            };
            let signal = unique[slot];
            let width = dump
                .waveform
                .signal(signal)
                .map(|info| info.width())
                .unwrap_or(0);
            let text = render_value(&value, width, &show);
            if cache.len() >= LOOKUP_CACHE_SIGNALS && !cache.contains_key(&signal) {
                cache.clear();
            }
            cache.insert(
                signal,
                CachedLookup {
                    change_time,
                    value: value.clone(),
                    requested_time: time,
                },
            );
            for &(request, request_slot) in &slots {
                if request_slot == slot {
                    rendered[request] = Some(ValueAtDto {
                        signal,
                        time: change_time,
                        value: text.clone(),
                    });
                }
            }
        }
        Ok(rendered)
    }

    /// Close one dump, freeing its mapping and hierarchy.
    pub fn close(&self, id: DumpId) -> Result<(), String> {
        match self.lock().dumps.remove(&id) {
            Some(_) => Ok(()),
            None => Err(format!("close_dump: unknown dump id {id}")),
        }
    }

    /// Per-dump memory accounting for diagnostics.
    ///
    /// Pure bookkeeping: it reads the library's cached counters, so it never
    /// materialises the body and is safe to poll from the front end.
    pub fn memory_stats(&self, id: DumpId) -> Result<DumpMemoryStats, String> {
        let dump = self.dump(id)?;
        let stats = dump.waveform.memory_stats();
        Ok(DumpMemoryStats {
            id,
            name: dump.name.clone(),
            bytes: dump.bytes,
            retained_bytes: stats.source_retained_bytes,
            mapped_bytes: stats.source_mapped_bytes,
            source_retained_bytes: stats.source_retained_bytes,
            source_mapped_bytes: stats.source_mapped_bytes,
            hierarchy_bytes: stats.hierarchy_bytes,
            packed_signals: stats.packed_signals,
            packed_bytes: stats.packed_bytes,
            decoded_signals: stats.decoded_signals,
            decoded_changes: stats.decoded_changes,
            decoded_struct_bytes: stats.decoded_struct_bytes,
            decoded_payload_bytes: stats.decoded_payload_bytes,
            time_table_bytes: stats.time_table_bytes,
            lookup_cache_entries: stats.lookup_cache_entries,
            lookup_cache_bytes: stats.lookup_cache_bytes,
            cache_bytes: stats.cache_bytes,
            total_bytes: stats.total_bytes,
        })
    }

    fn dump(&self, id: DumpId) -> Result<Arc<NativeDump>, String> {
        self.lock()
            .dumps
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("unknown dump id {id}"))
    }
}

/// Open the waveform: memory mapped when possible, read through the OS cache
/// otherwise.
fn open_waveform(path: &Path) -> Result<Waveform, String> {
    match Waveform::open_path_mmap(path) {
        Ok(waveform) => Ok(waveform),
        Err(WaveformError::Io(error)) => {
            // no mapping on this file system (or the file is empty): the read
            // path keeps the dump usable, without retaining the bytes either
            eprintln!(
                "open_dump: cannot mmap {}: {error}; falling back to the read path",
                path.display()
            );
            Waveform::open_path(path)
                .map_err(|error| format!("open_dump: {}: {}", path.display(), error.message()))
        }
        Err(error) => Err(format!(
            "open_dump: {}: {}",
            path.display(),
            error.message()
        )),
    }
}

/// Flatten the hierarchy in display order: a scope row, its signals, then its
/// child scopes.
fn flatten_hierarchy(hierarchy: &HierarchyView) -> (Vec<FlatEntry>, Vec<(String, String)>) {
    let mut flat = Vec::new();
    let mut scopes = Vec::new();
    for child in hierarchy.root().children() {
        flatten_scope(child, 0, &mut flat, &mut scopes);
    }
    (flat, scopes)
}

fn flatten_scope(
    node: &ScopeNode,
    depth: u32,
    flat: &mut Vec<FlatEntry>,
    scopes: &mut Vec<(String, String)>,
) {
    let scope = scopes.len();
    scopes.push((node.path().to_string(), node.name().to_string()));
    flat.push(FlatEntry::Scope { depth, scope });
    for &signal in node.signals() {
        flat.push(FlatEntry::Signal {
            depth: depth + 1,
            signal,
        });
    }
    for child in node.children() {
        flatten_scope(child, depth + 1, flat, scopes);
    }
}

/// Render one value to the text the webview shows. Bit vectors go through the
/// library's ShowType; real/string/event values have no bit pattern and get the
/// library's own textual form.
fn render_value(value: &WaveValue, width: u32, show: &ShowType) -> String {
    if let Some(text) = value.render(width as usize, show) {
        return text;
    }
    match value {
        WaveValue::Real(number) => format_real(*number),
        WaveValue::String(text) => text.clone(),
        WaveValue::Event => "event".to_string(),
        // render() only returns None for the three cases above
        WaveValue::Bits(_) => String::new(),
    }
}

fn format_real(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// How a dump_find query matches a rendered change.
#[derive(Clone, Debug, PartialEq)]
enum FindMatcher {
    /// Plain text: case-insensitive equality with the rendered value.
    Text(String),
    /// A 0x/0b/0o query: the value is rendered in that radix and the digits
    /// are compared (leading zeros and letter case ignored).
    Radix { show: ShowType, digits: String },
}

impl FindMatcher {
    fn parse(query: &str) -> Result<Self, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Err("dump_find: the query is empty".to_string());
        }
        let lower = trimmed.to_ascii_lowercase();
        let prefixed = lower
            .strip_prefix("0x")
            .map(|rest| (ShowType::Hex, rest))
            .or_else(|| lower.strip_prefix("0b").map(|rest| (ShowType::Bin, rest)))
            .or_else(|| lower.strip_prefix("0o").map(|rest| (ShowType::Oct, rest)));
        if let Some((show, digits)) = prefixed {
            if digits.is_empty() {
                return Err(format!(
                    "dump_find: the query '{trimmed}' has no digits after the radix prefix"
                ));
            }
            return Ok(FindMatcher::Radix {
                show,
                digits: normalize_digits(digits),
            });
        }
        Ok(FindMatcher::Text(lower))
    }

    fn matches(&self, rendered: &str, width: u32, show: &ShowType, value: &WaveValue) -> bool {
        match self {
            FindMatcher::Text(text) => rendered.trim().to_ascii_lowercase() == *text,
            FindMatcher::Radix {
                show: radix_show,
                digits,
            } => {
                // render in the query's radix, independent of the display show
                let text = if *radix_show == *show {
                    rendered.to_string()
                } else {
                    render_value(value, width, radix_show)
                };
                normalize_digits(text.trim()) == *digits
            }
        }
    }
}

/// Lowercase a digit string and drop leading zeros (all zeros stay "0").
fn normalize_digits(text: &str) -> String {
    let lower = text.trim().to_ascii_lowercase();
    let trimmed = lower.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_show_type(show: Option<&str>) -> Result<ShowType, String> {
    match show
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        None | Some("") | Some("hex") => Ok(ShowType::Hex),
        Some("uint") | Some("dec") | Some("unsigned") => Ok(ShowType::UInt),
        Some("sint") | Some("signed") => Ok(ShowType::SInt),
        Some("oct") => Ok(ShowType::Oct),
        Some("bin") => Ok(ShowType::Bin),
        Some("ascii") | Some("asc") => Ok(ShowType::Ascii),
        Some(other) => Err(format!(
            "unknown show type '{other}' (expected hex, uint, sint, oct, bin or ascii)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SMALL_VCD: &str = "$date test $end\n$version wave-insight native dump test $end\n$timescale 1ns $end\n$scope module top $end\n$var wire 1 ! clk $end\n$var wire 4 \" data [3:0] $end\n$upscope $end\n$enddefinitions $end\n$dumpvars\n0!\nb0000 \"\n$end\n#0\n0!\n#10\n1!\nb1010 \"\n#20\n0!\n";

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("wave-insight-native-dumps")
            .join(format!("{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn write_small_vcd(name: &str) -> PathBuf {
        let path = test_dir(name).join("small.vcd");
        let mut file = std::fs::File::create(&path).expect("create small.vcd");
        file.write_all(SMALL_VCD.as_bytes())
            .expect("write small.vcd");
        path
    }

    /// The big FST the QA/lib teams generated; the test skips (loudly) when it
    /// is not on this machine.
    fn large_fst() -> Option<PathBuf> {
        let base = std::env::temp_dir().join("wi-fst");
        let path = base.join("large.fst");
        path.is_file().then_some(path)
    }

    fn open_large_fst(registry: &DumpRegistry) -> Option<DumpSummary> {
        let Some(path) = large_fst() else {
            eprintln!("skipping large-FST test: %TEMP%\\wi-fst\\large.fst is not present");
            return None;
        };
        Some(
            registry
                .open(&path.display().to_string())
                .expect("open large.fst"),
        )
    }

    fn signal_index(registry: &DumpRegistry, id: DumpId, path: &str) -> usize {
        let mut offset = 0;
        loop {
            let page = registry
                .hierarchy(id, offset, MAX_HIERARCHY_PAGE)
                .expect("page");
            if let Some(row) = page
                .rows
                .iter()
                .find(|row| row.kind == "signal" && row.path == path)
            {
                return row.signal.expect("signal index");
            }
            if page.rows.is_empty() {
                panic!("signal {path} not found");
            }
            offset += page.rows.len();
        }
    }

    #[test]
    fn small_dump_round_trip() {
        let path = write_small_vcd("round-trip");
        let registry = DumpRegistry::new();
        let summary = registry
            .open(&path.display().to_string())
            .expect("open small.vcd");
        assert_eq!(summary.format, "VCD");
        assert_eq!(summary.signal_count, 2);
        assert_eq!(summary.scope_count, 1);
        assert_eq!(summary.time_range, Some(TimeRangeDto { start: 0, end: 20 }));
        assert_eq!(summary.retained_bytes, 0, "a file source retains no bytes");
        assert!(summary.bytes > 0);
        assert!(summary.open_ms >= 0.0);

        let page = registry.hierarchy(summary.id, 0, 10).expect("hierarchy");
        assert_eq!(page.total, 3, "one scope + two signals");
        assert_eq!(page.rows[0].kind, "scope");
        assert_eq!(page.rows[0].path, "top");
        let clk = page
            .rows
            .iter()
            .find(|row| row.path == "top.clk")
            .expect("top.clk row");
        assert_eq!(clk.kind, "signal");
        assert_eq!(clk.width, Some(1));
        assert_eq!(clk.signal_kind.as_deref(), Some("bit_vector"));
        assert_eq!(clk.var_type.as_deref(), Some("Wire"));
        let data = page
            .rows
            .iter()
            .find(|row| row.path == "top.data")
            .expect("top.data row");
        assert_eq!(data.range.as_deref(), Some("[3:0]"));
        assert_eq!(data.width, Some(4));

        let clk_index = signal_index(&registry, summary.id, "top.clk");
        let window = registry
            .window(summary.id, &[clk_index], 0, Some(20), Some(16), None)
            .expect("window");
        assert_eq!(window.len(), 1);
        assert_eq!(window[0].signal, clk_index);
        assert!(!window[0].changes.is_empty());
        assert_eq!(window[0].changes[0].time, 0);
        assert_eq!(window[0].changes[0].value, "0");

        let value = registry
            .value_at(summary.id, clk_index, 15, None)
            .expect("value at")
            .expect("a value");
        assert_eq!(value, "1");

        // unknown ids and signals are clean errors, not panics
        assert!(registry.hierarchy(9999, 0, 1).is_err());
        assert!(registry.window(9999, &[0], 0, None, None, None).is_err());
        assert!(registry.value_at(9999, 0, 0, None).is_err());
        assert!(registry.value_at(summary.id, 99, 0, None).is_err());
        assert!(registry.close(9999).is_err());
        assert!(registry.close(summary.id).is_ok());
        assert_eq!(registry.open_count(), 0);
    }

    #[test]
    fn invalid_arguments_are_clean_errors() {
        let registry = DumpRegistry::new();
        assert!(registry.open("").is_err());
        assert!(registry.open("this/path/does/not/exist.vcd").is_err());
        let path = write_small_vcd("bad-show");
        let summary = registry.open(&path.display().to_string()).expect("open");
        assert!(registry
            .window(summary.id, &[0], 0, None, None, Some("nonsense"))
            .is_err());
        // start > end is an empty window, not an error
        let window = registry
            .window(summary.id, &[0], 20, Some(0), None, None)
            .expect("empty window");
        assert!(window[0].changes.is_empty());
    }

    #[test]
    fn large_fst_opens_without_retaining_the_file() {
        let registry = DumpRegistry::new();
        let Some(summary) = open_large_fst(&registry) else {
            return;
        };
        eprintln!(
            "large.fst: {} bytes, {} signals in {} scopes, format {}, range {:?}, open {:.2} ms, retained {} bytes, mapped {} bytes",
            summary.bytes,
            summary.signal_count,
            summary.scope_count,
            summary.format,
            summary.time_range,
            summary.open_ms,
            summary.retained_bytes,
            summary.mapped_bytes
        );
        assert_eq!(summary.format, "FST");
        assert!(summary.bytes > 100_000_000, "large.fst is > 100 MB");
        assert!(summary.signal_count > 3_000);
        assert!(summary.scope_count > 1);
        // the mapping is the point: nothing of the file is kept on the heap
        assert_eq!(summary.retained_bytes, 0, "mmap retains no heap bytes");
        assert_eq!(
            summary.mapped_bytes, summary.bytes as usize,
            "the whole file is mapped"
        );
        assert!(summary.time_range.is_some());
        assert!(registry.close(summary.id).is_ok());
        assert_eq!(registry.open_count(), 0);
    }

    #[test]
    fn large_fst_window_queries_are_fast() {
        let registry = DumpRegistry::new();
        let Some(summary) = open_large_fst(&registry) else {
            return;
        };
        // the dense clock plus a handful of 1-bit and bus signals
        let mut signals = Vec::new();
        signals.push(signal_index(&registry, summary.id, "top.clk"));
        let mut offset = 0;
        'outer: loop {
            let page = registry
                .hierarchy(summary.id, offset, MAX_HIERARCHY_PAGE)
                .expect("hierarchy");
            for row in &page.rows {
                if row.kind == "signal" {
                    let path = row.path.as_str();
                    if (path.starts_with("top.u0.") || path.starts_with("top.buses32."))
                        && signals.len() < 10
                    {
                        if let Some(signal) = row.signal {
                            signals.push(signal);
                        }
                        if signals.len() >= 10 {
                            break 'outer;
                        }
                    }
                }
            }
            if page.rows.is_empty() {
                break;
            }
            offset += page.rows.len();
        }
        assert!(signals.len() >= 10, "found the dense rows");

        let mut clock = Instant::now();
        let window = registry
            .window(
                summary.id,
                &signals,
                0,
                summary.time_range.map(|range| range.end),
                Some(256),
                None,
            )
            .expect("window query");
        let window_ms = clock.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(window.len(), signals.len());
        assert!(window.iter().all(|row| !row.changes.is_empty()));
        assert!(
            window.iter().any(|row| row.capped),
            "the dense signals hit the per-signal cap"
        );

        clock = Instant::now();
        let value = registry
            .value_at(summary.id, signals[0], 5_000_000, None)
            .expect("value query");
        let value_ms = clock.elapsed().as_secs_f64() * 1000.0;
        assert!(value.is_some(), "the clock has a value mid-trace");

        clock = Instant::now();
        let warm_value = registry
            .value_at(summary.id, signals[1], 5_000_000, None)
            .expect("warm value query");
        let warm_value_ms = clock.elapsed().as_secs_f64() * 1000.0;
        assert!(warm_value.is_some());

        clock = Instant::now();
        let page = registry
            .hierarchy(summary.id, 0, MAX_HIERARCHY_PAGE)
            .expect("hierarchy page");
        let page_ms = clock.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(page.rows.len(), MAX_HIERARCHY_PAGE.min(page.total));

        clock = Instant::now();
        let narrow = registry
            .window(
                summary.id,
                &signals,
                5_000_000,
                Some(5_001_000),
                Some(64),
                None,
            )
            .expect("narrow window");
        let narrow_ms = clock.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "large.fst queries: {} signals x window = {:.2} ms, first value_at = {:.2} ms, warm value_at = {:.2} ms, hierarchy page = {:.2} ms, narrow window = {:.2} ms",
            signals.len(),
            window_ms,
            value_ms,
            warm_value_ms,
            page_ms,
            narrow_ms
        );
        assert!(
            window_ms < 5_000.0,
            "a window query is not a full trace scan"
        );
        assert!(value_ms < 1_000.0, "value_at is a random access read");
        assert!(narrow_ms < 5_000.0);
        assert!(narrow.iter().all(|row| row.changes.len() <= 64));
        assert!(registry.close(summary.id).is_ok());
    }

    #[test]
    fn close_frees_the_dump_and_reopen_gets_a_new_id() {
        let registry = DumpRegistry::new();
        let Some(summary) = open_large_fst(&registry) else {
            return;
        };
        let first_id = summary.id;
        assert_eq!(registry.open_count(), 1);
        registry.close(first_id).expect("close");
        assert_eq!(registry.open_count(), 0);
        // the closed id is gone
        assert!(registry.hierarchy(first_id, 0, 1).is_err());
        // and the file can be opened again (the mapping was released)
        let path = large_fst().expect("large.fst");
        let second = registry
            .open(&path.display().to_string())
            .expect("reopen large.fst");
        assert!(second.id > first_id);
        assert_eq!(second.retained_bytes, 0);
        assert!(registry.close(second.id).is_ok());
    }

    fn find_value(
        registry: &DumpRegistry,
        id: DumpId,
        signal: usize,
        query: &str,
        from: u64,
        backwards: bool,
        show: ShowType,
    ) -> Option<FoundValue> {
        registry
            .find(id, signal, query, from, backwards, &show)
            .expect("find")
    }

    #[test]
    fn small_dump_find_matches_and_directions() {
        let path = write_small_vcd("find");
        let registry = DumpRegistry::new();
        let summary = registry.open(&path.display().to_string()).expect("open");
        let clk = signal_index(&registry, summary.id, "top.clk");
        let data = signal_index(&registry, summary.id, "top.data");

        // forwards from 0 includes the change at time 0
        assert_eq!(
            find_value(&registry, summary.id, clk, "0", 0, false, ShowType::Hex),
            Some(FoundValue {
                time: 0,
                value: "0".to_string()
            })
        );
        // forwards is strictly after `from` when from > 0
        assert_eq!(
            find_value(&registry, summary.id, clk, "1", 0, false, ShowType::Hex),
            Some(FoundValue {
                time: 10,
                value: "1".to_string()
            })
        );
        assert_eq!(
            find_value(&registry, summary.id, clk, "1", 10, false, ShowType::Hex),
            None
        );
        // from the middle
        assert_eq!(
            find_value(&registry, summary.id, clk, "0", 15, false, ShowType::Hex),
            Some(FoundValue {
                time: 20,
                value: "0".to_string()
            })
        );
        // backwards from the end returns the last change ...
        assert_eq!(
            find_value(
                &registry,
                summary.id,
                clk,
                "0",
                u64::MAX,
                true,
                ShowType::Hex
            ),
            Some(FoundValue {
                time: 20,
                value: "0".to_string()
            })
        );
        // ... and backwards is strictly before `from`
        assert_eq!(
            find_value(&registry, summary.id, clk, "1", 20, true, ShowType::Hex),
            Some(FoundValue {
                time: 10,
                value: "1".to_string()
            })
        );
        assert_eq!(
            find_value(&registry, summary.id, clk, "1", 10, true, ShowType::Hex),
            None
        );
        assert_eq!(
            find_value(&registry, summary.id, clk, "0", 0, true, ShowType::Hex),
            None
        );
        // plain text is case-insensitive and trims the query
        assert_eq!(
            find_value(&registry, summary.id, data, " a ", 0, false, ShowType::Hex),
            Some(FoundValue {
                time: 10,
                value: "a".to_string()
            })
        );
        assert_eq!(
            find_value(&registry, summary.id, data, "A", 0, false, ShowType::Hex),
            Some(FoundValue {
                time: 10,
                value: "a".to_string()
            })
        );
        // a 0x/0b/0o query is compared in that radix (leading zeros/case ignored)
        for query in ["0xa", "0xA", "0x0a", "0b1010", "0o12"] {
            assert_eq!(
                find_value(&registry, summary.id, data, query, 0, false, ShowType::Hex),
                Some(FoundValue {
                    time: 10,
                    value: "a".to_string()
                }),
                "{query}"
            );
        }
        // the query radix wins over the display show type
        assert_eq!(
            find_value(&registry, summary.id, data, "0xa", 0, false, ShowType::Bin),
            Some(FoundValue {
                time: 10,
                value: "1010".to_string()
            })
        );
        // values that never appear
        assert_eq!(
            find_value(&registry, summary.id, clk, "x", 0, false, ShowType::Hex),
            None
        );
        assert_eq!(
            find_value(&registry, summary.id, data, "0xff", 0, false, ShowType::Hex),
            None
        );
        registry.close(summary.id).expect("close");
    }

    #[test]
    fn small_dump_find_errors_are_clean() {
        let path = write_small_vcd("find-errors");
        let registry = DumpRegistry::new();
        let summary = registry.open(&path.display().to_string()).expect("open");
        assert!(registry
            .find(9999, 0, "0", 0, false, &ShowType::Hex)
            .is_err());
        assert!(registry
            .find(summary.id, 99, "0", 0, false, &ShowType::Hex)
            .is_err());
        let error = registry
            .find(summary.id, 0, "   ", 0, false, &ShowType::Hex)
            .expect_err("empty query");
        assert!(error.contains("query is empty"), "{error}");
        let error = registry
            .find(summary.id, 0, "0x", 0, false, &ShowType::Hex)
            .expect_err("prefix without digits");
        assert!(error.contains("no digits"), "{error}");
        registry.close(summary.id).expect("close");
    }

    #[test]
    fn small_dense_fst_find_when_present() {
        // the generated 3 MB FST covers the FST streaming path without the
        // 100 MB file; skipped when the fixture is not on this machine
        let path = std::env::temp_dir().join("wi-fst").join("dense.fst");
        if !path.is_file() {
            eprintln!(
                "skipping small-FST find test: {} is not present",
                path.display()
            );
            return;
        }
        let registry = DumpRegistry::new();
        let summary = registry
            .open(&path.display().to_string())
            .expect("open dense.fst");
        assert_eq!(summary.format, "FST");
        let clk = signal_index(&registry, summary.id, "top.clk");
        let end = summary.time_range.expect("range").end;

        let started = Instant::now();
        let first = find_value(&registry, summary.id, clk, "1", 0, false, ShowType::Hex);
        let first_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            first,
            Some(FoundValue {
                time: 10,
                value: "1".to_string()
            })
        );

        let started = Instant::now();
        let near = find_value(
            &registry,
            summary.id,
            clk,
            "1",
            end.saturating_sub(100),
            false,
            ShowType::Hex,
        );
        let near_ms = started.elapsed().as_secs_f64() * 1000.0;
        let hit = near.expect("a 1 after the window");
        assert!(hit.time > end.saturating_sub(100) && hit.value == "1");

        let started = Instant::now();
        let back = find_value(
            &registry,
            summary.id,
            clk,
            "1",
            u64::MAX,
            true,
            ShowType::Hex,
        );
        let back_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert!(back.is_some(), "the clock is 1 on the last step");

        eprintln!(
            "dense.fst find: forward near start {:.2} ms, forward near end {:.2} ms, backwards {:.2} ms (range end {})",
            first_ms, near_ms, back_ms, end
        );
        registry.close(summary.id).expect("close");
    }

    #[test]
    fn large_fst_find_forward_near_start_and_end() {
        let registry = DumpRegistry::new();
        let Some(summary) = open_large_fst(&registry) else {
            return;
        };
        let clk = signal_index(&registry, summary.id, "top.clk");
        let end = summary.time_range.expect("range").end;

        let started = Instant::now();
        let first = find_value(&registry, summary.id, clk, "1", 0, false, ShowType::Hex);
        let start_ms = started.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(
            first,
            Some(FoundValue {
                time: 10,
                value: "1".to_string()
            })
        );

        // the worst case the docs describe: the stream has to walk the file
        // up to the window before it can answer
        let from = end.saturating_sub(100);
        let started = Instant::now();
        let near = find_value(&registry, summary.id, clk, "1", from, false, ShowType::Hex);
        let near_ms = started.elapsed().as_secs_f64() * 1000.0;
        let hit = near.expect("a 1 after the window");
        assert!(hit.time > from && hit.value == "1");
        assert!(near_ms < 60_000.0, "near-end find took {near_ms:.0} ms");
        eprintln!(
            "large.fst find: forward near start {:.2} ms (hit at {}), forward near end {:.2} ms (hit at {})",
            start_ms,
            first.expect("first hit").time,
            near_ms,
            hit.time
        );
        registry.close(summary.id).expect("close");
    }

    #[test]
    fn value_at_never_materialises_the_body() {
        // small VCD: open and the lookups leave every body counter at zero
        let path = write_small_vcd("streaming-value-at");
        let registry = DumpRegistry::new();
        let summary = registry.open(&path.display().to_string()).expect("open");
        let clk = signal_index(&registry, summary.id, "top.clk");
        let before = registry.memory_stats(summary.id).expect("stats");
        assert_eq!(before.time_table_bytes, 0);
        assert_eq!(before.packed_bytes, 0);
        assert_eq!(before.decoded_struct_bytes, 0);
        assert_eq!(before.cache_bytes, 0);
        assert_eq!(
            registry
                .value_at(summary.id, clk, 15, None)
                .expect("value")
                .as_deref(),
            Some("1")
        );
        // the same time again and a forward step must stay flat as well
        assert_eq!(
            registry
                .value_at(summary.id, clk, 15, None)
                .expect("repeat")
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            registry
                .value_at(summary.id, clk, 18, None)
                .expect("forward")
                .as_deref(),
            Some("1")
        );
        let after = registry.memory_stats(summary.id).expect("stats");
        assert_eq!(after.time_table_bytes, 0);
        assert_eq!(after.packed_bytes, 0);
        assert_eq!(after.decoded_struct_bytes, 0);
        assert_eq!(after.decoded_payload_bytes, 0);
        assert_eq!(after.cache_bytes, 0);
        registry.close(summary.id).expect("close");

        // real large.fst when present: the open path (header time range) and
        // the streaming lookups must keep the body counters at zero
        let registry = DumpRegistry::new();
        let Some(summary) = open_large_fst(&registry) else {
            return;
        };
        let clk = signal_index(&registry, summary.id, "top.clk");
        let before = registry.memory_stats(summary.id).expect("stats");
        eprintln!(
            "large.fst streaming before: time_table={} packed={} decoded={} cache={}",
            before.time_table_bytes,
            before.packed_bytes,
            before.decoded_struct_bytes,
            before.cache_bytes
        );
        assert_eq!(
            registry
                .value_at(summary.id, clk, 5_000_010, None)
                .expect("value")
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            registry
                .value_at(summary.id, clk, 5_000_020, None)
                .expect("forward")
                .as_deref(),
            Some("0")
        );
        let after = registry.memory_stats(summary.id).expect("stats");
        eprintln!(
            "large.fst streaming after: time_table={} packed={} decoded={} cache={} lookup={}/{} bytes",
            after.time_table_bytes, after.packed_bytes, after.decoded_struct_bytes, after.cache_bytes,
            after.lookup_cache_entries, after.lookup_cache_bytes
        );
        assert_eq!(after.time_table_bytes, before.time_table_bytes);
        assert_eq!(after.packed_bytes, before.packed_bytes);
        assert_eq!(after.decoded_struct_bytes, before.decoded_struct_bytes);
        assert_eq!(after.cache_bytes, before.cache_bytes);
        assert_eq!(after.time_table_bytes, 0, "no body materialisation");
        assert_eq!(after.packed_bytes, 0, "no packed decodes retained");
        assert!(
            after.lookup_cache_entries >= 1,
            "the last-lookup cache is used"
        );
        registry.close(summary.id).expect("close");
    }

    #[test]
    fn values_at_batch_matches_single_lookups_and_stays_flat() {
        // small VCD: the batch equals the per-signal readout, duplicates repeat
        let path = write_small_vcd("batch-values");
        let registry = DumpRegistry::new();
        let summary = registry.open(&path.display().to_string()).expect("open");
        let clk = signal_index(&registry, summary.id, "top.clk");
        let data = signal_index(&registry, summary.id, "top.data");
        let batch = registry
            .values_at(summary.id, &[clk, data, clk], 15, None)
            .expect("batch");
        assert_eq!(batch.len(), 3);
        let single_clk = registry.value_at(summary.id, clk, 15, None).expect("clk");
        let single_data = registry.value_at(summary.id, data, 15, None).expect("data");
        assert_eq!(batch[0].as_ref().map(|row| row.value.clone()), single_clk);
        assert_eq!(batch[1].as_ref().map(|row| row.value.clone()), single_data);
        assert_eq!(batch[2].as_ref().map(|row| row.value.clone()), single_clk);
        assert_eq!(batch[0].as_ref().expect("clk row").signal, clk);
        assert_eq!(batch[1].as_ref().expect("data row").signal, data);
        let stats = registry.memory_stats(summary.id).expect("stats");
        assert_eq!(stats.time_table_bytes, 0);
        assert_eq!(stats.packed_bytes, 0);
        assert_eq!(stats.cache_bytes, 0);
        registry.close(summary.id).expect("close");

        // large.fst when present: a 16-signal batch matches dump_value_at and
        // leaves every body counter at zero
        let registry = DumpRegistry::new();
        let Some(summary) = open_large_fst(&registry) else {
            return;
        };
        let page = registry
            .hierarchy(summary.id, 0, MAX_HIERARCHY_PAGE)
            .expect("hierarchy");
        let signals: Vec<usize> = page
            .rows
            .iter()
            .filter(|row| row.kind == "signal")
            .filter_map(|row| row.signal)
            .take(16)
            .collect();
        assert_eq!(signals.len(), 16);
        let time = 5_000_010;
        let before = registry.memory_stats(summary.id).expect("stats");
        let batch = registry
            .values_at(summary.id, &signals, time, None)
            .expect("batch");
        assert_eq!(batch.len(), signals.len());
        let mut matched = 0;
        for (index, &signal) in signals.iter().enumerate() {
            let single = registry
                .value_at(summary.id, signal, time, None)
                .expect("single value_at");
            assert_eq!(
                batch[index].as_ref().map(|row| row.value.clone()),
                single,
                "batch and single differ for signal {signal}"
            );
            if let Some(row) = &batch[index] {
                assert_eq!(row.signal, signal);
                assert!(
                    row.time <= time,
                    "the change time is at or before the request"
                );
                matched += 1;
            }
        }
        assert!(matched >= 14, "only {matched} of 16 rows had a value");
        let after = registry.memory_stats(summary.id).expect("stats");
        eprintln!(
            "large.fst batch: 16 rows, matched {matched}, before time_table={} packed={} cache={}, after time_table={} packed={} cache={} lookup={}/{}",
            before.time_table_bytes, before.packed_bytes, before.cache_bytes,
            after.time_table_bytes, after.packed_bytes, after.cache_bytes,
            after.lookup_cache_entries, after.lookup_cache_bytes
        );
        assert_eq!(after.time_table_bytes, before.time_table_bytes);
        assert_eq!(after.packed_bytes, before.packed_bytes);
        assert_eq!(after.decoded_struct_bytes, before.decoded_struct_bytes);
        assert_eq!(after.cache_bytes, before.cache_bytes);
        assert_eq!(after.time_table_bytes, 0, "no body materialisation");
        assert_eq!(after.packed_bytes, 0, "no packed decodes retained");
        registry.close(summary.id).expect("close");
    }
}
