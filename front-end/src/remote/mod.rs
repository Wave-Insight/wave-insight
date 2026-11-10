//! Native (desktop) dump backend: mmap-backed, read-only, over IPC.
//!
//! The desktop build can open very large dumps without copying them into the
//! webview: the Rust side (src-tauri/src/dumps.rs) maps the file, parses only the
//! header/hierarchy and answers *bounded* queries. This module is the front-end
//! half:
//!
//! * the DTOs of the frozen IPC protocol ([`DumpSummaryDto`] etc.),
//! * [`RemoteSession`]: what the app keeps for an open native dump — the
//!   summary, the flattened hierarchy turned into the same [`SidebarTree`] the
//!   sidebar renders, and a path -> signal index map so a row can be resolved
//!   without the wasm waveform,
//! * [`parse_value`]: the backend sends values already rendered to text; the
//!   renderer draws bit patterns, so this turns the hex text back into a
//!   [`WaveValue`] (the MVP asks for hex),
//! * [`Session::try_open`]: open through the native backend, decide whether the
//!   dump is big enough to stay native ([`NATIVE_THRESHOLD_BYTES`]; small dumps
//!   fall back to the wasm path) and load the hierarchy in pages.
//!
//! The commands used are exactly the frozen protocol:
//!
//! * `open_dump { path }` -> DumpSummaryDto
//! * `dump_hierarchy { id, offset, limit }` -> HierarchyPageDto (limit <= 4096)
//! * `dump_window { id, signals, start, end, maxChangesPerSignal, show }` -> [WindowDto]
//! * `dump_value_at { id, signal, time, show }` -> String | null
//! * `close_dump { id }` -> null
//!
//! Everything except the IPC calls is plain Rust and host tested.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use serde::Deserialize;
use wave_insight_lib::waveform::{BitValue, Bits, Change, ShowType, WaveValue};

use crate::state::types::{Time, View};

use crate::nav::tree::{ModuleRow, SignalRow};
use crate::nav::SidebarTree;
use crate::wave_show::windowed::{RowWindow, WindowRequest};

/// File size from which the desktop build opens a dump natively (the wasm path
/// keeps a copy of the file in linear memory; the native path maps it).
pub const NATIVE_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;
/// Hierarchy pages asked for at open (the backend caps one page at 4096 rows).
pub const HIERARCHY_PAGE: usize = 4096;
/// Safety cap on the hierarchy rows loaded into the sidebar tree (a dump with a
/// million rows would turn the tree into the memory problem it avoids).
pub const MAX_HIERARCHY_ROWS: usize = 400_000;
/// The value format the MVP asks the backend for (the renderer draws bit
/// patterns; hex always parses).
pub const VALUE_FORMAT: &str = "hex";

/// Inclusive time range of a trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct TimeRangeDto {
    pub start: u64,
    pub end: u64,
}

/// What `open_dump` returns.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct DumpSummaryDto {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub format: String,
    pub time_range: Option<TimeRangeDto>,
    pub signal_count: usize,
    pub scope_count: usize,
    pub retained_bytes: usize,
    pub mapped_bytes: usize,
    pub open_ms: f64,
}

/// One row of a hierarchy page.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct HierarchyRowDto {
    pub kind: String,
    pub depth: u32,
    pub path: String,
    pub name: String,
    #[serde(default)]
    pub signal: Option<usize>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub signal_kind: Option<String>,
    #[serde(default)]
    pub var_type: Option<String>,
}

/// One hierarchy page.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct HierarchyPageDto {
    pub total: usize,
    pub offset: usize,
    pub rows: Vec<HierarchyRowDto>,
}

/// One rendered value change.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ChangeDto {
    pub time: u64,
    pub value: String,
}

/// The bounded window of one signal, values already rendered to text.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct WindowDto {
    pub signal: usize,
    pub before: Option<String>,
    pub capped: bool,
    pub changes: Vec<ChangeDto>,
}

/// The summary an open native dump keeps for the UI (tab, status line, ruler).
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteDump {
    pub id: u64,
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub format: String,
    pub range: Option<(Time, Time)>,
    pub signal_count: usize,
    pub scope_count: usize,
    pub retained_bytes: usize,
    pub mapped_bytes: usize,
    pub open_ms: f64,
}

impl RemoteDump {
    pub fn from_dto(dto: DumpSummaryDto) -> Self {
        Self {
            id: dto.id,
            name: dto.name,
            path: dto.path,
            bytes: dto.bytes,
            format: dto.format,
            range: dto.time_range.map(|range| (range.start, range.end)),
            signal_count: dto.signal_count,
            scope_count: dto.scope_count,
            retained_bytes: dto.retained_bytes,
            mapped_bytes: dto.mapped_bytes,
            open_ms: dto.open_ms,
        }
    }

    /// Size in whole mebibytes, for messages.
    pub fn size_mb(&self) -> u64 {
        (self.bytes / (1024 * 1024)).max(1)
    }

    /// The status line of a native open: the stable "waveform loaded" prefix
    /// plus what the mode costs and what it maps.
    pub fn status_text(&self) -> String {
        let range = match self.range {
            Some((start, end)) => format!(", range {start}..{end}"),
            None => String::new(),
        };
        format!(
            "waveform loaded (native read-only, mmap) — {} {} MB{} in {:.1} ms, mapped {} MB, {} signals in {} scopes",
            self.format,
            self.size_mb(),
            range,
            self.open_ms,
            self.mapped_bytes / (1024 * 1024),
            self.signal_count,
            self.scope_count
        )
    }
}

/// One signal of the native hierarchy, resolved by path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeSignal {
    /// Index the backend uses in `dump_window` / `dump_value_at`.
    pub index: usize,
    /// Bit width (1 for scalars).
    pub width: usize,
    /// Draw as a single bit rather than a bus.
    pub is_bool: bool,
}

struct RemoteState {
    dump: RemoteDump,
    tree: SidebarTree,
    signals: HashMap<String, NativeSignal>,
}

/// Shared handle on one open native dump.
#[derive(Clone)]
pub struct RemoteSession(Rc<RemoteState>);

impl PartialEq for RemoteSession {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for RemoteSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteSession")
            .field("id", &self.0.dump.id)
            .field("name", &self.0.dump.name)
            .finish()
    }
}

