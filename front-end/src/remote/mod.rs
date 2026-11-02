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

use std::collections::HashMap;
use std::rc::Rc;

use serde::Deserialize;
use wave_insight_lib::waveform::{BitValue, Bits, Change, ShowType, WaveValue};

use crate::state::types::Time;

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

thread_local! {
    static NATIVE_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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

/// `true` when a file of `bytes` bytes should use the native path.
pub fn needs_native(bytes: u64) -> bool {
    bytes > NATIVE_THRESHOLD_BYTES
}

/// Parse one value the backend rendered to text back into the bit pattern the
/// renderer draws. The MVP asks for hex (`VALUE_FORMAT`); the other digit
/// radices are accepted too because they cost three lines and make the parser
/// usable by a future setting. Unparsable values (real/string/event rows) give
/// `None` and are not added as rows by the app.
pub fn parse_value(text: &str, width: usize) -> Option<WaveValue> {
    let width = width.max(1);
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // a 1 bit value carries no radix prefix
    if text == "0" || text == "1" {
        return Some(WaveValue::Bits(Bits::new_msb_first(vec![if text == "1" {
            BitValue::One
        } else {
            BitValue::Zero
        }])));
    }
    let (radix, digits) = if let Some(rest) = text.strip_prefix('\'') {
        let (radix, digits) = rest.split_at(1);
        match radix {
            "h" | "H" => (16u32, digits),
            "o" | "O" => (8, digits),
            "b" | "B" => (2, digits),
            // unsigned decimal is positional, not a digit-group radix
            "d" | "D" => {
                return parse_decimal(digits, width).map(WaveValue::Bits);
            }
            // a signed/ascii/unknown prefix is not a bit pattern the MVP draws
            _ => return None,
        }
    } else if text
        .chars()
        .all(|c| matches!(c, '0' | '1' | 'x' | 'X' | 'z' | 'Z'))
    {
        // a bare binary string (1 bit values of a bus rendered as bits)
        (2u32, text)
    } else {
        return None;
    };
    let bits = parse_digits(digits, radix, width)?;
    Some(WaveValue::Bits(bits))
}

fn parse_decimal(digits: &str, width: usize) -> Option<Bits> {
    let value: u64 = digits.parse().ok()?;
    let bits: Vec<BitValue> = (0..width)
        .rev()
        .map(|bit| {
            if bit < 64 && value & (1u64 << bit) != 0 {
                BitValue::One
            } else {
                BitValue::Zero
            }
        })
        .collect();
    Some(Bits::new_msb_first(bits))
}

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
    // pad or trim to the signal width (the text form drops leading zeros)
    if bits.len() < width {
        let mut padded = vec![BitValue::Zero; width - bits.len()];
        padded.extend(bits);
        bits = padded;
    } else if bits.len() > width {
        bits = bits.split_off(bits.len() - width);
    }
    Some(Bits::new_msb_first(bits))
}

/// Turn one IPC window into the renderer's bounded window.
pub fn row_window(dto: &WindowDto, req: WindowRequest) -> RowWindow {
    let changes: Vec<Change> = dto
        .changes
        .iter()
        .filter_map(|change| {
            let value = parse_value(&change.value, req_width(&req))?;
            Some((change.time, value))
        })
        .collect();
    let before = dto
        .before
        .as_ref()
        .and_then(|text| parse_value(text, req_width(&req)));
    RowWindow::new(changes, before, req, dto.capped)
}

/// The row width a window was asked for (the request does not carry it, so the
/// caller stores it alongside; 32 is the widest common default).
fn req_width(_req: &WindowRequest) -> usize {
    32
}

/// Turn one IPC window into the renderer's bounded window with the row's width.
pub fn row_window_with_width(dto: &WindowDto, req: WindowRequest, width: usize) -> RowWindow {
    let changes: Vec<Change> = dto
        .changes
        .iter()
        .filter_map(|change| Some((change.time, parse_value(&change.value, width)?)))
        .collect();
    let before = dto
        .before
        .as_ref()
        .and_then(|text| parse_value(text, width));
    RowWindow::new(changes, before, req, dto.capped)
}

/// The `show` argument for the backend: the row's format when the renderer can
/// parse it back, hex otherwise (the MVP draws everything as hex).
pub fn show_argument(_format: &ShowType) -> &'static str {
    VALUE_FORMAT
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
        let dump = RemoteDump::from_dto(summary);
        if !needs_native(dump.bytes) {
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
pub use ipc::{close, hierarchy, open, try_open, value_at, window};

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
    pub async fn close(_id: u64) -> Result<(), String> {
        Ok(())
    }
    pub async fn try_open(_path: &str) -> Result<Option<RemoteSession>, String> {
        Err("native dumps need the desktop build".to_string())
    }
}

// the browser build has no native backend: the stubs exist so the tauri-gated
// call sites compile in both builds, and nothing calls them here
#[cfg(not(feature = "tauri"))]
#[allow(unused_imports)]
pub use browser::{close, hierarchy, open, try_open, value_at, window};

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

    #[test]
    fn the_value_format_is_hex_for_the_mvp() {
        assert_eq!(show_argument(&ShowType::Hex), "hex");
        assert_eq!(show_argument(&ShowType::Bin), "hex");
    }
}
