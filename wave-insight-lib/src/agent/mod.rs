//! Types shared by the desktop MCP server and the webview it controls.
//!
//! The agent feature has two halves and they must agree on one wire format:
//! the Tauri backend runs the MCP server (`src-tauri/src/agent/`) and the
//! webview owns the state the tools read and change (`front-end/src/agent/`).
//! Defining the commands, replies and result payloads here means neither side
//! hand-writes JSON field names, and a serde round-trip test covers the shape
//! both of them rely on.
//!
//! Nothing in this module touches the network, Tauri or the DOM: it is plain
//! data, so the wasm build compiles it without pulling a single new
//! dependency.
//!
//! Times are the dump's own ticks (the same `u64` the waveform API uses) and
//! every range is inclusive on both ends.

use serde::{Deserialize, Serialize};

/// An inclusive range of dump time steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTimeRange {
    pub start: u64,
    pub end: u64,
}

/// The cursors of the active dump, in time steps.
///
/// The UI stores cursors as `i32` (see `front-end/src/state/types.rs`), so a
/// time past `i32::MAX` cannot be a cursor; the tool reports that instead of
/// silently truncating.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentCursors {
    pub one: Option<i32>,
    pub two: Option<i32>,
}

/// One dump the app knows about.
///
/// `path` is the absolute path the native backend needs to query the file. A
/// browser-created dump has none; the desktop build learns it when the file
/// was opened through a desktop transport, and `queryable` says whether the
/// data tools can read this dump.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentDump {
    /// File name (no directory).
    pub name: String,
    /// Absolute path, when the transport knew one.
    pub path: Option<String>,
    /// "VCD", "FST" or "GHW".
    pub format: String,
    /// Human-readable timescale (e.g. "1 ns"), when the format reports one.
    pub timescale: Option<String>,
    /// The dump's whole time range.
    pub time_range: Option<AgentTimeRange>,
    /// The range the wave pane currently shows.
    pub visible: Option<AgentTimeRange>,
    /// The committed cursors.
    pub cursors: AgentCursors,
    /// Hierarchical paths on the wave, in row order (deduplicated, capped).
    pub rows: Vec<String>,
    /// How many more rows the list would have carried.
    pub rows_truncated: bool,
    /// Signal count of the dump, when it is cheap to know.
    pub signal_count: Option<usize>,
    /// Whether the data tools can query this dump by `path`.
    pub queryable: bool,
    /// Whether the desktop native backend (mmap) currently draws it.
    pub native: bool,
}

/// What the tools see of the running app.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentSnapshot {
    /// Index into `dumps` of the dump the wave pane is showing.
    pub active: Option<usize>,
    /// Every open dump/tab, in tab order.
    pub dumps: Vec<AgentDump>,
    /// Whether the active dump is the native (mmap) one.
    pub native: bool,
    /// Width of the wave surface in css pixels; the visible range was derived
    /// from it. `0` means the pane has not been measured yet.
    pub surface_width: f64,
    /// The UI's idea of the current view (start/end in time steps).
    pub visible: Option<AgentTimeRange>,
    /// The open sources, for context ("the design the dump belongs to").
    pub sources: Vec<String>,
}

/// One signal a `find_signals` search matched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSignalMatch {
    /// Full hierarchical path, usable by every other tool.
    pub path: String,
    /// Local name inside its scope.
    pub name: String,
    /// Bit width.
    pub width: u32,
    /// "bit_vector", "event", "real" or "string".
    pub kind: String,
    /// Declared index range, e.g. "[7:0]".
    pub range: Option<String>,
    /// The scope the signal lives in.
    pub scope: String,
}

/// Dump metadata repeated in every data-tool answer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentDumpInfo {
    pub name: String,
    pub path: Option<String>,
    pub format: String,
    pub timescale: Option<String>,
    pub time_range: Option<AgentTimeRange>,
    pub signal_count: usize,
}

/// Result of `find_signals`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FindSignalsResult {
    pub dump: AgentDumpInfo,
    /// The match mode that was applied ("contains", "prefix" or "exact").
    pub mode: String,
    /// How many signals matched the pattern before `limit` was applied.
    pub total: usize,
    /// Whether `signals` was cut short by `limit`.
    pub truncated: bool,
    pub signals: Vec<AgentSignalMatch>,
}

