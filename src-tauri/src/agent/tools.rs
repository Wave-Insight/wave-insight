//! The MCP tool catalogue: definitions, argument parsing and dispatch.
//!
//! Every tool is a thin wrapper: the data tools call [`AgentData`], the control
//! tools call [`UiBridge`]. Keeping the two apart in one place makes the
//! permission story readable - `add_signals` cannot read a file, `get_changes`
//! cannot change what is on screen.
//!
//! Data tools take an optional `path`. When it is absent the server asks the
//! webview which dump is displayed and uses that file, which is the common
//! case ("the waveform I am looking at"); passing a path explicitly lets the
//! model analyse a file the window does not have open.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, ServerHandler};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use wave_insight_lib::agent::{CursorMove, UiCommand, UiResult};

use super::bridge::UiBridge;
use super::data::{
    AgentData, MAX_CHANGES_PER_SIGNAL, MAX_EXTRACT_SIGNALS, MAX_SIGNALS_PER_QUERY,
    MAX_SIGNAL_MATCHES, MAX_TIMES_PER_QUERY,
};
use super::extract::{self, ExtractRequest};

/// The tool server. Cloned by the streamable HTTP service for every request.
#[derive(Clone)]
pub struct AgentHandler {
    data: Arc<AgentData>,
    bridge: Arc<UiBridge>,
}

impl AgentHandler {
    pub fn new(data: Arc<AgentData>, bridge: Arc<UiBridge>) -> Self {
        Self { data, bridge }
    }

    // --- data tools -------------------------------------------------------

    /// The path a data tool should query: the argument, or the displayed dump.
    async fn resolve_path(&self, path: Option<String>) -> Result<String, String> {
        if let Some(path) = path {
            if !path.trim().is_empty() {
                return Ok(path);
            }
        }
        let snapshot = self.bridge.request(UiCommand::Snapshot).await?;
        let UiResult::Snapshot(snapshot) = snapshot else {
            return Err("the window answered a snapshot request with something else".to_string());
        };
        let active = snapshot
            .active
            .and_then(|index| snapshot.dumps.get(index))
            .or_else(|| snapshot.dumps.first());
        match active {
            Some(dump) => match &dump.path {
                Some(path) => Ok(path.clone()),
                None => Err(format!(
                    "'{}' was opened without an absolute path; pass `path` explicitly \
                     (the file is on this machine) or reopen it through the desktop file dialog",
                    if dump.name.is_empty() {
                        "the displayed dump"
                    } else {
                        &dump.name
                    }
                )),
            },
            None => Err(
                "no waveform is displayed. Open a dump in Wave Insight, or pass `path` to \
                 analyse a file directly."
                    .to_string(),
            ),
        }
    }

    async fn list_dumps(&self) -> Result<String, String> {
        let result = self.bridge.request(UiCommand::Snapshot).await?;
        let UiResult::Snapshot(snapshot) = result else {
            return Err("the window answered a snapshot request with something else".to_string());
        };
        let dumps: Vec<Value> = snapshot
            .dumps
            .iter()
            .map(|dump| {
                json!({
                    "name": dump.name,
                    "path": dump.path,
                    "format": dump.format,
                    "timescale": dump.timescale,
                    "time_range": dump.time_range,
                    "visible": dump.visible,
                    "cursors": dump.cursors,
                    "rows": dump.rows,
                    "rows_truncated": dump.rows_truncated,
                    "signal_count": dump.signal_count,
                    "queryable": dump.queryable,
                    "native": dump.native,
                })
            })
            .collect();
        Ok(json!({
            "active": snapshot.active,
            "native": snapshot.native,
            "surface_width": snapshot.surface_width,
            "visible": snapshot.visible,
            "sources": snapshot.sources,
            "dumps": dumps,
        })
        .to_string())
    }

