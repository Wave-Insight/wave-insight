//! JSON-lines bridge between the wave-insight DSH plugin and the waveform library.
//!
//! One request per line on stdin, one response per line on stdout. Every request
//! opens the dump with the native memory-mapped reader, so a helper process is
//! short lived and holds no state between calls. Values are bounded and plain
//! JSON so they can travel through a DSH session projection.

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::Path;
use wave_insight_lib::waveform::{WaveValue, Waveform};

fn err(message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": message.into() })
}

fn value_text(value: &WaveValue) -> String {
    if let Some(bits) = value.to_bit_string() {
        let count = bits.chars().count();
        if count <= 128 {
            bits
        } else {
            let short: String = bits.chars().take(128).collect();
            format!("{short}…")
        }
    } else {
        format!("{value:?}")
    }
}

fn format_name(path: &str) -> String {
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "fst" => "FST",
        "ghw" => "GHW",
        _ => "VCD",
    }
    .to_owned()
}

fn open_dump(path: &str) -> Result<Waveform, String> {
    Waveform::open_path_mmap(path).map_err(|e| format!("failed to open {path}: {e}"))
}

fn handle(request: &Value) -> Value {
    let command = request.get("cmd").and_then(Value::as_str).unwrap_or("");
    if command == "ping" {
        return json!({ "ok": true, "version": 1 });
    }
    let path = request.get("path").and_then(Value::as_str).unwrap_or("");
    if path.is_empty() {
        return err("missing `path`");
    }

    match command {
        "open" => {
            let wf = match open_dump(path) {
                Ok(wf) => wf,
                Err(message) => return err(message),
            };
            let range = match wf.time_range_hint() {
                Ok(Some(range)) => Some(json!({ "start": range.start, "end": range.end })),
                Ok(None) => None,
                Err(message) => return err(format!("time range: {message}")),
            };
            let limit = request
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(10_000)
                .min(100_000) as usize;
            let signals: Vec<Value> = wf
                .hierarchy()
                .signals()
                .iter()
                .take(limit)
                .map(|signal| {
                    json!({
                        "path": signal.path(),
                        "name": signal.name(),
                        "width": signal.width(),
                        "range": signal.range_string(),
                    })
                })
                .collect();
            json!({
                "ok": true,
                "name": Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path),
                "path": path,
                "format": format_name(path),
                "timescale": wf.timescale().map(|scale| scale.to_string()),
                "time_range": range,
                "signal_count": wf.hierarchy().len(),
                "signals": signals,
                "signals_truncated": wf.hierarchy().len() > signals.len(),
            })
        }
        "find" => {
            let wf = match open_dump(path) {
                Ok(wf) => wf,
                Err(message) => return err(message),
            };
            let query = request
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            let limit = request
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(100)
                .min(500) as usize;
            let matches: Vec<Value> = wf
                .hierarchy()
                .signals()
                .iter()
                .filter(|signal| signal.path().to_ascii_lowercase().contains(&query))
                .take(limit)
                .map(|signal| {
                    json!({
                        "path": signal.path(),
                        "name": signal.name(),
                        "width": signal.width(),
                        "range": signal.range_string(),
                    })
                })
                .collect();
            json!({ "ok": true, "matches": matches, "query": query })
        }
        "wave" => {
            let wf = match open_dump(path) {
                Ok(wf) => wf,
                Err(message) => return err(message),
            };
            let names: Vec<String> = request
                .get("signals")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            let start = request.get("start").and_then(Value::as_u64).unwrap_or(0);
            let end = request.get("end").and_then(Value::as_u64);
            let max = request
                .get("max")
                .and_then(Value::as_u64)
                .unwrap_or(256)
                .min(4096) as usize;
            let mut indices = Vec::with_capacity(names.len());
            for name in &names {
                match wf.hierarchy().signal_by_path(name) {
                    Some(signal) => indices.push(signal.index()),
                    None => return err(format!("unknown signal: {name}")),
                }
            }
            if indices.is_empty() {
                return json!({ "ok": true, "waves": [] });
            }
            let windows = match wf.window_changes_many(&indices, start, end, max) {
                Ok(windows) => windows,
                Err(message) => return err(format!("window query: {message}")),
            };
            let waves: Vec<Value> = windows
                .iter()
                .map(|window| {
                    let changes: Vec<Value> = window
                        .changes
                        .iter()
                        .map(|(time, value)| json!({ "t": time, "v": value_text(value) }))
                        .collect();
                    json!({ "changes": changes, "capped": window.capped })
                })
                .collect();
            json!({ "ok": true, "waves": waves, "start": start, "end": end })
        }
        _ => err(format!("unknown command: {command}")),
    }
}

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) => handle(&request),
            Err(error) => err(format!("invalid JSON request: {error}")),
        };
        let encoded = serde_json::to_string(&response)
            .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"response is not JSON\"}".to_owned());
        if writeln!(stdout, "{encoded}").is_err() || stdout.flush().is_err() {
            break;
        }
    }
}