impl RemoteSession {
    /// Build the session from the summary and the flattened hierarchy rows.
    ///
    /// Rows are the pre-order flattening of `dump_hierarchy` (a scope, its
    /// signals, then its child scopes); the tree is rebuilt with one pass over
    /// them, so the sidebar renders native dumps exactly like wasm ones.
    pub fn new(dump: RemoteDump, rows: &[HierarchyRowDto]) -> Self {
        let mut signals: HashMap<String, NativeSignal> = HashMap::new();
        // a sentinel root at depth 0 keeps the attach logic one loop
        let mut stack: Vec<(u32, ModuleRow)> = vec![(
            0,
            ModuleRow {
                name: String::new(),
                path: String::new(),
                signals: Vec::new(),
                children: Vec::new(),
            },
        )];
        for row in rows {
            while stack.len() > 1 && stack.last().map(|(depth, _)| *depth).unwrap_or(0) >= row.depth
            {
                let (_, scope) = stack.pop().expect("checked length");
                if let Some((_, parent)) = stack.last_mut() {
                    parent.children.push(scope);
                }
            }
            if row.kind == "scope" {
                stack.push((
                    row.depth,
                    ModuleRow {
                        name: row.name.clone(),
                        path: row.path.clone(),
                        signals: Vec::new(),
                        children: Vec::new(),
                    },
                ));
            } else if let Some(index) = row.signal {
                let width = row.width.unwrap_or(1).max(1) as usize;
                let is_bool = width == 1 && row.signal_kind.as_deref() != Some("real");
                signals.insert(
                    row.path.clone(),
                    NativeSignal {
                        index,
                        width,
                        is_bool,
                    },
                );
                if let Some((_, parent)) = stack.last_mut() {
                    parent.signals.push(SignalRow {
                        name: row.name.clone(),
                        path: row.path.clone(),
                        width,
                    });
                }
            }
        }
        while stack.len() > 1 {
            let (_, scope) = stack.pop().expect("checked length");
            if let Some((_, parent)) = stack.last_mut() {
                parent.children.push(scope);
            }
        }
        let root = stack.pop().expect("the sentinel root").1;
        Self(Rc::new(RemoteState {
            dump,
            tree: SidebarTree::new(root),
            signals,
        }))
    }

    pub fn dump(&self) -> &RemoteDump {
        &self.0.dump
    }

    pub fn id(&self) -> u64 {
        self.0.dump.id
    }

    pub fn range(&self) -> Option<(Time, Time)> {
        self.0.dump.range
    }

    pub fn status_text(&self) -> String {
        self.0.dump.status_text()
    }

    /// The sidebar tree: the native hierarchy, shaped like a wasm dump's.
    pub fn tree(&self) -> SidebarTree {
        self.0.tree.clone()
    }

    /// The backend signal index of a hierarchical path, when the dump has it.
    pub fn signal_of_path(&self, path: &str) -> Option<NativeSignal> {
        self.0.signals.get(path).copied()
    }

    /// Paths of the signals under one scope (for the bulk add): the tree stores
    /// them, so this is a plain walk.
    pub fn signals_under(&self, scope: &str) -> Vec<String> {
        crate::state::bulk::signals_under(&self.0.tree, scope)
    }

    /// The number of hierarchy rows the tree was built from.
    pub fn signal_count(&self) -> usize {
        self.0.signals.len()
    }
}

// --- the "a native dump was opened in the file dialog" channel -------------
//
// The desktop file browser is a child component; a native-only callback prop
// would have to be threaded through two components for one tauri feature, so it
// delivers the session here and the app installs one listener. In the browser
// build nothing ever delivers (there is no native backend).

type OpenListener = Rc<dyn Fn(RemoteSession)>;

thread_local! {
    static OPEN_LISTENER: std::cell::RefCell<Option<OpenListener>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the app's listener for dumps opened through the desktop file dialog.
pub fn set_open_listener(listener: Option<OpenListener>) {
    OPEN_LISTENER.with(|slot| *slot.borrow_mut() = listener);
}

/// Hand one freshly opened native dump to the app; `false` when no listener is
/// installed (the caller falls back to the wasm path).
pub fn deliver_open(session: RemoteSession) -> bool {
    OPEN_LISTENER.with(|slot| match slot.borrow().as_ref() {
        Some(listener) => {
            listener(session);
            true
        }
        None => false,
    })
}

/// The backend's `dump_memory_stats` answer for the open native dump.
///
/// Every field is the flattened library `MemoryStats` plus the registry's own
/// retained/mapped view; the wave pane publishes it under
/// `__waveDebug.memory.native` so a desktop QA run can see what the native
/// process holds (hierarchy + mapping, and that the body counters stay 0 on the
/// streaming path).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct DumpMemoryStatsDto {
    pub id: u64,
    pub name: String,
    pub bytes: u64,
    pub retained_bytes: u64,
    pub mapped_bytes: u64,
    pub source_retained_bytes: u64,
    pub source_mapped_bytes: u64,
    pub hierarchy_bytes: u64,
    pub packed_signals: u64,
    pub packed_bytes: u64,
    pub decoded_signals: u64,
    pub decoded_changes: u64,
    pub decoded_struct_bytes: u64,
    pub decoded_payload_bytes: u64,
    pub time_table_bytes: u64,
    pub lookup_cache_entries: u64,
    pub lookup_cache_bytes: u64,
    pub cache_bytes: u64,
    pub total_bytes: u64,
}

/// The native process working set (task-78 ~process_memory~ command).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct ProcessMemoryDto {
    pub current_bytes: Option<u64>,
    pub peak_bytes: Option<u64>,
}

/// What the desktop build holds: the open dump's library accounting and the
/// process working set (both from cheap backend commands).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeMemorySnapshot {
    pub dump: Option<DumpMemoryStatsDto>,
    pub process: Option<ProcessMemoryDto>,
}

thread_local! {
    static NATIVE_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NATIVE_MEMORY: std::cell::RefCell<Option<NativeMemorySnapshot>> =
        const { std::cell::RefCell::new(None) };
}

/// Remember the latest native memory snapshot (published by the debug hook).
pub fn set_native_memory(snapshot: Option<NativeMemorySnapshot>) {
    NATIVE_MEMORY.with(|slot| *slot.borrow_mut() = snapshot);
}

/// The latest native memory snapshot, cloned for the debug hook.
pub fn native_memory() -> Option<NativeMemorySnapshot> {
    NATIVE_MEMORY.with(|slot| slot.borrow().clone())
}

thread_local! {
    static LAST_MEMORY_REFRESH: std::cell::Cell<f64> = const { std::cell::Cell::new(-1.0e12) };
}

/// Refresh the native memory snapshot at most once per second.
///
/// The backend command is cheap (cached counters, no body materialisation), but
/// it is still an IPC round trip: the browser-level debug hook must not turn a
/// redraw loop into one request per frame. Desktop builds only.
#[cfg(feature = "tauri")]
pub fn maybe_refresh_native_memory(id: u64) {
    let now = js_sys::Date::now();
    let due = LAST_MEMORY_REFRESH.with(|last| {
        if now - last.get() >= 1000.0 {
            last.set(now);
            true
        } else {
            false
        }
    });
    if !due {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let dump = ipc::memory_stats(id).await.ok();
        let process = ipc::process_memory().await.ok();
        if dump.is_some() || process.is_some() {
            set_native_memory(Some(NativeMemorySnapshot { dump, process }));
        }
    });
}