    async fn find_signals(&self, args: FindSignalsArgs) -> Result<String, String> {
        let path = self.resolve_path(args.path).await?;
        let data = Arc::clone(&self.data);
        let limit = args.limit.unwrap_or(50);
        let result = tokio::task::spawn_blocking(move || {
            let (id, info) = data.open_path(&path)?;
            let (total, signals) = data.find_signals(
                id,
                &args.query,
                args.mode.as_deref().unwrap_or("contains"),
                args.scope.as_deref(),
                limit,
            )?;
            Ok::<_, String>(json!({
                "dump": info,
                "mode": args.mode.unwrap_or_else(|| "contains".to_string()),
                "total": total,
                "truncated": signals.len() < total,
                "signals": signals,
            }))
        })
        .await
        .map_err(|error| format!("find_signals: the query task failed: {error}"))??;
        Ok(result.to_string())
    }

    async fn get_values(&self, args: GetValuesArgs) -> Result<String, String> {
        let path = self.resolve_path(args.path).await?;
        let format = args.format.unwrap_or_else(|| "hex".to_string());
        let _show = super::dumps::parse_show_type(Some(&format))?;
        let data = Arc::clone(&self.data);
        let result = tokio::task::spawn_blocking(move || {
            let (id, info) = data.open_path(&path)?;
            let result = data.values(id, &args.signals, &args.times, &format, info)?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("get_values: the query task failed: {error}"))??;
        Ok(result.to_string())
    }

    async fn get_changes(&self, args: GetChangesArgs) -> Result<String, String> {
        let path = self.resolve_path(args.path).await?;
        let format = args.format.unwrap_or_else(|| "hex".to_string());
        // reject an unknown format here, where the message names the tool
        super::dumps::parse_show_type(Some(&format))?;
        let cap = args
            .max_changes_per_signal
            .unwrap_or(256)
            .clamp(1, MAX_CHANGES_PER_SIGNAL);
        let data = Arc::clone(&self.data);
        let result = tokio::task::spawn_blocking(move || {
            let (id, info) = data.open_path(&path)?;
            let start = args
                .start
                .unwrap_or_else(|| info.time_range.map(|range| range.start).unwrap_or(0));
            let end = args.end.or_else(|| info.time_range.map(|range| range.end));
            let result = data.changes(id, &args.signals, start, end, cap, &format, info)?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("get_changes: the query task failed: {error}"))??;
        Ok(result.to_string())
    }

    async fn search_value(&self, args: SearchValueArgs) -> Result<String, String> {
        let path = self.resolve_path(args.path).await?;
        let format = args.format.unwrap_or_else(|| "hex".to_string());
        let show = super::dumps::parse_show_type(Some(&format))?;
        let data = Arc::clone(&self.data);
        let result = tokio::task::spawn_blocking(move || {
            let (id, info) = data.open_path(&path)?;
            let from = args.from.unwrap_or(0);
            let result = data.search(
                id,
                &args.signal,
                &args.query,
                from,
                args.backwards.unwrap_or(false),
                &show,
                info,
            )?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("search_value: the query task failed: {error}"))??;
        Ok(result.to_string())
    }

    async fn summarize_signals(&self, args: SummarizeSignalsArgs) -> Result<String, String> {
        let path = self.resolve_path(args.path).await?;
        let format = args.format.unwrap_or_else(|| "hex".to_string());
        let show = super::dumps::parse_show_type(Some(&format))?;
        let data = Arc::clone(&self.data);
        let result = tokio::task::spawn_blocking(move || {
            let (id, info) = data.open_path(&path)?;
            let start = args
                .start
                .unwrap_or_else(|| info.time_range.map(|range| range.start).unwrap_or(0));
            let end = args
                .end
                .or_else(|| info.time_range.map(|range| range.end))
                .unwrap_or(start);
            let result = data.summarize(id, &args.signals, start, end, &show, &format, info)?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("summarize_signals: the query task failed: {error}"))??;
        Ok(result.to_string())
    }

    async fn extract_changes(&self, args: ExtractChangesArgs) -> Result<String, String> {
        let path = self.resolve_path(args.path).await?;
        let format = args.format.unwrap_or_else(|| "hex".to_string());
        let show = super::dumps::parse_show_type(Some(&format))?;
        let data = Arc::clone(&self.data);
        let result = tokio::task::spawn_blocking(move || {
            let (id, info) = data.open_path(&path)?;
            let start = args
                .start
                .unwrap_or_else(|| info.time_range.map(|range| range.start).unwrap_or(0));
            let end = args.end.or_else(|| info.time_range.map(|range| range.end));
            let mut signals = Vec::with_capacity(args.signals.len());
            for signal in &args.signals {
                let (index, resolved) = data.resolve(id, signal)?;
                signals.push((resolved.path, index, resolved.width));
            }
            let end = end.unwrap_or(start);
            let request = ExtractRequest {
                path,
                signals,
                start,
                end,
                show,
                format,
                max_rows: args
                    .max_rows
                    .unwrap_or(extract::DEFAULT_MAX_ROWS)
                    .min(extract::MAX_MAX_ROWS),
                max_bytes: args
                    .max_bytes
                    .unwrap_or(extract::DEFAULT_MAX_BYTES)
                    .min(extract::MAX_MAX_BYTES),
            };
            let result = extract::extract(&request)?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| format!("extract_changes: the extraction task failed: {error}"))??;
        Ok(result.to_string())
    }

    // --- control tools ----------------------------------------------------

    async fn add_signals(&self, args: AddSignalsArgs) -> Result<String, String> {
        if args.signals.is_empty() {
            return Err("give at least one signal path".to_string());
        }
        if args.signals.len() > 200 {
            return Err(format!(
                "too many signals in one call: {} (limit 200)",
                args.signals.len()
            ));
        }
        let result = self
            .bridge
            .request(UiCommand::AddSignals {
                signals: args.signals,
            })
            .await?;
        let UiResult::AddSignals(result) = result else {
            return Err("the window answered an add request with something else".to_string());
        };
        serde_json::to_string(&result).map_err(|error| error.to_string())
    }

    async fn remove_signals(&self, args: RemoveSignalsArgs) -> Result<String, String> {
        if args.signals.is_empty() {
            return Err("give at least one signal path".to_string());
        }
        let result = self
            .bridge
            .request(UiCommand::RemoveSignals {
                signals: args.signals,
            })
            .await?;
        let UiResult::RemoveSignals(result) = result else {
            return Err("the window answered a remove request with something else".to_string());
        };
        serde_json::to_string(&result).map_err(|error| error.to_string())
    }

    async fn set_time_range(&self, args: SetTimeRangeArgs) -> Result<String, String> {
        if args.start > args.end {
            return Err(format!(
                "the range is empty: start {} is after end {}",
                args.start, args.end
            ));
        }
        let result = self
            .bridge
            .request(UiCommand::SetTimeRange {
                start: args.start,
                end: args.end,
            })
            .await?;
        let UiResult::SetTimeRange(result) = result else {
            return Err("the window answered a range request with something else".to_string());
        };
        serde_json::to_string(&result).map_err(|error| error.to_string())
    }

    async fn set_cursors(&self, args: SetCursorsArgs) -> Result<String, String> {
        if args.one.is_none() && args.two.is_none() {
            return Err("give `one`, `two`, or both".to_string());
        }
        let result = self
            .bridge
            .request(UiCommand::SetCursors {
                one: args.one.map(CursorArg::into_move).transpose()?,
                two: args.two.map(CursorArg::into_move).transpose()?,
            })
            .await?;
        let UiResult::SetCursors(result) = result else {
            return Err("the window answered a cursor request with something else".to_string());
        };
        serde_json::to_string(&result).map_err(|error| error.to_string())
    }
}

impl ServerHandler for AgentHandler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Wave Insight exposes a waveform viewer to an AI assistant.\n\n\
             Start with `list_dumps`: it reports the dump the window is showing, its \
             time range (in raw time steps), the visible range, the cursors and the \
             rows already on the wave. Data tools take an optional `path`; when it is \
             omitted the displayed dump is used, and any local waveform file can be \
             analysed by passing its absolute path.\n\n\
             Times are the dump's own ticks (see `timescale` for their unit) and every \
             range is inclusive on both ends. Typical flow: `list_dumps` -> \
             `find_signals` -> `get_values` / `get_changes` -> `extract_changes` for a \
             long range. `add_signals`, `remove_signals`, `set_time_range` and \
             `set_cursors` change what the window shows; they need an absolute path for \
             a dump that was opened without one.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: tool_definitions(),
            next_cursor: None,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request.name.to_string();
        let arguments: Map<String, Value> = request.arguments.unwrap_or_default();
        let outcome = self.dispatch(&name, arguments).await;
        Ok(match outcome {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]).into(),
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]).into(),
        })
    }
}

