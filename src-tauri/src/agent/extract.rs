//! Streaming CSV extraction for the AI tools.
//!
//! `extract_changes` is the one tool that deliberately reads a whole range: an
//! assistant that needs "every pc value between t=1000 and t=2000" must not
//! pull thousands of changes through a JSON tool answer, and a 4 GB dump must
//! not be decoded into memory to do it. So the extraction streams: one
//! `stream_changes` pass, one row written at a time, a row/byte budget that
//! stops the pass, and a file the caller can cite.
//!
//! Two deliberate limits:
//!
//! * the output directory is the system temp directory under a fixed
//!   `wave-insight-agent` name - a tool that takes an arbitrary write path is a
//!   tool an assistant can use to overwrite files;
//! * the file is opened through a *fresh* `Waveform` rather than the cached
//!   agent dump: an extraction may take minutes, and it must not hold one of
//!   the four cache slots (or its mapping) hostage while it runs.

use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use wave_insight_lib::agent::{AgentDumpInfo, AgentSignalCount, AgentTimeRange, ExtractResult};
use wave_insight_lib::waveform::{ShowType, ValueQuery, Waveform};

/// Default row budget of one extraction.
pub const DEFAULT_MAX_ROWS: u64 = 2_000_000;
/// Hard row cap (a caller may ask for less, not more).
pub const MAX_MAX_ROWS: u64 = 20_000_000;
/// Default byte budget of one extraction.
pub const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// Hard byte cap.
pub const MAX_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// Rows repeated in the preview (head and tail each).
const PREVIEW_ROWS: usize = 5;

/// What one extraction was asked for.
#[derive(Clone, Debug)]
pub struct ExtractRequest {
    /// Absolute path of the dump.
    pub path: String,
    /// Hierarchical paths, already resolved by the caller.
    pub signals: Vec<(String, usize, u32)>,
    pub start: u64,
    pub end: u64,
    pub show: ShowType,
    /// The name the answer reports (`hex`, `bin`, ...).
    pub format: String,
    pub max_rows: u64,
    pub max_bytes: u64,
}

/// One extracted row, kept only for the preview.
#[derive(Clone, Debug)]
struct PreviewRow {
    text: String,
    /// Row ordinal from the start (0-based).
    index: u64,
}