/// One value readout of `get_values`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentValue {
    /// The signal path that was asked for.
    pub signal: String,
    /// The time the caller asked for.
    pub requested_time: u64,
    /// The change time the value comes from (<= `requested_time`).
    pub value_time: Option<u64>,
    /// The value rendered with the requested format.
    pub value: Option<String>,
    /// `false` when the signal had not changed at or before `requested_time`.
    pub found: bool,
}

/// Result of `get_values`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GetValuesResult {
    pub dump: AgentDumpInfo,
    /// The display format the values were rendered with.
    pub format: String,
    pub values: Vec<AgentValue>,
}

/// One change of one signal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentChange {
    pub time: u64,
    pub value: String,
}

/// The changes of one signal inside the requested range.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSignalChanges {
    pub signal: String,
    pub width: u32,
    /// The value in effect at the window start, when the backend had it.
    pub before: Option<String>,
    /// The per-signal cap stopped the collection.
    pub capped: bool,
    /// The time to continue from when `capped` is true.
    pub next_start: Option<u64>,
    pub changes: Vec<AgentChange>,
}

/// Result of `get_changes`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GetChangesResult {
    pub dump: AgentDumpInfo,
    pub start: u64,
    pub end: u64,
    pub format: String,
    /// The whole answer hit the requested budget; narrow the range.
    pub truncated: bool,
    pub signals: Vec<AgentSignalChanges>,
}

/// Result of `search_value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchValueResult {
    pub dump: AgentDumpInfo,
    pub signal: String,
    pub query: String,
    pub backwards: bool,
    pub found: bool,
    pub time: Option<u64>,
    pub value: Option<String>,
}

/// One signal's summary in `summarize_signals`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSignalSummary {
    pub signal: String,
    pub width: u32,
    /// Changes inside the range.
    pub changes: u64,
    pub first_time: Option<u64>,
    pub last_time: Option<u64>,
    /// The first value in the range (at or before its start).
    pub first_value: Option<String>,
    /// The last value seen in the range.
    pub last_value: Option<String>,
}

/// Result of `summarize_signals`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummarizeSignalsResult {
    pub dump: AgentDumpInfo,
    pub start: u64,
    pub end: u64,
    pub format: String,
    pub signals: Vec<AgentSignalSummary>,
}

/// Per-signal row count of an extraction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSignalCount {
    pub signal: String,
    pub rows: u64,
}

/// Result of `extract_changes`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExtractResult {
    pub dump: AgentDumpInfo,
    pub start: u64,
    pub end: u64,
    pub format: String,
    /// Absolute path of the written CSV file (long format: time,signal,value).
    pub output_path: String,
    /// Absolute path of the sidecar JSON with the metadata below.
    pub meta_path: String,
    pub rows: u64,
    pub bytes: u64,
    /// A row or byte budget stopped the extraction.
    pub truncated: bool,
    pub per_signal: Vec<AgentSignalCount>,
    pub first_time: Option<u64>,
    pub last_time: Option<u64>,
    /// The first and last rows of the file, so the model can sanity-check it
    /// without reading the whole thing.
    pub preview: Vec<String>,
    /// Extra context (e.g. the GHW materialisation warning).
    pub note: Option<String>,
}

/// Result of `add_signals`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AddSignalsResult {
    pub added: Vec<String>,
    pub already_present: Vec<String>,
    pub not_found: Vec<String>,
    /// Row count after the add.
    pub total_rows: usize,
    /// The status line the UI shows for this batch.
    pub status: String,
}

/// Result of `remove_signals`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RemoveSignalsResult {
    pub removed: Vec<String>,
    pub not_found: Vec<String>,
    /// Row count after the removal.
    pub total_rows: usize,
}

/// Result of `set_time_range`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SetTimeRangeResult {
    /// The dump the range was applied to, when one was displayed.
    pub dump: Option<String>,
    /// The requested range.
    pub requested: AgentTimeRange,
    /// The range that was actually applied (clamped to the dump).
    pub applied: AgentTimeRange,
    /// Whether the dump's own range changed the request.
    pub clamped: bool,
    /// Width of the wave surface the view was computed against.
    pub surface_width: f64,
}