/// Remember whether the desktop build is drawing a native dump right now.
///
/// The few call sites that only have the store (the value search) use this to
/// say why an operation is unavailable instead of pretending there is no dump.
pub fn set_native_active(active: bool) {
    NATIVE_ACTIVE.with(|flag| flag.set(active));
}

/// `true` while a native (read-only, mmap) dump is the one being drawn.
pub fn native_active() -> bool {
    NATIVE_ACTIVE.with(|flag| flag.get())
}

/// \`true\` when a file of `bytes` bytes should use the native path.
pub fn needs_native(bytes: u64) -> bool {
    bytes > NATIVE_THRESHOLD_BYTES
}

/// Which mode a dump of `bytes` bytes is opened in.
///
/// The desktop build keeps large dumps native (mmap, windowed IPC) and reads
/// small ones through wasm, where the extra copy is cheap and the whole existing
/// UI (find/export/split/multi-dump) keeps working. The decision is a pure
/// function so both open paths ([`ipc::try_open`] for drag&drop and
/// [`ipc::try_open_parts`] for the file dialog) share it and it is host tested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenDecision {
    /// Keep the dump open natively and draw it read-only.
    Native,
    /// Close the native dump and read the file through the wasm path.
    Wasm,
}

/// The mode decision for a dump of `bytes` bytes.
pub fn open_decision(bytes: u64) -> OpenDecision {
    if needs_native(bytes) {
        OpenDecision::Native
    } else {
        OpenDecision::Wasm
    }
}

/// Parse one value the backend rendered to text back into the bit pattern the
/// renderer draws.
///
/// The backend renders with the row's [`ShowType`] (see src-tauri's
/// `render_value`): hex/oct/bin are digit strings (x/z kept), uint/sint are
/// decimal text ("X" when the value has unknown bits), ascii is the raw
/// characters of the bytes. The renderer draws bit patterns, so every form is
/// parsed back into [`WaveValue::Bits`] at the signal width; a leading "'h2a"
/// style prefix (the ruler/find box form) is accepted too.
pub fn parse_rendered(text: &str, width: usize, show: &ShowType) -> Option<WaveValue> {
    let width = width.max(1);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some(rest) = text.strip_prefix('\'') {
        return parse_prefixed(rest, width);
    }
    match show {
        ShowType::Hex => parse_digits(text, 16, width).map(WaveValue::Bits),
        ShowType::Bin => parse_digits(text, 2, width).map(WaveValue::Bits),
        ShowType::Oct => parse_digits(text, 8, width).map(WaveValue::Bits),
        ShowType::UInt => {
            if text.eq_ignore_ascii_case("x") {
                return Some(all_x(width));
            }
            parse_decimal(text, width).map(WaveValue::Bits)
        }
        ShowType::SInt => {
            if text.eq_ignore_ascii_case("x") {
                return Some(all_x(width));
            }
            let signed: i128 = text.parse().ok()?;
            Some(WaveValue::Bits(bits_from_i128(signed, width)))
        }
        ShowType::Ascii => Some(WaveValue::Bits(bits_from_bytes(text.as_bytes(), width))),
    }
}

/// The lenient entry point: parse with hex digits, accepting the "'h.." form.
/// Kept for the ruler/find-box style values and for the tests.
pub fn parse_value(text: &str, width: usize) -> Option<WaveValue> {
    parse_rendered(text, width, &ShowType::Hex)
}

fn parse_prefixed(rest: &str, width: usize) -> Option<WaveValue> {
    let (radix, digits) = rest.split_at(1.min(rest.len()));
    match radix {
        "h" | "H" => parse_digits(digits, 16, width).map(WaveValue::Bits),
        "o" | "O" => parse_digits(digits, 8, width).map(WaveValue::Bits),
        "b" | "B" => parse_digits(digits, 2, width).map(WaveValue::Bits),
        "d" | "D" => parse_decimal(digits, width).map(WaveValue::Bits),
        _ => None,
    }
}

fn all_x(width: usize) -> WaveValue {
    WaveValue::Bits(Bits::new_msb_first(vec![BitValue::X; width]))
}

/// A two's complement (signed) value as the raw `width` bits the renderer draws.
fn bits_from_i128(value: i128, width: usize) -> Bits {
    let mask: u128 = if width >= 128 {
        u128::MAX
    } else {
        (1u128 << width) - 1
    };
    let raw = (value as u128) & mask;
    bits_from_u128(raw, width)
}

/// A bit pattern from a raw unsigned value, padded on the left.
fn bits_from_u128(raw: u128, width: usize) -> Bits {
    let bits: Vec<BitValue> = (0..width)
        .rev()
        .map(|bit| {
            if bit < 128 && raw & (1u128 << bit) != 0 {
                BitValue::One
            } else {
                BitValue::Zero
            }
        })
        .collect();
    Bits::new_msb_first(bits)
}

/// Parse a radix digit string (x/z digits become four/three/one unknown bits per
/// digit, as the backend's renderer emits them) into `width` bits.
fn parse_digits(digits: &str, radix: u32, width: usize) -> Option<Bits> {
    let per_digit = match radix {
        16 => 4usize,
        8 => 3,
        _ => 1,
    };
    let mut bits: Vec<BitValue> = Vec::with_capacity(width);
    for character in digits.chars() {
        match character {
            'x' | 'X' => bits.extend(std::iter::repeat_n(BitValue::X, per_digit)),
            'z' | 'Z' => bits.extend(std::iter::repeat_n(BitValue::Z, per_digit)),
            _ => {
                let digit = character.to_digit(radix)?;
                for shift in (0..per_digit).rev() {
                    bits.push(if digit & (1 << shift) != 0 {
                        BitValue::One
                    } else {
                        BitValue::Zero
                    });
                }
            }
        }
    }
    if bits.is_empty() {
        return None;
    }
    // the text form drops leading zeros: pad back to the signal width
    if bits.len() < width {
        let mut padded = vec![BitValue::Zero; width - bits.len()];
        padded.extend(bits);
        bits = padded;
    } else if bits.len() > width {
        bits = bits.split_off(bits.len() - width);
    }
    Some(Bits::new_msb_first(bits))
}

/// Unsigned decimal text (uint form) as `width` bits.
fn parse_decimal(digits: &str, width: usize) -> Option<Bits> {
    let value: u128 = digits.parse().ok()?;
    Some(bits_from_u128(value, width))
}