/// Stream the selected signals of `[start, end]` into a CSV file.
///
/// The CSV is long format (`time,signal,value`), so one file holds any number
/// of signals and the signal name is a column instead of a fixed header. A
/// sidecar `.meta.json` records what the file is and how it was cut, because
/// the assistant may read it much later.
pub fn extract(request: &ExtractRequest) -> Result<ExtractResult, String> {
    if request.signals.is_empty() {
        return Err("give at least one signal to extract".to_string());
    }
    if request.start > request.end {
        return Err(format!(
            "the range is empty: start {} is after end {}",
            request.start, request.end
        ));
    }
    let path = Path::new(&request.path);
    // the same mmap/read fallback the registry uses: extraction on a huge FST
    // must not copy the file either
    let waveform = super::open_for_read(path)?;
    let info = dump_info(&waveform, path);
    let output_dir = output_dir()?;
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wave".to_string());
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or(0);
    let output_path = output_dir.join(format!("{stem}-{stamp}.csv"));
    let meta_path = output_dir.join(format!("{stem}-{stamp}.meta.json"));

    let file = std::fs::File::create(&output_path)
        .map_err(|error| format!("{}: {error}", output_path.display()))?;
    let mut writer = BufWriter::new(file);
    let mut rows = 0u64;
    let mut bytes = 0u64;
    let mut truncated = false;
    let mut per_signal: Vec<AgentSignalCount> = request
        .signals
        .iter()
        .map(|(name, _, _)| AgentSignalCount {
            signal: name.clone(),
            rows: 0,
        })
        .collect();
    let mut first_time: Option<u64> = None;
    let mut last_time: Option<u64> = None;
    let mut head: Vec<PreviewRow> = Vec::with_capacity(PREVIEW_ROWS);
    let mut tail: Vec<PreviewRow> = Vec::with_capacity(PREVIEW_ROWS);
    // name per index, so the visitor does not walk the request for every change
    let mut names: Vec<(usize, &str, u32)> = request
        .signals
        .iter()
        .map(|(name, index, width)| (*index, name.as_str(), *width))
        .collect();
    names.sort_unstable_by_key(|(index, _, _)| *index);

    let header = bytes_written(&mut writer, "time,signal,value\n")?;
    bytes += header;
    let mut failure: Option<String> = None;
    let query = ValueQuery::for_signals(request.signals.iter().map(|(_, index, _)| *index))
        .between(request.start, Some(request.end));
    let streamed = waveform.stream_changes(&query, |signal, time, value| {
        if rows >= request.max_rows || bytes >= request.max_bytes {
            truncated = true;
            return false;
        }
        let Some((_, name, width)) = names.iter().find(|(index, _, _)| *index == signal) else {
            return true;
        };
        let rendered = crate::dumps::render_value(&value, *width, &request.show);
        let line = format!("{time},{},{}\n", escape(name), escape(&rendered));
        match writer.write_all(line.as_bytes()) {
            Ok(()) => {
                bytes += line.len() as u64;
                if let Some(count) = per_signal.iter_mut().find(|count| count.signal == *name) {
                    count.rows += 1;
                }
                first_time.get_or_insert(time);
                last_time = Some(time);
                if head.len() < PREVIEW_ROWS {
                    head.push(PreviewRow {
                        text: line.trim_end().to_string(),
                        index: rows,
                    });
                }
                tail.push(PreviewRow {
                    text: line.trim_end().to_string(),
                    index: rows,
                });
                if tail.len() > PREVIEW_ROWS {
                    tail.remove(0);
                }
                rows += 1;
                true
            }
            Err(error) => {
                failure = Some(format!("{}: {error}", output_path.display()));
                false
            }
        }
    });
    if let Some(error) = failure {
        drop(writer);
        let _ = std::fs::remove_file(&output_path);
        return Err(error);
    }
    streamed
        .map_err(|error| format!("extract_changes: {}: {}", path.display(), error.message()))?;
    writer
        .flush()
        .map_err(|error| format!("{}: {error}", output_path.display()))?;
    drop(writer);
    if rows == 0 {
        // an empty file with a header is more confusing than no file
        let _ = std::fs::remove_file(&output_path);
        return Err(format!(
            "no changes of the selected signals fall inside {}..={}",
            request.start, request.end
        ));
    }
    let meta = serde_json::json!({
        "dump": info,
        "start": request.start,
        "end": request.end,
        "format": request.format,
        "rows": rows,
        "bytes": bytes,
        "truncated": truncated,
        "per_signal": per_signal,
        "first_time": first_time,
        "last_time": last_time,
        "csv": output_path.display().to_string(),
        "columns": "time,signal,value",
    });
    std::fs::write(
        &meta_path,
        serde_json::to_vec_pretty(&meta)
            .map_err(|error| format!("could not serialize the extraction metadata: {error}"))?,
    )
    .map_err(|error| format!("{}: {error}", meta_path.display()))?;

    let mut preview: Vec<String> = head.into_iter().map(|row| row.text).collect();
    let tail: Vec<String> = tail
        .into_iter()
        .filter(|row| row.index as usize >= preview.len())
        .map(|row| row.text)
        .collect();
    preview.extend(tail);
    Ok(ExtractResult {
        dump: info,
        start: request.start,
        end: request.end,
        format: request.format.clone(),
        output_path: output_path.display().to_string(),
        meta_path: meta_path.display().to_string(),
        rows,
        bytes,
        truncated,
        per_signal,
        first_time,
        last_time,
        preview,
        note: None,
    })
}

/// The time range an extraction defaults to is resolved by the caller from the
/// dump metadata; the waveform itself is only consulted when the format cannot
/// report it (GHW).
fn dump_info(waveform: &Waveform, path: &Path) -> AgentDumpInfo {
    AgentDumpInfo {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        path: Some(path.display().to_string()),
        format: waveform.file_format().name().to_string(),
        timescale: waveform.timescale().map(|timescale| timescale.to_string()),
        time_range: waveform
            .time_range_hint()
            .ok()
            .flatten()
            .map(|range| AgentTimeRange {
                start: range.start,
                end: range.end,
            }),
        signal_count: waveform.signal_count(),
    }
}

fn bytes_written(writer: &mut BufWriter<std::fs::File>, text: &str) -> Result<u64, String> {
    writer
        .write_all(text.as_bytes())
        .map_err(|error| format!("could not write the CSV header: {error}"))?;
    Ok(text.len() as u64)
}

