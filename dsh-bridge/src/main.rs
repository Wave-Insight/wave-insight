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
//! The design (source) side reads Verilog/SystemVerilog with the same library
//! the viewer uses, so the two answer alike. Each command parses the sources
//! it is given and answers one question about them:
//!
//! * `sources`      -> the companion RTL next to a dump (the SpinalHDL layout)
//! * `modules`      -> the module definitions, their ports and where they live
//! * `declare`      -> where one hierarchical signal is declared, plus its
//!   drivers and loads
//! * `diagnostics`  -> what the parser or the elaborator could not resolve
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
use wave_insight_companion as companion;
use wave_insight_lib::design::{DesignDb, Diagnostic, Severity, Span};
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

/// One source position as file:line:col, the form a model can quote back.
fn span_json(span: &Span) -> Value {
    json!({
        "file": span.file.display().to_string(),
        "line": span.start_line,
        "col": span.start_col,
        "end_line": span.end_line,
        "end_col": span.end_col,
    })
}

/// A source position as the one-line text the tool answers with.
fn span_text(span: &Span) -> String {
    format!(
        "{}:{}:{}",
        span.file.display(),
        span.start_line,
        span.start_col
    )
}

/// One diagnostic, bounded: the message is what a model acts on, the span is
/// where to look.
fn diagnostic_json(diagnostic: &Diagnostic) -> Value {
    json!({
        "severity": match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        },
        "kind": diagnostic.kind.as_str(),
        "message": diagnostic.message,
        "span": diagnostic.span.as_ref().map(span_json),
    })
}

/// The .v/.sv paths a design command was asked to read.
///
/// A command takes `paths` (an explicit list) or `dump` (find the companion RTL
/// next to that waveform). A path that is not a source file is reported by the
/// library as a diagnostic instead of being parsed as Verilog.
fn source_paths(request: &Value) -> Result<Vec<String>, String> {
    if let Some(items) = request.get("paths").and_then(Value::as_array) {
        let paths: Vec<String> = items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
        if paths.is_empty() {
            return Err("`paths` was empty: give the .v/.sv files to read".to_owned());
        }
        return Ok(paths);
    }
    if let Some(dump) = request.get("dump").and_then(Value::as_str) {
        let dump = dump.trim();
        if dump.is_empty() {
            return Err("`dump` was empty".to_owned());
        }
        let found = companion::companion_sources(Path::new(dump));
        if found.is_empty() {
            return Err(format!(
                "no companion RTL next to {dump}: looked for rtl/, generated/ or verilog/ in its directory and the two above it"
            ));
        }
        return Ok(found);
    }
    Err(
        "give either `paths` (.v/.sv files) or `dump` (find the RTL next to that waveform)"
            .to_owned(),
    )
}

/// Parse the requested sources into one database.
fn load_design(request: &Value) -> Result<DesignDb, String> {
    let paths = source_paths(request)?;
    let mut db = DesignDb::new();
    let mut unreadable = Vec::new();
    for path in &paths {
        // `add_file` answers false for a file the database already has and for
        // one it could not read; only the second is worth reporting here.
        if !db.add_file(path) && !Path::new(path).is_file() {
            unreadable.push(path.clone());
        }
    }
    if unreadable.len() == paths.len() {
        return Err(format!(
            "none of the sources could be read: {}",
            unreadable.join(", ")
        ));
    }
    Ok(db)
}

/// The module a design command elaborates: the explicit `top`, else the only
/// module the database has, else nothing (the caller then lists the modules
/// instead of guessing).
fn top_of(request: &Value, db: &DesignDb) -> Option<String> {
    if let Some(top) = request.get("top").and_then(Value::as_str) {
        let top = top.trim();
        if !top.is_empty() {
            return Some(top.to_owned());
        }
    }
    if db.modules().len() == 1 {
        return Some(db.modules()[0].name.clone());
    }
    None
}

fn signal_json(signal: &wave_insight_lib::waveform::SignalInfo) -> Value {
    json!({
        "path": signal.path(),
        "name": signal.name(),
        "width": signal.width(),
        "range": signal.range_string(),
    })
}

/// At most this many modules one `modules` answer lists.
const MODULE_LIMIT: usize = 400;
/// At most this many diagnostics one `diagnostics` answer carries.
const DIAGNOSTIC_LIMIT: usize = 100;

/// One port of a module definition, as the source declares it.
fn port_json(port: &wave_insight_lib::design::PortDef) -> Value {
    json!({
        "name": port.name,
        "direction": port.direction.as_str(),
        "kind": port.kind.as_str(),
        "width": port.width,
        "line": port.span.start_line,
    })
}