/// Bytes (ascii form) as the raw `width` bits, padded with zeros on the left.
fn bits_from_bytes(bytes: &[u8], width: usize) -> Bits {
    let mut bits: Vec<BitValue> = Vec::with_capacity(bytes.len() * 8);
    for byte in bytes {
        for shift in (0..8).rev() {
            bits.push(if byte & (1 << shift) != 0 {
                BitValue::One
            } else {
                BitValue::Zero
            });
        }
    }
    if bits.len() < width {
        let mut padded = vec![BitValue::Zero; width - bits.len()];
        padded.extend(bits);
        bits = padded;
    } else if bits.len() > width {
        bits = bits.split_off(bits.len() - width);
    }
    Bits::new_msb_first(bits)
}

/// Turn one IPC window into the renderer's bounded window, parsing the rendered
/// text with the row's display format.
pub fn row_window_with_width_and_show(
    dto: &WindowDto,
    req: WindowRequest,
    width: usize,
    show: &ShowType,
) -> RowWindow {
    let changes: Vec<Change> = dto
        .changes
        .iter()
        .filter_map(|change| Some((change.time, parse_rendered(&change.value, width, show)?)))
        .collect();
    let before = dto
        .before
        .as_ref()
        .and_then(|text| parse_rendered(text, width, show));
    RowWindow::new(changes, before, req, dto.capped)
}

/// Hex shorthand kept for the places that have no row format (and the tests).
pub fn row_window_with_width(dto: &WindowDto, req: WindowRequest, width: usize) -> RowWindow {
    row_window_with_width_and_show(dto, req, width, &ShowType::Hex)
}

/// The backend's name for one display format ("hex", "uint", ...), the value of
/// the `show` argument of dump_window/dump_value_at/dump_find.
pub fn show_name(show: &ShowType) -> &'static str {
    match show {
        ShowType::Hex => "hex",
        ShowType::UInt => "uint",
        ShowType::SInt => "sint",
        ShowType::Oct => "oct",
        ShowType::Bin => "bin",
        ShowType::Ascii => "ascii",
    }
}

/// The inverse of [`show_name`] (the backend also accepts the aliases dec/signed).
pub fn show_type_named(name: &str) -> Option<ShowType> {
    match name.trim().to_ascii_lowercase().as_str() {
        "hex" | "" => Some(ShowType::Hex),
        "uint" | "dec" | "unsigned" => Some(ShowType::UInt),
        "sint" | "signed" => Some(ShowType::SInt),
        "oct" => Some(ShowType::Oct),
        "bin" => Some(ShowType::Bin),
        "ascii" | "asc" => Some(ShowType::Ascii),
        _ => None,
    }
}

/// Where one frame's native window request goes: the visible rows grouped by
/// display format, because dump_window renders the text with ONE show type.
///
/// The group key is the backend argument, so the fetch and the parse agree: one
/// request per format that is actually on screen (normally one or two).
pub fn window_groups(
    rows: impl IntoIterator<Item = usize>,
    show_of: impl Fn(usize) -> ShowType,
) -> Vec<(&'static str, Vec<usize>)> {
    let mut groups: Vec<(&'static str, Vec<usize>)> = Vec::new();
    for row in rows {
        let name = show_name(&show_of(row));
        match groups.iter_mut().find(|(group, _)| *group == name) {
            Some((_, members)) => members.push(row),
            None => groups.push((name, vec![row])),
        }
    }
    groups
}

/// What the desktop open path does when the native backend did not keep the dump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFallback {
    /// The native backend *proved* the file is small (the >64 MiB threshold
    /// decision returned Wasm): read it through the wasm path, as before.
    Wasm,
    /// Do not fall back: report the native error instead.
    ///
    /// A native error carries no proof that the file is small, so copying the
    /// whole file into wasm memory to retry is exactly the multi-GB peak this
    /// path exists to avoid. The status line names the native error.
    Report,
}

/// The fallback decision after a native open.
///
/// `proven_small` is true only for the threshold answer (`Ok(None)` from
/// `try_open`/`try_open_parts`): the backend successfully opened the dump and
/// told us it is within the wasm threshold. Any error is [`NativeFallback::Report`];
/// there is deliberately no "error + unknown size" fallback.
pub fn native_fallback(proven_small: bool) -> NativeFallback {
    if proven_small {
        NativeFallback::Wasm
    } else {
        NativeFallback::Report
    }
}

/// The status line of a native open error on the desktop path.
///
/// The wording is also what a QA/doc match can look for: names the file, the
/// native error, and why the wasm fallback is not used. `file` is the name the
/// dialog/drop path shows, `error` the backend string.
pub fn native_error_status(file: &str, error: &str) -> String {
    format!(
        "{file}: native dump error ({error}) — not falling back to the wasm reader, because a possibly large dump would be copied into wasm memory; dumps up to 64 MB can still be opened through the wasm path"
    )
}

/// The JSON name of a [`ShowType`]: the `dump_find` command takes `show: ShowType`
/// (serde unit-variant names), unlike dump_window/dump_value_at which take a
/// plain string ("hex", ...).
pub fn show_serde_name(show: &ShowType) -> &'static str {
    match show {
        ShowType::Hex => "Hex",
        ShowType::UInt => "UInt",
        ShowType::SInt => "SInt",
        ShowType::Oct => "Oct",
        ShowType::Bin => "Bin",
        ShowType::Ascii => "Ascii",
    }
}

/// Translate the ruler/find box's Verdi-style value ("'h2a", "'d42", "'b101") to
/// the native search's query form (0x/0b/0o digit strings, plain text otherwise).
///
/// dump_find compares its query against the rendered text, so a radix prefix must
/// be expressed the way the backend parses it; an unprefixed query is passed
/// through unchanged (plain text, case-insensitive).
pub fn find_query(query: &str) -> String {
    let trimmed = query.trim();
    let Some(rest) = trimmed.strip_prefix('\'') else {
        return trimmed.to_string();
    };
    let Some(radix) = rest.chars().next() else {
        return trimmed.to_string();
    };
    let digits = &rest[1..];
    if digits.is_empty() {
        return trimmed.to_string();
    }
    match radix {
        'h' | 'H' => format!("0x{digits}"),
        'b' | 'B' => format!("0b{digits}"),
        'o' | 'O' => format!("0o{digits}"),
        'd' | 'D' => match digits.parse::<u128>() {
            Ok(value) => format!("0x{value:x}"),
            Err(_) => trimmed.to_string(),
        },
        _ => trimmed.to_string(),
    }
}

/// One find result the native backend reports (dump_find).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FindHitDto {
    pub time: u64,
    pub value: String,
}