/// Quote a CSV field the way RFC 4180 asks for.
fn escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn output_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("wave-insight-agent");
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wave_insight_lib::waveform::Waveform;

    const SMALL_VCD: &str = "$date test $end\n$version wave-insight extract test $end\n$timescale 1ns $end\n$scope module top $end\n$var wire 1 ! clk $end\n$var wire 4 \" data [3:0] $end\n$upscope $end\n$enddefinitions $end\n$dumpvars\n0!\nb0000 \"\n$end\n#10\n1!\nb1010 \"\n#20\n0!\nb0101 \"\n#30\n1!\nb1111 \"\n";

    fn fixture(name: &str, text: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("wave-insight-agent-extract")
            .join(format!("{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.vcd"));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn request(path: &Path, signals: Vec<(&str, usize, u32)>) -> ExtractRequest {
        ExtractRequest {
            path: path.display().to_string(),
            signals: signals
                .into_iter()
                .map(|(name, index, width)| (name.to_string(), index, width))
                .collect(),
            start: 0,
            end: 30,
            show: ShowType::Hex,
            format: "hex".to_string(),
            max_rows: DEFAULT_MAX_ROWS,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// Resolve the fixture's signals so the test uses the dump's own indices.
    fn signals(path: &Path) -> Vec<(&'static str, usize, u32)> {
        let waveform = Waveform::open_path(path).unwrap();
        let mut found = Vec::new();
        for name in ["top.clk", "top.data"] {
            let info = waveform.signal_by_path(name).unwrap();
            found.push((
                if name == "top.clk" {
                    "top.clk"
                } else {
                    "top.data"
                },
                info.index(),
                info.width(),
            ));
        }
        found
    }

    #[test]
    fn an_extraction_writes_a_long_csv_and_its_metadata() {
        let path = fixture("full", SMALL_VCD);
        let request = request(&path, signals(&path));
        let result = extract(&request).unwrap();
        assert!(!result.truncated);
        assert_eq!(
            result.rows,
            result
                .per_signal
                .iter()
                .map(|count| count.rows)
                .sum::<u64>()
        );
        // clk: 2 changes at t=0 (dumpvars and #0?), data was declared once
        assert!(result.per_signal[1].rows >= 4, "{:?}", result.per_signal);
        assert_eq!(result.first_time, Some(0));
        assert_eq!(result.last_time, Some(30));
        let text = std::fs::read_to_string(&result.output_path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "time,signal,value");
        assert_eq!(lines.len() as u64, result.rows + 1);
        // every row is `time,signal,value` with the signal named in the file
        assert!(lines[1].starts_with("0,"), "{:?}", lines[1]);
        assert!(lines.iter().any(|line| line.contains("top.data")));
        // the preview repeats the head (and the tail when the file is longer)
        assert!(!result.preview.is_empty());
        assert_eq!(result.preview[0], lines[1]);
        // the sidecar says what the CSV is
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&result.meta_path).unwrap()).unwrap();
        assert_eq!(meta["rows"], result.rows);
        assert_eq!(meta["columns"], "time,signal,value");
        assert_eq!(meta["format"], "hex");
        let _ = std::fs::remove_file(&result.output_path);
        let _ = std::fs::remove_file(&result.meta_path);
    }

    #[test]
    fn a_range_limits_the_rows() {
        let path = fixture("range", SMALL_VCD);
        let mut request = request(&path, vec![signals(&path)[0]]);
        request.start = 10;
        request.end = 20;
        let result = extract(&request).unwrap();
        let text = std::fs::read_to_string(&result.output_path).unwrap();
        for line in text.lines().skip(1) {
            let time: u64 = line.split(',').next().unwrap().parse().unwrap();
            assert!((10..=20).contains(&time), "{line} is outside the range");
        }
        assert_eq!(result.last_time, Some(20));
        let _ = std::fs::remove_file(&result.output_path);
        let _ = std::fs::remove_file(&result.meta_path);
    }

    #[test]
    fn a_row_budget_stops_the_stream_and_says_so() {
        let path = fixture("budget", SMALL_VCD);
        let mut request = request(&path, signals(&path));
        request.max_rows = 2;
        let result = extract(&request).unwrap();
        assert!(result.truncated);
        assert_eq!(result.rows, 2);
        // both signals change at t=0 (dumpvars), so the budget lands there
        assert_eq!(result.last_time, Some(0));
        let text = std::fs::read_to_string(&result.output_path).unwrap();
        assert_eq!(text.lines().count(), 3, "header plus two rows");
        let _ = std::fs::remove_file(&result.output_path);
        let _ = std::fs::remove_file(&result.meta_path);
    }

    #[test]
    fn an_empty_range_is_an_error_instead_of_an_empty_file() {
        let path = fixture("empty", SMALL_VCD);
        let mut request = request(&path, vec![signals(&path)[0]]);
        request.start = 100;
        request.end = 200;
        let error = extract(&request).unwrap_err();
        assert!(error.contains("no changes"), "{error}");
        // and the file it would have written is gone
        let entries = std::fs::read_dir(output_dir().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("empty-"))
            .count();
        assert_eq!(entries, 0, "the half-written CSV was removed");
    }

    #[test]
    fn a_reversed_range_is_rejected_before_any_io() {
        let path = fixture("reversed", SMALL_VCD);
        let mut reversed = request(&path, vec![signals(&path)[0]]);
        reversed.start = 20;
        reversed.end = 10;
        let error = extract(&reversed).unwrap_err();
        assert!(error.contains("after end"), "{error}");
        assert!(extract(&request(&path, vec![])).is_err());
    }

    #[test]
    fn csv_fields_are_quoted_only_when_they_need_it() {
        assert_eq!(escape("top.pc"), "top.pc");
        assert_eq!(escape("0x1"), "0x1");
        assert_eq!(escape("a,b"), "\"a,b\"");
        assert_eq!(escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(escape("two\nlines"), "\"two\nlines\"");
    }
}