/// One module definition: where it is, what it exposes, what it contains.
fn module_json(module: &wave_insight_lib::design::ModuleDef) -> Value {
    json!({
        "name": module.name,
        "file": module.span.file.display().to_string(),
        "line": module.span.start_line,
        "module_ports": module.ports.iter().map(port_json).collect::<Vec<_>>(),
        "signals": module.signals.len(),
        "instances": module
            .instances
            .iter()
            .map(|instance| json!({ "name": instance.name, "module": instance.module }))
            .collect::<Vec<_>>(),
    })
}

/// The answers of the design (source) commands.
///
/// They parse the same Verilog/SystemVerilog the viewer parses, so a model and
/// the source pane cannot disagree about a declaration.
fn design_command(command: &str, request: &Value) -> Value {
    if command == "sources" {
        let dump = match request.get("dump").and_then(Value::as_str) {
            Some(dump) if !dump.trim().is_empty() => dump.trim(),
            _ => return err("`sources` needs `dump`: the waveform to look next to"),
        };
        let found = companion::companion_sources(Path::new(dump));
        return json!({ "ok": true, "dump": dump, "paths": found });
    }

    let db = match load_design(request) {
        Ok(db) => db,
        Err(message) => return err(message),
    };

    match command {
        "modules" => {
            let all = db.modules();
            let limit = request
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(MODULE_LIMIT as u64)
                .min(MODULE_LIMIT as u64) as usize;
            let modules: Vec<Value> = all.iter().take(limit).map(module_json).collect();
            json!({
                "ok": true,
                "files": db.files().iter().map(|file| file.display().to_string()).collect::<Vec<_>>(),
                "module_count": all.len(),
                "modules": modules,
                "truncated": all.len() > modules.len(),
            })
        }
        "declare" => {
            let signal = match request.get("signal").and_then(Value::as_str) {
                Some(signal) if !signal.trim().is_empty() => signal.trim(),
                _ => return err("`declare` needs `signal`: the hierarchical path to locate"),
            };
            let top = match top_of(request, &db) {
                Some(top) => top,
                None => {
                    let names: Vec<&str> = db
                        .modules()
                        .iter()
                        .take(20)
                        .map(|m| m.name.as_str())
                        .collect();
                    return err(format!(
                        "`declare` needs `top` (the design has {} modules: {})",
                        db.modules().len(),
                        names.join(", ")
                    ));
                }
            };
            let design = db.elaborate(&top);
            let found = match design.signal(signal) {
                Some(found) => found,
                None => {
                    return json!({
                        "ok": true,
                        "signal": signal,
                        "declared": false,
                        "top": top,
                        "diagnostics": design
                            .diagnostics()
                            .iter()
                            .take(DIAGNOSTIC_LIMIT)
                            .map(diagnostic_json)
                            .collect::<Vec<_>>(),
                    })
                }
            };
            json!({
                "ok": true,
                "signal": signal,
                "declared": true,
                "top": top,
                "declaration": span_json(&found.declaration),
                "location": span_text(&found.declaration),
                "module": found.module,
                "instance": found.instance,
                "kind": found.kind.as_str(),
                "direction": found.direction.map(|direction| direction.as_str()),
                "width": found.width,
                "drivers": found
                    .drivers
                    .iter()
                    .map(|driver| json!({ "location": span_text(&driver.span), "bits": driver.bits }))
                    .collect::<Vec<_>>(),
                "loads": found
                    .loads
                    .iter()
                    .map(|load| json!({ "location": span_text(&load.span), "bits": load.bits }))
                    .collect::<Vec<_>>(),
            })
        }
        _ => {
            // "diagnostics": what the parser skipped, plus what the elaborator
            // could not resolve when a top is known.
            let mut entries: Vec<Value> = db.diagnostics().iter().map(diagnostic_json).collect();
            let mut top = top_of(request, &db);
            if top.is_none() && db.modules().len() > 1 {
                top = None;
            }
            if let Some(ref top) = top {
                let design = db.elaborate(top);
                entries.extend(design.diagnostics().iter().map(diagnostic_json));
            }
            let total = entries.len();
            entries.truncate(DIAGNOSTIC_LIMIT);
            json!({
                "ok": true,
                "modules": db.modules().len(),
                "top": top,
                "count": total,
                "diagnostics": entries,
                "truncated": total > DIAGNOSTIC_LIMIT,
            })
        }
    }
}