/// The status line of a native find hit, parallel to the wasm path's wording.
pub fn find_status(row_name: &str, query: &str, hit: &FindHitDto) -> String {
    format!(
        "found {query} in {row_name} at t={} ({})",
        hit.time, hit.value
    )
}

/// The status line of a native find miss (the wasm path has "no match for ...").
pub fn find_missing_status(row_name: &str, query: &str, backward: bool) -> String {
    let when = if backward {
        "before cursor 1"
    } else {
        "after cursor 1"
    };
    format!("no match for \"{query}\" in {row_name} {when}")
}

/// The view that brings `time` into the left quarter of the window (the same
/// jump the wasm find performs).
pub fn view_for_time(mut view: View, width: f64, time: i64) -> View {
    let span = view.time_span(width);
    view.x_axis = (time as f64 - span * 0.25).max(0.0);
    view
}

/// A small per-row cache of rendered values at a cursor time: `(row, time)` ->
/// text, with an insertion-order eviction so a long session cannot grow it.
#[derive(Debug, Default)]
pub struct CursorCache {
    entries: HashMap<(usize, i32), String>,
    order: VecDeque<(usize, i32)>,
    cap: usize,
}

impl CursorCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            cap: 512,
        }
    }

    /// The cached text for one row at the given cursor time.
    pub fn get(&self, row: usize, time: i32) -> Option<&str> {
        self.entries.get(&(row, time)).map(String::as_str)
    }

    /// Whether one row/time pair is already cached.
    pub fn contains(&self, row: usize, time: i32) -> bool {
        self.entries.contains_key(&(row, time))
    }

    /// Remember one rendered value; the oldest entry goes when the cap is hit.
    pub fn insert(&mut self, row: usize, time: i32, text: String) {
        let key = (row, time);
        if self.entries.insert(key, text).is_none() {
            self.order.push_back(key);
        }
        while self.entries.len() > self.cap.max(1) {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    /// Cached entries (for tests and the debug hook).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

#[cfg(feature = "tauri")]
mod ipc {
    use super::*;

    async fn call<T: serde::de::DeserializeOwned>(
        command: &str,
        args: &JsValue,
    ) -> Result<T, String> {
        let value = crate::file_load::invoke(command, args.clone()).await;
        serde_wasm_bindgen::from_value(value).map_err(|error| format!("{command}: {error}"))
    }

    use wasm_bindgen::JsValue;

    /// Open `path` natively; returns the summary.
    pub async fn open(path: &str) -> Result<DumpSummaryDto, String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({ "path": path }))
            .map_err(|error| error.to_string())?;
        call("open_dump", &args).await
    }

    /// Open the path the in-app dialog collected (`parts` relative to the
    /// backend's base directory, e.g. ["Downloads", "run.fst"]).
    ///
    /// The dialog only knows the navigation parts, not an absolute path: the
    /// backend resolves them against its own base directory, which is the same
    /// directory the listing came from. The command is `open_dump_parts` and the
    /// argument is `parts`.
    pub async fn open_parts(parts: &[String]) -> Result<DumpSummaryDto, String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({ "parts": parts }))
            .map_err(|error| error.to_string())?;
        call("open_dump_parts", &args).await
    }

    /// One page of the flattened hierarchy.
    pub async fn hierarchy(
        id: u64,
        offset: usize,
        limit: usize,
    ) -> Result<HierarchyPageDto, String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({
            "id": id,
            "offset": offset,
            "limit": limit,
        }))
        .map_err(|error| error.to_string())?;
        call("dump_hierarchy", &args).await
    }

    /// The bounded value window of `signals`, rendered to text.
    pub async fn window(
        id: u64,
        signals: &[usize],
        start: Time,
        end: Option<Time>,
        max_changes_per_signal: usize,
        show: &str,
    ) -> Result<Vec<WindowDto>, String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({
            "id": id,
            "signals": signals,
            "start": start,
            "end": end,
            "maxChangesPerSignal": max_changes_per_signal,
            "show": show,
        }))
        .map_err(|error| error.to_string())?;
        call("dump_window", &args).await
    }

    /// The rendered value of one signal at one time.
    pub async fn value_at(
        id: u64,
        signal: usize,
        time: Time,
        show: &str,
    ) -> Result<Option<String>, String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({
            "id": id,
            "signal": signal,
            "time": time,
            "show": show,
        }))
        .map_err(|error| error.to_string())?;
        call("dump_value_at", &args).await
    }

    /// Find a value in one signal, rendered with `show`: the next (or previous,
    /// when `backwards`) change at/after `from` whose text matches `query`.
    ///
    /// Command `dump_find { id, signal, query, from, backwards, show }`; the
    /// backend answers `{ time, value }` or null.
    #[allow(clippy::too_many_arguments)]
    pub async fn find(
        id: u64,
        signal: usize,
        query: &str,
        from: u64,
        backwards: bool,
        show: &str,
    ) -> Result<Option<FindHitDto>, String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({
            "id": id,
            "signal": signal,
            "query": query,
            "from": from,
            "backwards": backwards,
            "show": show,
        }))
        .map_err(|error| error.to_string())?;
        call("dump_find", &args).await
    }

    /// What the native dump actually holds (library memory accounting + the
    /// registry's retained/mapped view). Cheap: the backend reads cached
    /// counters, it never materialises the body.
    ///
    /// Command `dump_memory_stats { id }`.
    pub async fn memory_stats(id: u64) -> Result<DumpMemoryStatsDto, String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({ "id": id }))
            .map_err(|error| error.to_string())?;
        call("dump_memory_stats", &args).await
    }

    /// The native process working set (current/peak). One syscall on the backend;
    /// safe to poll (task-78 `process_memory`).
    pub async fn process_memory() -> Result<ProcessMemoryDto, String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({}))
            .map_err(|error| error.to_string())?;
        call("process_memory", &args).await
    }

    /// Close one native dump (frees the mapping).
    pub async fn close(id: u64) -> Result<(), String> {
        let args = serde_wasm_bindgen::to_value(&serde_json::json!({ "id": id }))
            .map_err(|error| error.to_string())?;
        call("close_dump", &args).await
    }

    /// Open `path` and decide the mode: `Ok(Some(session))` keeps it native,
    /// `Ok(None)` means "small dump, closed again: use the wasm path", and an
    /// error means the native backend could not open it (also wasm path).
    ///
    /// The hierarchy is fetched page by page (the backend caps a page at 4096
    /// rows) and the tree is built once; [`MAX_HIERARCHY_ROWS`] bounds it.
    pub async fn try_open(path: &str) -> Result<Option<RemoteSession>, String> {
        let summary = open(path).await?;
        finish_open(summary).await
    }

    /// [`try_open`] for the in-app file dialog: the path arrives as the
    /// navigation parts the dialog collected, resolved by the backend against its
    /// own base directory (the directory the listing came from). Same threshold
    /// and fallback semantics as [`try_open`].
    pub async fn try_open_parts(parts: &[String]) -> Result<Option<RemoteSession>, String> {
        let summary = open_parts(parts).await?;
        finish_open(summary).await
    }

    /// Apply the mode decision to an opened dump and load its hierarchy into a
    /// session.
    ///
    /// `Ok(Some)` = keep it native (big dump), `Ok(None)` = it was small, so the
    /// dump is closed again and the caller uses the wasm path; an error means the
    /// hierarchy could not be fetched (the dump is closed too, so no mapping is
    /// leaked).
    async fn finish_open(summary: DumpSummaryDto) -> Result<Option<RemoteSession>, String> {
        let dump = RemoteDump::from_dto(summary);
        if open_decision(dump.bytes) == OpenDecision::Wasm {
            let _ = close(dump.id).await;
            return Ok(None);
        }
        let mut rows: Vec<HierarchyRowDto> = Vec::new();
        let mut offset = 0usize;
        loop {
            let page = match hierarchy(dump.id, offset, HIERARCHY_PAGE).await {
                Ok(page) => page,
                Err(error) => {
                    let _ = close(dump.id).await;
                    return Err(error);
                }
            };
            let total = page.total;
            let fetched = page.rows.len();
            rows.extend(page.rows);
            offset = offset.saturating_add(fetched);
            if fetched == 0 || offset >= total || rows.len() >= MAX_HIERARCHY_ROWS {
                break;
            }
        }
        Ok(Some(RemoteSession::new(dump, &rows)))
    }
}

