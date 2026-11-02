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
    HierarchyView, ScopeNode, ShowType, SignalInfo, SignalKind, TimeRange, WaveValue, Waveform,
    WaveformError,
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

/// A dump opened natively. Only the hierarchy is parsed; values are streamed on
/// demand, so the retained heap cost is the hierarchy, not the file.
struct NativeDump {
    /// Pre-flattened hierarchy, so a page is a slice instead of a tree walk.
    flat: Vec<FlatEntry>,
    /// Scope path/name pairs referenced by the scope rows.
    scopes: Vec<(String, String)>,
    waveform: Waveform,
}

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
            flat,
            scopes,
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

    /// The rendered value of one signal at time.
    pub fn value_at(
        &self,
        id: DumpId,
        signal: usize,
        time: u64,
        show: Option<&str>,
    ) -> Result<Option<String>, String> {
        let dump = self.dump(id)?;
        let show = parse_show_type(show)?;
        let value = dump
            .waveform
            .value_at(signal, time)
            .map_err(|error| format!("dump_value_at: dump {id}: {}", error.message()))?;
        let width = dump
            .waveform
            .signal(signal)
            .map(|info| info.width())
            .unwrap_or(0);
        Ok(value.map(|value| render_value(&value, width, &show)))
    }

    /// Close one dump, freeing its mapping and hierarchy.
    pub fn close(&self, id: DumpId) -> Result<(), String> {
        match self.lock().dumps.remove(&id) {
            Some(_) => Ok(()),
            None => Err(format!("close_dump: unknown dump id {id}")),
        }
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
}