/// Result of `set_cursors`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetCursorsResult {
    pub one: Option<i32>,
    pub two: Option<i32>,
}

/// What to do with one cursor.
///
/// An `Option<Option<i32>>` would be ambiguous over JSON (a `null` reads back
/// as "absent" either way), so "leave it alone" stays the *absence* of the
/// field in [`UiCommand::SetCursors`] and the two actions are explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum CursorMove {
    /// Put the cursor on this time step.
    Set { time: u64 },
    /// Remove the cursor.
    Clear,
}

/// One command the MCP server sends to the webview.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiCommand {
    /// Read the current UI state (no change).
    Snapshot,
    /// Add hierarchical signal paths to the wave, as one batch.
    AddSignals { signals: Vec<String> },
    /// Remove the rows whose path is listed.
    RemoveSignals { signals: Vec<String> },
    /// Show the range (clamped to the dump) and keep the cursors.
    SetTimeRange { start: u64, end: u64 },
    /// Move the cursors; `None` leaves that cursor alone.
    SetCursors {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        one: Option<CursorMove>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        two: Option<CursorMove>,
    },
}

/// What the webview answers for one [`UiCommand`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UiResult {
    Snapshot(AgentSnapshot),
    AddSignals(AddSignalsResult),
    RemoveSignals(RemoveSignalsResult),
    SetTimeRange(SetTimeRangeResult),
    SetCursors(SetCursorsResult),
}

impl UiResult {
    /// The `kind` tag the result carries (for error messages and tests).
    pub fn kind(&self) -> &'static str {
        match self {
            UiResult::Snapshot(_) => "snapshot",
            UiResult::AddSignals(_) => "add_signals",
            UiResult::RemoveSignals(_) => "remove_signals",
            UiResult::SetTimeRange(_) => "set_time_range",
            UiResult::SetCursors(_) => "set_cursors",
        }
    }
}

/// The webview's answer to one command id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UiReply {
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<UiResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl UiReply {
    pub fn ok(id: u64, result: UiResult) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: u64, message: impl Into<String>) -> Self {
        Self {
            id,
            result: None,
            error: Some(message.into()),
        }
    }
}