impl AgentHandler {
    async fn dispatch(&self, name: &str, arguments: Map<String, Value>) -> Result<String, String> {
        match name {
            "list_dumps" => self.list_dumps().await,
            "find_signals" => self.find_signals(parse(arguments)?).await,
            "get_values" => self.get_values(parse(arguments)?).await,
            "get_changes" => self.get_changes(parse(arguments)?).await,
            "search_value" => self.search_value(parse(arguments)?).await,
            "summarize_signals" => self.summarize_signals(parse(arguments)?).await,
            "extract_changes" => self.extract_changes(parse(arguments)?).await,
            "add_signals" => self.add_signals(parse(arguments)?).await,
            "remove_signals" => self.remove_signals(parse(arguments)?).await,
            "set_time_range" => self.set_time_range(parse(arguments)?).await,
            "set_cursors" => self.set_cursors(parse(arguments)?).await,
            other => Err(format!("unknown tool '{other}'")),
        }
    }
}

/// Deserialize a tool's arguments, with a message the model can act on.
fn parse<T: for<'de> Deserialize<'de>>(arguments: Map<String, Value>) -> Result<T, String> {
    serde_json::from_value(Value::Object(arguments))
        .map_err(|error| format!("invalid arguments: {error}"))
}

fn schema(value: Value) -> Arc<Map<String, Value>> {
    match value {
        Value::Object(map) => Arc::new(map),
        other => panic!("a tool schema must be an object, got {other}"),
    }
}