fn handle(request: &Value) -> Value {
    let command = request.get("cmd").and_then(Value::as_str).unwrap_or("");
    if command == "ping" {
        return json!({ "ok": true, "version": 2 });
    }
    // The design commands read sources, not a dump, so they resolve their own
    // paths and skip the waveform `path` gate below.
    if matches!(command, "sources" | "modules" | "declare" | "diagnostics") {
        return design_command(command, request);
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

    /// A temporary directory of source fixtures, removed when the test ends.
    fn source_dir(tag: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wave-insight-dsh-design-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the test directory");
        for (name, text) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create the fixture's directory");
            }
            std::fs::write(&path, text).expect("write the fixture");
        }
        dir
    }

    const DESIGN: &str = "module leaf(input logic a, output logic y);\n  assign y = a;\nendmodule\n\nmodule top(input logic clk, output logic y);\n  logic mid;\n  leaf u_leaf(.a(clk), .y(mid));\n  assign y = mid;\nendmodule\n";

    #[test]
    fn the_modules_command_lists_definitions_and_their_ports() {
        let dir = source_dir("modules", &[("design.sv", DESIGN)]);
        let response = handle(&json!({
            "cmd": "modules",
            "paths": [dir.join("design.sv").display().to_string()],
        }));
        assert_eq!(response.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            response.get("module_count").and_then(Value::as_u64),
            Some(2)
        );
        let modules = response
            .get("modules")
            .and_then(Value::as_array)
            .expect("a module list");
        let top = modules
            .iter()
            .find(|module| module.get("name").and_then(Value::as_str) == Some("top"))
            .expect("the top module");
        let ports = top
            .get("module_ports")
            .and_then(Value::as_array)
            .expect("the port list");
        assert_eq!(ports.len(), 2, "{ports:?}");
        assert_eq!(ports[0].get("name").and_then(Value::as_str), Some("clk"));
        assert_eq!(
            ports[0].get("direction").and_then(Value::as_str),
            Some("input")
        );
        let instances = top
            .get("instances")
            .and_then(Value::as_array)
            .expect("instances");
        assert_eq!(instances.len(), 1);
        assert_eq!(
            instances[0].get("module").and_then(Value::as_str),
            Some("leaf")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_declare_command_locates_a_hierarchical_signal() {
        let dir = source_dir("declare", &[("design.sv", DESIGN)]);
        let path = dir.join("design.sv").display().to_string();
        let response = handle(&json!({
            "cmd": "declare",
            "paths": [path.clone()],
            "top": "top",
            "signal": "top.u_leaf.y",
        }));
        assert_eq!(response.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            response.get("declared").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            response.get("instance").and_then(Value::as_str),
            Some("top.u_leaf")
        );
        let declaration = response
            .get("declaration")
            .and_then(|span| span.get("file"))
            .and_then(Value::as_str)
            .expect("the declaration file");
        assert!(declaration.ends_with("design.sv"), "{declaration}");
        // a path the design does not know is an answer, not an error
        let missing = handle(&json!({
            "cmd": "declare",
            "paths": [path],
            "top": "top",
            "signal": "top.u_leaf.nope",
        }));
        assert_eq!(missing.get("ok").and_then(Value::as_bool), Some(true));
        assert_eq!(
            missing.get("declared").and_then(Value::as_bool),
            Some(false)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_diagnostics_command_reports_what_the_elaborator_could_not_resolve() {
        let dir = source_dir(
            "diagnostics",
            &[(
                "broken.sv",
                "module top(input logic clk);\n  assign y = missing_name;\nendmodule\n",
            )],
        );
        let response = handle(&json!({
            "cmd": "diagnostics",
            "paths": [dir.join("broken.sv").display().to_string()],
            "top": "top",
        }));
        assert_eq!(response.get("ok").and_then(Value::as_bool), Some(true));
        let diagnostics = response
            .get("diagnostics")
            .and_then(Value::as_array)
            .expect("a diagnostic list");
        assert!(!diagnostics.is_empty(), "{response}");
        assert!(diagnostics
            .iter()
            .all(|entry| entry.get("message").is_some()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_sources_command_finds_the_rtl_next_to_the_dump() {
        let dir = source_dir(
            "sources",
            &[("run/rtl/design.v", "module top; endmodule\n")],
        );
        std::fs::write(dir.join("run/test.vcd"), b"$enddefinitions $end\n")
            .expect("write a fake dump");
        let response = handle(&json!({
            "cmd": "sources",
            "dump": dir.join("run/test.vcd").display().to_string(),
        }));
        assert_eq!(response.get("ok").and_then(Value::as_bool), Some(true));
        let paths = response
            .get("paths")
            .and_then(Value::as_array)
            .expect("a path list");
        assert_eq!(paths.len(), 1, "{paths:?}");
        // a dump with nothing next to it answers with an empty list
        std::fs::remove_dir_all(dir.join("run/rtl")).expect("remove the rtl directory");
        let empty = handle(&json!({
            "cmd": "sources",
            "dump": dir.join("run/test.vcd").display().to_string(),
        }));
        assert_eq!(
            empty.get("paths").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_design_command_without_sources_answers_a_usable_error() {
        let missing = handle(&json!({ "cmd": "modules" }));
        assert_eq!(missing.get("ok").and_then(Value::as_bool), Some(false));
        let message = missing.get("error").and_then(Value::as_str).unwrap_or("");
        assert!(message.contains("paths"), "{message}");

        let unreadable = handle(&json!({
            "cmd": "modules",
            "paths": ["/no/such/file.sv"],
        }));
        assert_eq!(unreadable.get("ok").and_then(Value::as_bool), Some(false));
    }
}