/// What the AI / MCP panel shows about the running server.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct McpInfo {
    pub running: bool,
    /// The endpoint an MCP client connects to, e.g.
    /// `http://127.0.0.1:7437/mcp`.
    pub url: Option<String>,
    /// The bearer token, in full: the panel masks it for display and copies it
    /// into the client config.
    pub token: String,
    pub port: Option<u16>,
    /// Why the server is not running, when it is not.
    pub error: Option<String>,
    /// The app version the client is talking to.
    pub version: String,
    /// The tools the server exposes, in registration order.
    pub tools: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> AgentSnapshot {
        AgentSnapshot {
            active: Some(0),
            dumps: vec![AgentDump {
                name: "sim.fst".into(),
                path: Some("D:\\sim\\sim.fst".into()),
                format: "FST".into(),
                timescale: Some("1 ns".into()),
                time_range: Some(AgentTimeRange { start: 0, end: 999 }),
                visible: Some(AgentTimeRange { start: 10, end: 20 }),
                cursors: AgentCursors {
                    one: Some(12),
                    two: None,
                },
                rows: vec!["top.pc".into()],
                rows_truncated: false,
                signal_count: Some(42),
                queryable: true,
                native: true,
            }],
            native: true,
            surface_width: 812.0,
            visible: Some(AgentTimeRange { start: 10, end: 20 }),
            sources: vec!["design.sv".into()],
        }
    }

    #[test]
    fn commands_and_results_round_trip_through_json() {
        let commands = [
            UiCommand::Snapshot,
            UiCommand::AddSignals {
                signals: vec!["top.pc".into()],
            },
            UiCommand::RemoveSignals {
                signals: vec!["top.pc".into()],
            },
            UiCommand::SetTimeRange {
                start: 100,
                end: 200,
            },
            UiCommand::SetCursors {
                one: Some(CursorMove::Set { time: 12 }),
                two: Some(CursorMove::Clear),
            },
        ];
        for command in commands {
            let text = serde_json::to_string(&command).unwrap();
            assert_eq!(serde_json::from_str::<UiCommand>(&text).unwrap(), command);
        }

        // "leave this cursor alone" is the absent field, and the two actions
        // stay distinguishable over JSON
        let text = serde_json::to_string(&UiCommand::SetCursors {
            one: None,
            two: Some(CursorMove::Clear),
        })
        .unwrap();
        assert_eq!(text, r#"{"kind":"set_cursors","two":{"action":"clear"}}"#);
        assert_eq!(
            serde_json::from_str::<UiCommand>(&text).unwrap(),
            UiCommand::SetCursors {
                one: None,
                two: Some(CursorMove::Clear)
            }
        );

        let results = [
            UiResult::Snapshot(snapshot()),
            UiResult::AddSignals(AddSignalsResult {
                added: vec!["top.pc".into()],
                already_present: vec![],
                not_found: vec!["top.nope".into()],
                total_rows: 1,
                status: "AI: added 1 signal".into(),
            }),
            UiResult::RemoveSignals(RemoveSignalsResult {
                removed: vec!["top.pc".into()],
                not_found: vec![],
                total_rows: 0,
            }),
            UiResult::SetTimeRange(SetTimeRangeResult {
                dump: Some("sim.fst".into()),
                requested: AgentTimeRange {
                    start: 100,
                    end: 200,
                },
                applied: AgentTimeRange {
                    start: 100,
                    end: 180,
                },
                clamped: true,
                surface_width: 800.0,
            }),
            UiResult::SetCursors(SetCursorsResult {
                one: Some(12),
                two: None,
            }),
        ];
        for result in results {
            let text = serde_json::to_string(&result).unwrap();
            assert_eq!(serde_json::from_str::<UiResult>(&text).unwrap(), result);
        }
    }

    #[test]
    fn the_command_tag_is_the_kind_field() {
        let text = serde_json::to_string(&UiCommand::SetTimeRange { start: 1, end: 2 }).unwrap();
        assert_eq!(text, r#"{"kind":"set_time_range","start":1,"end":2}"#);
        let text = serde_json::to_string(&UiResult::SetCursors(SetCursorsResult {
            one: None,
            two: None,
        }))
        .unwrap();
        assert!(text.starts_with(r#"{"kind":"set_cursors""#));
    }

    #[test]
    fn a_reply_omits_the_field_it_does_not_carry() {
        let text = serde_json::to_string(&UiReply::error(7, "boom")).unwrap();
        assert_eq!(text, r#"{"id":7,"error":"boom"}"#);
        let text = serde_json::to_string(&UiReply::ok(
            7,
            UiResult::SetCursors(SetCursorsResult {
                one: Some(1),
                two: None,
            }),
        ))
        .unwrap();
        assert!(text.contains(r#""kind":"set_cursors""#));
        assert!(!text.contains("error"));
    }

    #[test]
    fn a_data_answer_round_trips() {
        let result = FindSignalsResult {
            dump: AgentDumpInfo {
                name: "sim.vcd".into(),
                path: None,
                format: "VCD".into(),
                timescale: Some("1 ns".into()),
                time_range: None,
                signal_count: 3,
            },
            mode: "contains".into(),
            total: 1,
            truncated: false,
            signals: vec![AgentSignalMatch {
                path: "top.pc".into(),
                name: "pc".into(),
                width: 8,
                kind: "bit_vector".into(),
                range: Some("[7:0]".into()),
                scope: "top".into(),
            }],
        };
        let text = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serde_json::from_str::<FindSignalsResult>(&text).unwrap(),
            result
        );
        // the panel gets the token and the endpoint in one shape
        let info = McpInfo {
            running: true,
            url: Some("http://127.0.0.1:7437/mcp".into()),
            token: "abc".into(),
            port: Some(7437),
            error: None,
            version: "0.23.0".into(),
            tools: vec!["list_dumps".into()],
        };
        let text = serde_json::to_string(&info).unwrap();
        assert_eq!(serde_json::from_str::<McpInfo>(&text).unwrap(), info);
    }
}