#[allow(unused_imports)]
#[cfg(feature = "tauri")]
pub use ipc::{
    close, find, hierarchy, memory_stats, open, open_parts, process_memory, try_open,
    try_open_parts, value_at, window,
};

#[cfg(not(feature = "tauri"))]
mod browser {
    use super::*;

    /// The browser build has no native backend: every call is a clean error so
    /// the tauri-gated call sites can be written once.
    pub async fn open(_path: &str) -> Result<DumpSummaryDto, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn hierarchy(
        _id: u64,
        _offset: usize,
        _limit: usize,
    ) -> Result<HierarchyPageDto, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn window(
        _id: u64,
        _signals: &[usize],
        _start: Time,
        _end: Option<Time>,
        _max: usize,
        _show: &str,
    ) -> Result<Vec<WindowDto>, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn value_at(
        _id: u64,
        _signal: usize,
        _time: Time,
        _show: &str,
    ) -> Result<Option<String>, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn find(
        _id: u64,
        _signal: usize,
        _query: &str,
        _from: u64,
        _backwards: bool,
        _show: &str,
    ) -> Result<Option<FindHitDto>, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn memory_stats(_id: u64) -> Result<DumpMemoryStatsDto, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn process_memory() -> Result<ProcessMemoryDto, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn close(_id: u64) -> Result<(), String> {
        Ok(())
    }
    pub async fn open_parts(_parts: &[String]) -> Result<DumpSummaryDto, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn try_open(_path: &str) -> Result<Option<RemoteSession>, String> {
        Err("native dumps need the desktop build".to_string())
    }
    pub async fn try_open_parts(_parts: &[String]) -> Result<Option<RemoteSession>, String> {
        Err("native dumps need the desktop build".to_string())
    }
}