fn tool(name: &'static str, description: &'static str, input: Value) -> Tool {
    Tool::new(name, description, schema(input))
}

fn tool_definitions() -> Vec<Tool> {
    vec![
        tool(
            "list_dumps",
            "List the dumps Wave Insight has open: the displayed one (index `active`), \
             every open tab, its format, timescale, full time range, the visible range, \
             the cursors and the signal paths already on the wave. Call this first: the \
             other tools default to the displayed dump.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        tool(
            "find_signals",
            "Search the dump's signals by name (case-insensitive) and return their full \
             hierarchical paths, which every other tool uses. `mode` is contains (the \
             default), prefix or exact; `scope` restricts the search to a scope path.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Part of the signal name, e.g. 'pc'." },
                    "mode": { "type": "string", "enum": ["contains", "prefix", "exact"] },
                    "scope": { "type": "string", "description": "Only signals under this scope path, e.g. 'top.u_cpu'." },
                    "limit": { "type": "integer", "minimum": 1, "maximum": MAX_SIGNAL_MATCHES },
                    "path": { "type": "string", "description": "Dump file to search; defaults to the displayed dump." }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
        ),
        tool(
            "get_values",
            "Read the value of signals at one or more times, in one batch. Answers report \
             `value_time`, the change the value comes from (<= the requested time), and \
             `found: false` when the signal had not changed yet. Times are raw dump ticks.",
            json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": MAX_SIGNALS_PER_QUERY,
                        "description": "Full signal paths from find_signals."
                    },
                    "times": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 0 },
                        "minItems": 1,
                        "maxItems": MAX_TIMES_PER_QUERY
                    },
                    "format": { "type": "string", "enum": ["hex", "uint", "sint", "oct", "bin", "ascii"] },
                    "path": { "type": "string" }
                },
                "required": ["signals", "times"],
                "additionalProperties": false
            }),
        ),
        tool(
            "get_changes",
            "The value changes of several signals inside a time range, capped per signal. \
             `capped: true` means the cap stopped the collection and `next_start` is where \
             to continue. Narrow the range instead of raising the cap when the answer is \
             large; use extract_changes for a whole trace.",
            json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": MAX_SIGNALS_PER_QUERY
                    },
                    "start": { "type": "integer", "minimum": 0, "description": "Inclusive; defaults to the dump start." },
                    "end": { "type": "integer", "minimum": 0, "description": "Inclusive; defaults to the dump end." },
                    "max_changes_per_signal": { "type": "integer", "minimum": 1, "maximum": MAX_CHANGES_PER_SIGNAL },
                    "format": { "type": "string", "enum": ["hex", "uint", "sint", "oct", "bin", "ascii"] },
                    "path": { "type": "string" }
                },
                "required": ["signals"],
                "additionalProperties": false
            }),
        ),
        tool(
            "search_value",
            "Walk one signal's changes looking for a value (plain text, or a 0x/0b/0o radix \
             form). `from` is where the search starts (default 0); `backwards` searches the \
             last matching change before it. The worst case is a full trace scan, so narrow \
             it with a large `from` when possible.",
            json!({
                "type": "object",
                "properties": {
                    "signal": { "type": "string" },
                    "query": { "type": "string", "description": "Value text, e.g. '1', '0xa', '0b1010'." },
                    "from": { "type": "integer", "minimum": 0 },
                    "backwards": { "type": "boolean" },
                    "format": { "type": "string", "enum": ["hex", "uint", "sint", "oct", "bin", "ascii"] },
                    "path": { "type": "string" }
                },
                "required": ["signal", "query"],
                "additionalProperties": false
            }),
        ),
        tool(
            "summarize_signals",
            "One streaming pass over the selected signals: how many changes fall in the \
             range, the first and last change times and the values at the range start and \
             end. Use it to decide whether extracting the range is worth it.",
            json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": MAX_SIGNALS_PER_QUERY
                    },
                    "start": { "type": "integer", "minimum": 0 },
                    "end": { "type": "integer", "minimum": 0 },
                    "format": { "type": "string", "enum": ["hex", "uint", "sint", "oct", "bin", "ascii"] },
                    "path": { "type": "string" }
                },
                "required": ["signals"],
                "additionalProperties": false
            }),
        ),
        tool(
            "extract_changes",
            "Stream a time range of the selected signals into a CSV file (long format: \
             time,signal,value) and return its path, row/byte counts, a per-signal breakdown \
             and a short preview. This is the tool for a whole trace or a dense range: it \
             never materialises the history in memory. A sidecar .meta.json records the \
             dump, the range and the format. Output goes to the system temp directory.",
            json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": MAX_EXTRACT_SIGNALS
                    },
                    "start": { "type": "integer", "minimum": 0 },
                    "end": { "type": "integer", "minimum": 0 },
                    "format": { "type": "string", "enum": ["hex", "uint", "sint", "oct", "bin", "ascii"] },
                    "max_rows": { "type": "integer", "minimum": 1 },
                    "max_bytes": { "type": "integer", "minimum": 1 },
                    "path": { "type": "string" }
                },
                "required": ["signals"],
                "additionalProperties": false
            }),
        ),
        tool(
            "add_signals",
            "Add signal paths to the wave view of the displayed dump, as one batch. Reports \
             which paths were added, which were already there and which the dump does not \
             contain. Use the full path find_signals returned.",
            json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": 200
                    }
                },
                "required": ["signals"],
                "additionalProperties": false
            }),
        ),
        tool(
            "remove_signals",
            "Remove rows from the wave view by signal path.",
            json!({
                "type": "object",
                "properties": {
                    "signals": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": 200
                    }
                },
                "required": ["signals"],
                "additionalProperties": false
            }),
        ),
        tool(
            "set_time_range",
            "Show the range [start, end] of the displayed dump (inclusive, in raw ticks). \
             The range is clamped to the dump and the answer reports what was applied.",
            json!({
                "type": "object",
                "properties": {
                    "start": { "type": "integer", "minimum": 0 },
                    "end": { "type": "integer", "minimum": 0 }
                },
                "required": ["start", "end"],
                "additionalProperties": false
            }),
        ),
        tool(
            "set_cursors",
            "Place or clear the two time cursors of the displayed dump. Each of `one`/`two` \
             is either {\"time\": <tick>} or {\"clear\": true}; omit one to leave it alone. \
             Cursors are limited to i32 time steps; use set_time_range for larger times.",
            json!({
                "type": "object",
                "properties": {
                    "one": {
                        "oneOf": [
                            { "type": "object", "properties": { "time": { "type": "integer", "minimum": 0 } }, "required": ["time"], "additionalProperties": false },
                            { "type": "object", "properties": { "clear": { "type": "boolean" } }, "required": ["clear"], "additionalProperties": false }
                        ]
                    },
                    "two": {
                        "oneOf": [
                            { "type": "object", "properties": { "time": { "type": "integer", "minimum": 0 } }, "required": ["time"], "additionalProperties": false },
                            { "type": "object", "properties": { "clear": { "type": "boolean" } }, "required": ["clear"], "additionalProperties": false }
                        ]
                    }
                },
                "additionalProperties": false
            }),
        ),
    ]
}

