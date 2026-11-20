//! JSON-lines bridge between the wave-insight DSH plugin and the waveform library.
//!
//! One request per line on stdin, one response per line on stdout. Every request
//! opens the dump with the native memory-mapped reader, so a helper process is
//! short lived and holds no state between calls. Values are bounded and plain
//! JSON so they can travel through a DSH session projection.
//!
//! Commands:
//!
//! * `ping`     -> `{ ok, version }`
//! * `open`     -> dump summary plus the first `limit` signal paths
//! * `find`     -> signal paths matching a case-insensitive substring
//! * `validate` -> which of the given paths exist, without decoding any value
//! * `wave`     -> bounded time/value changes for the given paths
//!
//! `.fsdb` goes through the library's `fsdb::prepare`: on the first open Verdi's
//! `fsdb2vcd` (or, later, the SDK backend) converts the trace and the VCD is
//! cached; every command then reads that cached VCD while the reported path and
//! format stay the original file's.  Without a converter the `open` failure
//! carries the library's actionable text (put `fsdb2vcd` on PATH or set
//! `WAVE_INSIGHT_FSDB2VCD`).

use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::Path;
use wave_insight_lib::waveform::{WaveValue, Waveform};

fn err(message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": message.into() })
}

/// Text a model can read. Bit vectors keep the bit pattern the panel draws
/// (truncated with an explicit width marker); real, string and event values
/// have no bit pattern and use the library's plain textual form instead of a
/// Rust `Debug` dump.
fn value_text(value: &WaveValue, width: u32) -> String {
    if let Some(bits) = value.to_bit_string() {
        let count = bits.chars().count();
        if count <= 128 {
            return bits;
        }
        let short: String = bits.chars().take(128).collect();
        return format!("{short}… ({count} bits)");
    }
    match value {
        WaveValue::Real(number) => {
            if number.is_finite() && number.fract() == 0.0 && number.abs() < 1e15 {
                format!("{number:.0}")
            } else {
                format!("{number}")
            }
        }
        WaveValue::String(text) => text.clone(),
        WaveValue::Event => "event".to_string(),
        // `to_bit_string` only returns None for the three cases above.
        WaveValue::Bits(_) => format!("{} bit value", width.max(1)),
    }
}

fn open_dump(path: &str) -> Result<Waveform, String> {
    // `.fsdb` is converted and cached inside the library; an unavailable
    // converter surfaces as the library's actionable message here.
    Waveform::open_path_mmap(path)
        .map_err(|error| format!("failed to open {path}: {}", error.message()))
}

fn signal_names(request: &Value) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(items) = request.get("signals").and_then(Value::as_array) {
        for item in items.iter().filter_map(Value::as_str) {
            if !names.iter().any(|seen| seen == item) {
                names.push(item.to_owned());
            }
        }
    }
    names
}

/// The dump's time range, as the JSON the plugin reads. GHW cannot answer the
/// cheap hint, so fall back to the first and last time step exactly as the
/// desktop registry does.
fn time_range_json(wf: &Waveform) -> Result<Option<Value>, String> {
    match wf.time_range_hint() {
        Ok(Some(range)) => Ok(Some(json!({ "start": range.start, "end": range.end }))),
        Ok(None) => {
            let start = wf.start_time().ok().flatten();
            let end = wf.end_time().ok().flatten();
            Ok(match (start, end) {
                (Some(start), Some(end)) => Some(json!({ "start": start, "end": end })),
                _ => None,
            })
        }
        Err(error) => Err(format!("time range: {}", error.message())),
    }
}

fn signal_json(signal: &wave_insight_lib::waveform::SignalInfo) -> Value {
    json!({
        "path": signal.path(),
        "name": signal.name(),
        "width": signal.width(),
        "range": signal.range_string(),
    })
}

fn handle(request: &Value) -> Value {
    let command = request.get("cmd").and_then(Value::as_str).unwrap_or("");
    if command == "ping" {
        return json!({ "ok": true, "version": 2 });
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
            let range = match time_range_json(&wf) {
                Ok(range) => range,
                Err(message) => return err(message),
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
                .map(signal_json)
                .collect();
            json!({
                "ok": true,
                "name": Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path),
                "path": path,
                // The library knows the real format; the extension can lie.
                "format": wf.file_format().name(),
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
                .map(signal_json)
                .collect();
            json!({ "ok": true, "matches": matches, "query": query })
        }
        // Resolve paths without decoding any value: `wave` with max 0 would
        // work too, but it returns one process round-trip per unknown name.
        "validate" => {
            let wf = match open_dump(path) {
                Ok(wf) => wf,
                Err(message) => return err(message),
            };
            let names = signal_names(request);
            let hierarchy = wf.hierarchy();
            let mut known = Vec::with_capacity(names.len());
            let mut unknown = Vec::new();
            for name in names {
                if hierarchy.signal_by_path(&name).is_some() {
                    known.push(name);
                } else {
                    unknown.push(name);
                }
            }
            json!({ "ok": true, "known": known, "unknown": unknown })
        }
        "wave" => {
            let wf = match open_dump(path) {
                Ok(wf) => wf,
                Err(message) => return err(message),
            };
            let names = signal_names(request);
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
            let widths: Vec<u32> = indices
                .iter()
                .map(|&index| wf.signal(index).map(|info| info.width()).unwrap_or(0))
                .collect();
            let windows = match wf.window_changes_many(&indices, start, end, max) {
                Ok(windows) => windows,
                Err(message) => return err(format!("window query: {message}")),
            };
            let waves: Vec<Value> = windows
                .iter()
                .enumerate()
                .map(|(row, window)| {
                    let changes: Vec<Value> = window
                        .changes
                        .iter()
                        .map(|(time, value)| json!({ "t": time, "v": value_text(value, widths[row]) }))
                        .collect();
                    json!({ "changes": changes, "capped": window.capped })
                })
                .collect();
            json!({ "ok": true, "waves": waves, "signals": names, "start": start, "end": end })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_fsdb_without_a_converter_answers_the_actionable_error() {
        let dir =
            std::env::temp_dir().join(format!("wave-insight-dsh-fsdb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the test directory");
        let fsdb = dir.join("run.fsdb");
        std::fs::write(&fsdb, b"not convertible here").expect("write the fake trace");

        // Point the converter lookup at a program that does not exist, so the
        // test does not depend on whether this machine has Verdi installed.
        let previous = std::env::var_os("WAVE_INSIGHT_FSDB2VCD");
        std::env::set_var("WAVE_INSIGHT_FSDB2VCD", dir.join("missing-fsdb2vcd"));
        let response = handle(&json!({
            "cmd": "open",
            "path": fsdb.display().to_string(),
        }));
        match previous {
            Some(value) => std::env::set_var("WAVE_INSIGHT_FSDB2VCD", value),
            None => std::env::remove_var("WAVE_INSIGHT_FSDB2VCD"),
        }

        assert_eq!(response.get("ok").and_then(Value::as_bool), Some(false));
        let error = response
            .get("error")
            .and_then(Value::as_str)
            .expect("an error string");
        assert!(error.contains("run.fsdb"), "{error}");
        assert!(error.contains("fsdb2vcd"), "{error}");
        assert!(error.contains("WAVE_INSIGHT_FSDB2VCD"), "{error}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