// the browser build has no native backend: the stubs exist so the tauri-gated
// call sites compile in both builds, and nothing calls them here
#[cfg(not(feature = "tauri"))]
#[allow(unused_imports)]
pub use browser::{
    close, find, hierarchy, memory_stats, open, open_parts, process_memory, try_open,
    try_open_parts, value_at, window,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn signal_row(depth: u32, path: &str, name: &str, index: usize, width: u32) -> HierarchyRowDto {
        HierarchyRowDto {
            kind: "signal".to_string(),
            depth,
            path: path.to_string(),
            name: name.to_string(),
            signal: Some(index),
            width: Some(width),
            range: None,
            signal_kind: Some("bit_vector".to_string()),
            var_type: Some("Wire".to_string()),
        }
    }

    fn scope_row(depth: u32, path: &str, name: &str) -> HierarchyRowDto {
        HierarchyRowDto {
            kind: "scope".to_string(),
            depth,
            path: path.to_string(),
            name: name.to_string(),
            signal: None,
            width: None,
            range: None,
            signal_kind: None,
            var_type: None,
        }
    }

    fn summary() -> RemoteDump {
        RemoteDump {
            id: 7,
            name: "large.fst".to_string(),
            path: "/tmp/large.fst".to_string(),
            bytes: 102_000_000,
            format: "FST".to_string(),
            range: Some((0, 1_000)),
            signal_count: 4,
            scope_count: 2,
            retained_bytes: 0,
            mapped_bytes: 102_000_000,
            open_ms: 9.4,
        }
    }

    #[test]
    fn a_native_error_never_falls_back_to_wasm() {
        // the only proof that the wasm path is safe is the threshold answer
        assert_eq!(native_fallback(true), NativeFallback::Wasm);
        assert_eq!(native_fallback(false), NativeFallback::Report);
        // the status line names the file, the native error and the reason
        let status = native_error_status("big.fst", "open_dump: bad magic bytes");
        assert!(status.contains("big.fst"));
        assert!(status.contains("bad magic bytes"));
        assert!(status.contains("not falling back"));
        assert!(status.contains("wasm"));
        assert!(status.contains("64 MB"));
        // and it is an error line, not a load line
        assert!(!status.starts_with("waveform loaded"));
    }

    #[test]
    fn the_open_decision_is_shared_by_both_open_paths() {
        // drag&drop (absolute path) and the file dialog (parts) must agree
        assert_eq!(open_decision(0), OpenDecision::Wasm);
        assert_eq!(open_decision(64 * 1024 * 1024), OpenDecision::Wasm);
        assert_eq!(open_decision(64 * 1024 * 1024 + 1), OpenDecision::Native);
        assert_eq!(open_decision(4 * 1024 * 1024 * 1024), OpenDecision::Native);
        // and it agrees with the bool helper the older code used
        for bytes in [
            0u64,
            1,
            NATIVE_THRESHOLD_BYTES,
            NATIVE_THRESHOLD_BYTES + 1,
            500_000_000,
        ] {
            assert_eq!(
                open_decision(bytes) == OpenDecision::Native,
                needs_native(bytes)
            );
        }
    }

    #[test]
    fn the_threshold_switches_to_the_native_path() {
        assert!(!needs_native(0));
        assert!(!needs_native(NATIVE_THRESHOLD_BYTES));
        assert!(needs_native(NATIVE_THRESHOLD_BYTES + 1));
        assert!(needs_native(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn the_status_line_says_native_read_only() {
        let text = summary().status_text();
        assert!(text.starts_with("waveform loaded"));
        assert!(text.contains("native read-only, mmap"));
        assert!(text.contains("FST"));
        assert!(text.contains("97 MB") || text.contains("97 MB,") || text.contains("97 MB "));
        assert!(text.contains("mapped"));
        assert!(text.contains("4 signals in 2 scopes"));
        // no range information must not print an empty pair
        let mut dump = summary();
        dump.range = None;
        assert!(!dump.status_text().contains("range"));
    }

    #[test]
    fn hex_values_parse_back_into_bit_patterns() {
        let value = parse_value("'h1f", 8).expect("hex parses");
        assert_eq!(value.width(), Some(8));
        // the renderer formats without the radix prefix (the value column adds it)
        assert_eq!(value.render(8, &ShowType::Hex).as_deref(), Some("1f"));
        // leading zeros are optional in the text form
        let short = parse_value("'h7", 8).expect("short hex parses");
        assert_eq!(short.render(8, &ShowType::Hex).as_deref(), Some("07"));
        // x and z digits expand to four unknown bits each
        let unknown = parse_value("'hx", 4).expect("x parses");
        assert_eq!(
            unknown.as_bits().and_then(|bits| bits.get(0)),
            Some(BitValue::X)
        );
        // 1 bit values carry no radix prefix
        let one = parse_value("1", 1).expect("bit parses");
        assert_eq!(one.render(1, &ShowType::Bin).as_deref(), Some("1"));
        // binary, octal and decimal forms
        assert!(parse_value("'b1010", 8).is_some());
        assert!(parse_value("'o17", 8).is_some());
        assert!(parse_value("'d42", 8).is_some());
        // real/string/event text is not a bit pattern
        assert!(parse_value("event", 1).is_none());
        assert!(parse_value("1.5", 8).is_none());
        assert!(parse_value("", 8).is_none());
        assert!(parse_value("'q1", 8).is_none());
    }

    #[test]
    fn the_tree_is_rebuilt_from_the_flat_hierarchy() {
        let rows = vec![
            scope_row(0, "top", "top"),
            signal_row(1, "top.clk", "clk", 0, 1),
            scope_row(1, "top.u0", "u0"),
            signal_row(2, "top.u0.pc", "pc", 1, 8),
            scope_row(1, "top.u1", "u1"),
            signal_row(2, "top.u1.pc", "pc", 2, 8),
        ];
        let session = RemoteSession::new(summary(), &rows);
        let tree = session.tree();
        let top = tree.find("top").expect("top scope");
        assert_eq!(top.name, "top");
        assert_eq!(top.signals.len(), 1);
        assert_eq!(top.signals[0].path, "top.clk");
        assert_eq!(top.children.len(), 2);
        assert_eq!(top.children[0].path, "top.u0");
        assert_eq!(top.children[0].signals[0].path, "top.u0.pc");
        assert_eq!(top.children[1].path, "top.u1");
        // the root is virtual (the scopes are its children)
        assert_eq!(tree.root().children.len(), 1);
        // rows resolve to backend signal indices and widths
        let pc = session.signal_of_path("top.u0.pc").expect("pc resolves");
        assert_eq!(pc.index, 1);
        assert_eq!(pc.width, 8);
        assert!(!pc.is_bool);
        assert!(session.signal_of_path("top.clk").expect("clk").is_bool);
        assert!(session.signal_of_path("nope").is_none());
        assert_eq!(session.signal_count(), 3);
    }

    #[test]
    fn a_window_becomes_a_bounded_row_window() {
        let dto = WindowDto {
            signal: 3,
            before: Some("'h0".to_string()),
            capped: true,
            changes: vec![
                ChangeDto {
                    time: 10,
                    value: "'h1".to_string(),
                },
                ChangeDto {
                    time: 20,
                    value: "event".to_string(),
                },
                ChangeDto {
                    time: 30,
                    value: "'h2".to_string(),
                },
            ],
        };
        let req = WindowRequest {
            start: 0,
            end: 100,
            visible_start: 0,
            visible_end: 100,
            cap: 16,
        };
        let window = row_window_with_width(&dto, req, 4);
        // the unparsable change is skipped, the rest keep their times and order
        let times: Vec<Time> = window.changes().iter().map(|(time, _)| *time).collect();
        assert_eq!(times, vec![10, 30]);
        assert!(window.capped);
        // the carried value fills the left edge
        assert!(window.value_at(0).is_some());
        assert_eq!(
            window.bytes(),
            2 * crate::wave_show::windowed::BYTES_PER_CHANGE
        );
    }

    fn bits_value(width: usize, raw: u128) -> WaveValue {
        let bits: Vec<BitValue> = (0..width)
            .rev()
            .map(|bit| {
                if bit < 128 && raw & (1u128 << bit) != 0 {
                    BitValue::One
                } else {
                    BitValue::Zero
                }
            })
            .collect();
        WaveValue::Bits(Bits::new_msb_first(bits))
    }

    #[test]
    fn each_show_type_round_trips_through_the_rendered_text() {
        // the backend renders with the row's ShowType; the renderer parses that
        // text back into the pattern it draws, so the drawn value must not change
        for (width, raw) in [
            (1usize, 1u128),
            (8, 0x1f),
            (12, 0x117),
            (16, 0xabcd),
            (32, 0xdead_beef),
        ] {
            let value = bits_value(width, raw);
            let expected = value.as_bits().expect("bits").bit_string();
            for show in [
                ShowType::Hex,
                ShowType::Bin,
                ShowType::Oct,
                ShowType::UInt,
                ShowType::SInt,
            ] {
                let text = value
                    .render(width, &show)
                    .unwrap_or_else(|| panic!("{show:?} renders {width} bits"));
                let parsed = parse_rendered(&text, width, &show)
                    .unwrap_or_else(|| panic!("{show:?} parses {text:?}"));
                assert_eq!(
                    parsed.as_bits().expect("bits").bit_string(),
                    expected,
                    "{show:?} text {text:?} lost bits"
                );
            }
        }
    }

    #[test]
    fn the_ascii_and_unknown_forms_parse_sensibly() {
        // ascii renders the bytes; 2 bytes = 16 bits
        let value = bits_value(16, 0x4142);
        let text = value.render(16, &ShowType::Ascii).expect("ascii");
        assert_eq!(text, "AB");
        let parsed = parse_rendered(&text, 16, &ShowType::Ascii).expect("ascii parses");
        assert_eq!(
            parsed.as_bits().expect("bits").bit_string(),
            "0100000101000010"
        );
        // uint/sint render "X" when a bit is unknown: every bit becomes unknown
        let mixed_x = WaveValue::Bits(Bits::new_msb_first(vec![
            BitValue::One,
            BitValue::X,
            BitValue::Zero,
            BitValue::Z,
        ]));
        for show in [ShowType::UInt, ShowType::SInt] {
            let text = mixed_x.render(4, &show).expect("renders");
            assert_eq!(text, "X");
            let parsed = parse_rendered(&text, 4, &show).expect("parses");
            assert_eq!(parsed.as_bits().expect("bits").bit_string(), "xxxx");
        }
        // signed decimal keeps the two's complement pattern
        let signed = parse_rendered("-1", 8, &ShowType::SInt).expect("sint");
        assert_eq!(signed.as_bits().expect("bits").bit_string(), "11111111");
        // a leading "'h2a" style value is accepted whatever the row format is
        let prefixed = parse_rendered("'h2a", 8, &ShowType::Bin).expect("prefix");
        assert_eq!(prefixed.as_bits().expect("bits").bit_string(), "00101010");
        // hexadecimal text with unknown digits keeps them
        let unknown = parse_rendered("x", 4, &ShowType::Hex).expect("hex x");
        assert_eq!(unknown.as_bits().expect("bits").bit_string(), "xxxx");
        // an empty/garbage value is not a value
        assert!(parse_rendered("", 8, &ShowType::Hex).is_none());
        assert!(parse_rendered("event", 8, &ShowType::Hex).is_none());
    }

    #[test]
    fn the_show_name_maps_both_ways() {
        let all = [
            (ShowType::Hex, "hex"),
            (ShowType::UInt, "uint"),
            (ShowType::SInt, "sint"),
            (ShowType::Oct, "oct"),
            (ShowType::Bin, "bin"),
            (ShowType::Ascii, "ascii"),
        ];
        for (show, name) in &all {
            assert_eq!(show_name(show), *name);
            assert_eq!(show_type_named(name), Some(show.clone()));
        }
        // the backend aliases
        assert_eq!(show_type_named("dec"), Some(ShowType::UInt));
        assert_eq!(show_type_named("signed"), Some(ShowType::SInt));
        assert_eq!(show_type_named("asc"), Some(ShowType::Ascii));
        assert_eq!(show_type_named("nonsense"), None);
        // the backend's accepted names are exactly the ones the groups produce
        for (show, name) in &all {
            assert!(show_type_named(name).is_some(), "{show:?}");
        }
    }

    #[test]
    fn visible_rows_are_grouped_by_display_format() {
        // a row's format is the group key: dump_window renders ONE show type
        let shows = [ShowType::Hex, ShowType::Bin, ShowType::Hex, ShowType::Ascii];
        let groups = window_groups(0..4, |row| shows[row].clone());
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].0, "hex");
        assert_eq!(groups[0].1, vec![0, 2]);
        assert_eq!(groups[1].0, "bin");
        assert_eq!(groups[1].1, vec![1]);
        assert_eq!(groups[2].0, "ascii");
        assert_eq!(groups[2].1, vec![3]);
        // every row lands in exactly one group
        let total: usize = groups.iter().map(|(_, rows)| rows.len()).sum();
        assert_eq!(total, 4);
        assert!(window_groups(0..0, |_| ShowType::Hex).is_empty());
    }

    #[test]
    fn the_cursor_cache_holds_rendered_values_and_evicts_old_ones() {
        let mut cache = CursorCache::new();
        assert!(cache.get(3, 100).is_none());
        assert!(!cache.contains(3, 100));
        cache.insert(3, 100, "'h1".to_string());
        assert_eq!(cache.get(3, 100), Some("'h1"));
        // a different time is a different key
        assert!(!cache.contains(3, 101));
        cache.insert(3, 101, "'h2".to_string());
        assert_eq!(cache.len(), 2);
        // re-inserting the same key replaces the text, not the count
        cache.insert(3, 100, "'h3".to_string());
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(3, 100), Some("'h3"));
        // the cap keeps the cache small: the oldest entry goes first
        for row in 0..600 {
            cache.insert(row, 5, "0".to_string());
        }
        assert!(cache.len() <= 512);
        assert_eq!(cache.get(3, 100), None);
        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn the_find_query_translates_the_ruler_forms() {
        assert_eq!(find_query("'h2a"), "0x2a");
        assert_eq!(find_query("'H2A"), "0x2A");
        assert_eq!(find_query("'b1010"), "0b1010");
        assert_eq!(find_query("'o17"), "0o17");
        assert_eq!(find_query("'d42"), "0x2a");
        // an unprefixed query is plain text and passes through
        assert_eq!(find_query("  2a  "), "2a");
        assert_eq!(find_query("event"), "event");
        // an unparsable/empty form is left alone (the backend reports it)
        assert_eq!(find_query("'dnot-a-number"), "'dnot-a-number");
        assert_eq!(find_query("'"), "'");
        assert_eq!(find_query("'h"), "'h");
    }

    #[test]
    fn the_find_show_argument_uses_the_serde_names() {
        // dump_find takes show: ShowType (unit variant names), NOT "hex"
        assert_eq!(show_serde_name(&ShowType::Hex), "Hex");
        assert_eq!(show_serde_name(&ShowType::UInt), "UInt");
        assert_eq!(show_serde_name(&ShowType::SInt), "SInt");
        assert_eq!(show_serde_name(&ShowType::Oct), "Oct");
        assert_eq!(show_serde_name(&ShowType::Bin), "Bin");
        assert_eq!(show_serde_name(&ShowType::Ascii), "Ascii");
        // and they differ from the string-argument names on purpose
        assert_ne!(show_serde_name(&ShowType::Hex), show_name(&ShowType::Hex));
    }

    #[test]
    fn native_find_results_map_to_status_lines_and_a_view() {
        let hit = FindHitDto {
            time: 420,
            value: "2a".to_string(),
        };
        let message = find_status("top.pc", "'h2a", &hit);
        assert!(message.contains("found 'h2a in top.pc"));
        assert!(message.contains("t=420"));
        assert!(message.contains("(2a)"));
        let miss = find_missing_status("top.pc", "'h2a", true);
        assert!(miss.contains("no match"));
        assert!(miss.contains("before cursor 1"));
        assert!(find_missing_status("top.pc", "'h2a", false).contains("after cursor 1"));
        // the found time lands a quarter into the window, like the wasm find
        let view = view_for_time(View::new(0.0, 1.0), 800.0, 420);
        assert!((view.x_axis - (420.0 - 200.0)).abs() < 1e-6);
        // and never pans before time zero
        assert_eq!(view_for_time(View::new(5.0, 1.0), 800.0, 3).x_axis, 0.0);
    }
}