/// The names the panel lists; kept next to the definitions so a missing tool is
/// caught by `the_catalogue_matches_the_tool_names`.
#[cfg(test)]
pub fn tool_names() -> Vec<&'static str> {
    super::TOOL_NAMES.to_vec()
}

#[derive(Debug, Deserialize)]
struct FindSignalsArgs {
    query: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GetValuesArgs {
    signals: Vec<String>,
    times: Vec<u64>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GetChangesArgs {
    signals: Vec<String>,
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    end: Option<u64>,
    #[serde(default)]
    max_changes_per_signal: Option<usize>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchValueArgs {
    signal: String,
    query: String,
    #[serde(default)]
    from: Option<u64>,
    #[serde(default)]
    backwards: Option<bool>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SummarizeSignalsArgs {
    signals: Vec<String>,
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    end: Option<u64>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExtractChangesArgs {
    signals: Vec<String>,
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    end: Option<u64>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    max_rows: Option<u64>,
    #[serde(default)]
    max_bytes: Option<u64>,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AddSignalsArgs {
    signals: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RemoveSignalsArgs {
    signals: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SetTimeRangeArgs {
    start: u64,
    end: u64,
}

/// A cursor argument: `{"time": 10}` or `{"clear": true}`.
#[derive(Debug, Deserialize)]
struct CursorArg {
    #[serde(default)]
    time: Option<u64>,
    #[serde(default)]
    clear: Option<bool>,
}

impl CursorArg {
    fn into_move(self) -> Result<CursorMove, String> {
        if self.clear.unwrap_or(false) {
            return Ok(CursorMove::Clear);
        }
        match self.time {
            Some(time) => Ok(CursorMove::Set { time }),
            None => Err("a cursor needs either `time` or `clear`".to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SetCursorsArgs {
    #[serde(default)]
    one: Option<CursorArg>,
    #[serde(default)]
    two: Option<CursorArg>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_catalogue_matches_the_tool_names() {
        let defined: Vec<String> = tool_definitions()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert_eq!(defined, tool_names());
        // and the dispatch table knows every one of them
        for name in tool_names() {
            assert!(
                [
                    "list_dumps",
                    "find_signals",
                    "get_values",
                    "get_changes",
                    "search_value",
                    "summarize_signals",
                    "extract_changes",
                    "add_signals",
                    "remove_signals",
                    "set_time_range",
                    "set_cursors"
                ]
                .contains(&name),
                "{name} is listed but not dispatched"
            );
        }
    }

    #[test]
    fn every_tool_has_a_valid_object_schema() {
        for tool in tool_definitions() {
            let schema = tool.input_schema.as_ref();
            assert_eq!(
                schema.get("type").and_then(Value::as_str),
                Some("object"),
                "{} has no object input schema",
                tool.name
            );
            assert!(
                tool.description.is_some(),
                "{} has no description",
                tool.name
            );
        }
    }

    #[test]
    fn a_cursor_argument_is_a_time_or_a_clear() {
        let set: CursorArg = serde_json::from_value(json!({"time": 42})).unwrap();
        assert_eq!(set.into_move().unwrap(), CursorMove::Set { time: 42 });
        let clear: CursorArg = serde_json::from_value(json!({"clear": true})).unwrap();
        assert_eq!(clear.into_move().unwrap(), CursorMove::Clear);
        let empty: CursorArg = serde_json::from_value(json!({})).unwrap();
        assert!(empty.into_move().is_err());
    }

    #[test]
    fn bad_arguments_read_as_a_usable_message() {
        let error = parse::<SetTimeRangeArgs>(Map::new()).unwrap_err();
        assert!(error.starts_with("invalid arguments:"), "{error}");
    }
}
