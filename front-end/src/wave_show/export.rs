//! Exporting the values of wave rows as a downloaded CSV/TXT file.
//!
//! Two shapes, both produced from the *decoded* histories the renderer already
//! caches ([`DecodedSignal`]):
//!
//! * one row — `time,value` CSV (or `<time> <value>` TXT), one line per change;
//! * every row — long format `time,signal,value`, produced by a k-way merge of
//!   the rows' sorted histories so the file is ordered by time.
//!
//! The value column goes through the same [`crate::nav::value_text`] path as the
//! screen (base format *and* fixed point), so the file matches what the user
//! sees, and an `x`/`z` bit is written as the text it already renders as.
//!
//! Everything except [`download`] is pure and unit tested on the host target;
//! the download itself needs a browser (`window`/`Blob`/`<a download>`) and is
//! only exercised there.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wave_insight_lib::waveform::{Change, DecodedSignal, WaveValue};

use crate::nav::{value_text, ValueFormat};

/// Refuse to build more than this much text: a string that large already
/// costs its size twice (the builder plus the Blob), and the tab would die
/// without a usable error.
pub const MAX_EXPORT_BYTES: usize = 200 * 1024 * 1024;

/// Which file format an export produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFormat {
    /// `time,value` (one row) or `time,signal,value` (all rows).
    Csv,
    /// `<time> <value>` per line; one row only.
    Txt,
    /// MATLAB level 5 binary: two double columns (`wave_time` and the
    /// signal) for one row. `x`/`z` bits become `NaN`, because a MAT double
    /// cannot carry an unknown bit.
    Mat,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Txt => "txt",
            ExportFormat::Mat => "mat",
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            ExportFormat::Csv => "text/csv;charset=utf-8",
            ExportFormat::Txt => "text/plain;charset=utf-8",
            ExportFormat::Mat => "application/x-matlab-data",
        }
    }

    /// `true` when the file is binary (the caller then downloads bytes, not
    /// a string).
    pub fn is_binary(self) -> bool {
        matches!(self, ExportFormat::Mat)
    }
}

/// What the export action covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportScope {
    /// The row the settings dialog was opened for.
    Row,
    /// Every row of the wave view, in row order.
    AllRows,
}

/// Why an export was refused. The message is what the status line shows.
///
/// "No decoded values" is reported by the caller that owns the row list, so the
/// only refusal the builders can produce is the size guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// The generated text would pass the size limit.
    TooLarge { limit: usize },
}

impl ExportError {
    pub fn message(self) -> String {
        match self {
            ExportError::TooLarge { limit } => format!(
                "export refused: the file would exceed {} — narrow the selection or export one row",
                format_bytes(limit)
            ),
        }
    }
}

/// One row's exportable history.
pub struct ExportRow<'a> {
    /// Display name (`top.pc[7:0]`); the `signal` column of the long format.
    pub name: String,
    /// The row's current format, fixed point included.
    pub format: ValueFormat,
    /// Bit width the values are formatted with.
    pub width: usize,
    /// The decoded history, sorted by time.
    pub changes: &'a [Change],
}

// -- MATLAB level 5 -------------------------------------------------------

/// Numeric value of one change for the MAT export.
///
/// MAT has no unknown bit: an `x`/`z` bit, a string or an event becomes `NaN`.
/// A signed row is sign-extended like the screen readout and an active
/// fixed-point exponent divides by `2^exp`, so the numbers match the value
/// column.
fn mat_value(value: &WaveValue, format: &ValueFormat) -> f64 {
    let number = match value {
        WaveValue::Real(number) => *number,
        other => {
            let Some(bits) = other.as_bits() else {
                return f64::NAN;
            };
            let Some(raw) = bits.as_u64() else {
                return f64::NAN;
            };
            let width = bits.width();
            let signed = format.show == wave_insight_lib::waveform::ShowType::SInt
                && (1..=63).contains(&width);
            if signed {
                let modulus = 1i64 << width;
                let value = if raw & (1u64 << (width - 1)) != 0 {
                    raw as i64 - modulus
                } else {
                    raw as i64
                };
                value as f64
            } else {
                raw as f64
            }
        }
    };
    match format.fixed_point {
        Some(exponent) => number / 2f64.powi(exponent.min(63) as i32),
        None => number,
    }
}

/// One MAT-file data element: `[u32 type][u32 size][payload padded to 8]`.
///
/// The small-data-element form (payload of 4 bytes or fewer folded into the
/// tag) is never produced here: every payload this writer emits is larger than
/// that, including the variable names (see [`mat_variable_name`]).
fn mat_element(out: &mut Vec<u8>, kind: u32, payload: &[u8]) {
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    let padding = (8 - payload.len() % 8) % 8;
    out.extend(std::iter::repeat_n(0u8, padding));
}

/// One MATLAB variable: an `n x 1` `double` matrix inside a MAT v5 file.
fn mat_matrix(out: &mut Vec<u8>, name: &str, values: &[f64]) {
    let mut content = Vec::new();
    // array flags: mxDOUBLE_CLASS (6) in the high byte, nzmax 0
    let mut flags = Vec::with_capacity(8);
    flags.extend_from_slice(&(6u32 << 8).to_le_bytes());
    flags.extend_from_slice(&0u32.to_le_bytes());
    mat_element(&mut content, 6, &flags);
    // dimensions
    let mut dimensions = Vec::with_capacity(8);
    dimensions.extend_from_slice(&(values.len() as u32).to_le_bytes());
    dimensions.extend_from_slice(&1u32.to_le_bytes());
    mat_element(&mut content, 5, &dimensions);
    // array name (miINT8)
    mat_element(&mut content, 1, name.as_bytes());
    // the real part
    let mut data = Vec::with_capacity(values.len() * 8);
    for value in values {
        data.extend_from_slice(&value.to_le_bytes());
    }
    mat_element(&mut content, 9, &data);
    // the matrix element itself
    out.extend_from_slice(&14u32.to_le_bytes());
    out.extend_from_slice(&(content.len() as u32).to_le_bytes());
    out.extend_from_slice(&content);
}

/// A MATLAB-safe variable name for a signal display name.
///
/// Letters, digits and `_` only, starting with a letter, at most 63 characters
/// (`top.pc[7:0]` becomes `sig_top_pc_7_0`). It is always longer than four bytes
/// so the writer never has to use the small-element tag form.
pub fn mat_variable_name(display: &str) -> String {
    let mut name = String::with_capacity(display.len() + 4);
    name.push_str("sig_");
    for character in display.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            name.push(character);
        } else {
            name.push('_');
        }
        if name.len() >= 63 {
            break;
        }
    }
    name
}

/// One row as a MATLAB level-5 `.mat` file.
///
/// Two variables: `wave_time` (double, seconds as written by the dump) and the
/// sanitized signal name (double, see [`mat_value`]); both are `n x 1` so
/// `plot(wave_time, sig_top_pc_7_0)` works in MATLAB/Octave. The file is written
/// little-endian (`IM`) with version `0x0100`.
pub fn row_mat(row: &ExportRow<'_>) -> Result<Vec<u8>, ExportError> {
    let estimated = row.changes.len().saturating_mul(16).saturating_add(1024);
    if estimated > MAX_EXPORT_BYTES {
        return Err(ExportError::TooLarge {
            limit: MAX_EXPORT_BYTES,
        });
    }
    let mut out = vec![0u8; 128];
    let header = b"MATLAB 5.0 MAT-file, Wave Insight";
    out[..header.len()].copy_from_slice(header);
    // subsystem data offset stays 0; version 0x0100 little endian, then `IM`
    out[124] = 0x00;
    out[125] = 0x01;
    out[126] = b'I';
    out[127] = b'M';
    let times: Vec<f64> = row.changes.iter().map(|(time, _)| *time as f64).collect();
    let values: Vec<f64> = row
        .changes
        .iter()
        .map(|(_, value)| mat_value(value, &row.format))
        .collect();
    mat_matrix(&mut out, "wave_time", &times);
    mat_matrix(&mut out, &mat_variable_name(&row.name), &values);
    Ok(out)
}

/// One row as `time,value` CSV, one line per change.
pub fn row_csv(row: &ExportRow<'_>) -> Result<String, ExportError> {
    row_csv_with_limit(row, MAX_EXPORT_BYTES)
}

/// [`row_csv`] with an explicit limit, so the guard is testable without
/// building 200 MB.
pub fn row_csv_with_limit(row: &ExportRow<'_>, limit: usize) -> Result<String, ExportError> {
    let mut out = String::with_capacity(16 + row.changes.len().saturating_mul(24));
    out.push_str("time,value\n");
    for (time, value) in row.changes {
        push_cell(&mut out, &time.to_string());
        out.push(',');
        push_value_cell(&mut out, value, row);
        out.push('\n');
        guard(&out, limit)?;
    }
    Ok(out)
}

/// One row as TXT: `<time> <value>` per line.
pub fn row_txt(row: &ExportRow<'_>) -> Result<String, ExportError> {
    row_txt_with_limit(row, MAX_EXPORT_BYTES)
}

/// [`row_txt`] with an explicit limit.
pub fn row_txt_with_limit(row: &ExportRow<'_>, limit: usize) -> Result<String, ExportError> {
    let mut out = String::with_capacity(16 + row.changes.len().saturating_mul(20));
    for (time, value) in row.changes {
        out.push_str(&time.to_string());
        out.push(' ');
        if let Some(text) = value_text(value, row.width, &row.format) {
            out.push_str(&text);
        }
        out.push('\n');
        guard(&out, limit)?;
    }
    Ok(out)
}

/// Every row as long-format `time,signal,value` CSV.
///
/// The rows are merged by time (a k-way merge over the already sorted
/// histories), ties broken by row order and then by the order inside the row,
/// so two runs over the same dump produce the same bytes.
pub fn rows_csv(rows: &[ExportRow<'_>]) -> Result<String, ExportError> {
    rows_csv_with_limit(rows, MAX_EXPORT_BYTES)
}

/// [`rows_csv`] with an explicit limit.
pub fn rows_csv_with_limit(rows: &[ExportRow<'_>], limit: usize) -> Result<String, ExportError> {
    let mut out = String::with_capacity(32);
    out.push_str("time,signal,value\n");
    // (time, row, index) in a min-heap: the smallest time first, then the
    // lowest row index, then the earliest change of that row
    let mut heap: BinaryHeap<Reverse<(u64, usize, usize)>> = BinaryHeap::new();
    for (row_index, row) in rows.iter().enumerate() {
        if let Some((time, _)) = row.changes.first() {
            heap.push(Reverse((*time, row_index, 0)));
        }
    }
    while let Some(Reverse((time, row_index, change_index))) = heap.pop() {
        let row = &rows[row_index];
        let Some((_, value)) = row.changes.get(change_index) else {
            continue;
        };
        push_cell(&mut out, &time.to_string());
        out.push(',');
        push_cell(&mut out, &row.name);
        out.push(',');
        push_value_cell(&mut out, value, row);
        out.push('\n');
        guard(&out, limit)?;
        let next = change_index + 1;
        if let Some((next_time, _)) = row.changes.get(next) {
            heap.push(Reverse((*next_time, row_index, next)));
        }
    }
    Ok(out)
}

/// The export stem for a dump file name (`test3.vcd` -> `test3`).
///
/// The name can arrive as a bare file name (browser picker) or as a path
/// (desktop dialog); both separators are stripped without touching the
/// platform, and the result is sanitized like every other file-name part.
pub fn dump_stem_name(file_name: &str) -> String {
    let base = file_name.rsplit(['/', '\\']).next().unwrap_or(file_name);
    let stem = match base.rsplit_once('.') {
        Some((stem, _extension)) if !stem.is_empty() => stem,
        _ => base,
    };
    let stem = sanitize_file_part(stem);
    if stem.is_empty() {
        "waveform".to_string()
    } else {
        stem
    }
}

/// The file name a download should use.
///
/// `<dump-stem>-<signal>.<ext>`, or `<dump-stem>-rows.<ext>` for the whole view.
/// Signal display names contain `[7:0]`, and a `:` cannot appear in a Windows
/// file name, so both parts are sanitized ([`sanitize_file_part`]).
pub fn file_name(dump_stem: &str, signal: Option<&str>, format: ExportFormat) -> String {
    let stem = sanitize_file_part(dump_stem);
    match signal {
        Some(signal) => {
            format!(
                "{stem}-{}.{}",
                sanitize_file_part(signal),
                format.extension()
            )
        }
        None => format!("{stem}-rows.{}", format.extension()),
    }
}

/// `1,204` — a change count a human reads at a glance.
pub fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `34 KB` — the generated size, rounded the way a file manager shows it.
pub fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let size = bytes as f64;
    if size < KB {
        format!("{bytes} B")
    } else if size < MB {
        format!("{:.0} KB", size / KB)
    } else {
        format!("{:.1} MB", size / MB)
    }
}

/// The success line: `exported top.pc[7:0]: 1,204 changes, 34 KB`.
pub fn row_status(name: &str, changes: usize, bytes: usize) -> String {
    format!(
        "exported {name}: {} changes, {}",
        format_count(changes),
        format_bytes(bytes)
    )
}

/// The success line for the whole view: `exported 12 rows: 1,204 changes, 34 KB`.
pub fn rows_status(rows: usize, changes: usize, bytes: usize) -> String {
    format!(
        "exported {} rows: {} changes, {}",
        format_count(rows),
        format_count(changes),
        format_bytes(bytes)
    )
}

/// Trigger the browser download of `text` as `file_name`.
///
/// The only DOM-touching function of the module: it builds a `Blob`, hands it
/// to an object URL and clicks a detached `<a download>`. It cannot be covered
/// by the host tests (there is no `window` there); the browser QA run is the
/// only place this path is exercised.
pub fn download(file_name: &str, mime: &str, text: &str) -> Result<(), String> {
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let parts = js_sys::Array::new();
    parts.push(&wasm_bindgen::JsValue::from_str(text));
    let blob = web_sys::Blob::new_with_str_sequence_and_options(&parts, &options)
        .map_err(|_| "the browser refused to build the file".to_string())?;
    save_blob(file_name, &blob)
}

/// Download raw bytes (the MAT writer produces a binary file).
pub fn download_bytes(file_name: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let array = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(bytes);
    let parts = js_sys::Array::new();
    parts.push(&array.buffer());
    let blob = web_sys::Blob::new_with_buffer_source_sequence_and_options(&parts, &options)
        .map_err(|_| "the browser refused to build the file".to_string())?;
    save_blob(file_name, &blob)
}

/// Attach a blob to a temporary `<a download>` and click it.
fn save_blob(file_name: &str, blob: &web_sys::Blob) -> Result<(), String> {
    let window = web_sys::window().ok_or("the browser window is gone")?;
    let document = window.document().ok_or("the page has no document")?;
    let url = web_sys::Url::create_object_url_with_blob(blob)
        .map_err(|_| "the browser refused to create a download link".to_string())?;

    let anchor = document
        .create_element("a")
        .map_err(|_| "could not create the download link".to_string())?
        .dyn_into::<web_sys::HtmlAnchorElement>()
        .map_err(|_| "could not create the download link".to_string())?;
    anchor.set_href(&url);
    anchor.set_download(file_name);
    // Some browsers ignore a click on a detached anchor; the body is the
    // cheapest place to attach it for the duration of the click.
    let body = document.body();
    if let Some(body) = &body {
        let _ = body.append_child(&anchor);
    }
    anchor.click();
    if let Some(body) = &body {
        let _ = body.remove_child(&anchor);
    }

    // Revoking immediately can cancel the download in some browsers, so let the
    // navigation start first. `once_into_js` hands the closure to JS without
    // keeping Rust memory alive, and the string is cloned because the closure
    // must own it.
    let url_for_revoke = url.clone();
    let revoke = Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url_for_revoke);
    });
    if window
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            revoke.unchecked_ref::<js_sys::Function>(),
            1_000,
        )
        .is_err()
    {
        // no timers (a headless harness?): at least do not leak the URL
        let _ = web_sys::Url::revoke_object_url(&url);
    }
    Ok(())
}

// -- helpers ---------------------------------------------------------------

fn push_cell(out: &mut String, text: &str) {
    out.push_str(&csv_field(text));
}

/// Append one value as a CSV cell; an event or a row with no value is empty.
fn push_value_cell(out: &mut String, value: &WaveValue, row: &ExportRow<'_>) {
    if let Some(text) = value_text(value, row.width, &row.format) {
        out.push_str(&csv_field(&text));
    }
}

fn guard(out: &str, limit: usize) -> Result<(), ExportError> {
    if out.len() > limit {
        Err(ExportError::TooLarge { limit })
    } else {
        Ok(())
    }
}

/// One CSV field: quoted only when it has to be, RFC 4180 style.
pub fn csv_field(text: &str) -> String {
    let needs_quotes = text
        .chars()
        .any(|character| matches!(character, ',' | '"' | '\n' | '\r'));
    if !needs_quotes {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        if character == '"' {
            out.push('"');
        }
        out.push(character);
    }
    out.push('"');
    out
}

/// A file-name part that no platform refuses: control characters and the
/// Windows-reserved set become `_`, and a trailing dot/space is dropped.
pub fn sanitize_file_part(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|character| {
            let reserved = matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            );
            if reserved || character.is_control() {
                '_'
            } else {
                character
            }
        })
        .collect();
    while matches!(out.chars().last(), Some('.' | ' ')) {
        out.pop();
    }
    if out.is_empty() {
        "waveform".to_string()
    } else {
        out
    }
}

/// The stem a dump's export files are named after.
///
/// A dump opened from disk uses its file stem; a browser dump (and the FST
/// fixture) is opened from bytes, so it falls back to the root scope name and
/// finally to `waveform`.
pub fn dump_stem(waveform: Option<&wave_insight_lib::waveform::Waveform>) -> String {
    let Some(waveform) = waveform else {
        return "waveform".to_string();
    };
    if let Some(stem) = waveform
        .path()
        .and_then(|path| path.file_stem())
        .and_then(|stem| stem.to_str())
    {
        if !stem.is_empty() {
            return stem.to_string();
        }
    }
    let root = waveform.hierarchy().root().name();
    if root.is_empty() {
        "waveform".to_string()
    } else {
        root.to_string()
    }
}

/// A decoded history for one row, borrowing the cached handle when the
/// renderer already decoded it.
///
/// Kept here (and not in the component) so the "cached or decode" rule has one
/// name; the caller passes the handle the renderer holds.
pub fn decoded_or(
    cached: Option<&DecodedSignal>,
    decode: impl FnOnce() -> Option<DecodedSignal>,
) -> Option<DecodedSignal> {
    cached.cloned().or_else(decode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wave_insight_lib::waveform::ShowType;
    use wave_insight_lib::waveform::{BitValue, Bits};

    #[test]
    fn the_export_stem_comes_from_the_file_name() {
        assert_eq!(dump_stem_name("test3.vcd"), "test3");
        assert_eq!(dump_stem_name(r"C:\dumps\test3.fst"), "test3");
        assert_eq!(dump_stem_name("/home/u/run.ghw"), "run");
        assert_eq!(dump_stem_name("weird:name.vcd"), "weird_name");
        assert_eq!(dump_stem_name("no_extension"), "no_extension");
        assert_eq!(dump_stem_name("a.b.vcd"), "a.b");
        assert_eq!(dump_stem_name(""), "waveform");
    }

    /// A bit vector with the given raw value, `width` bits wide.
    fn bits(raw: u64, width: usize) -> WaveValue {
        WaveValue::Bits(Bits::new_msb_first(
            (0..width)
                .rev()
                .map(|bit| {
                    if raw & (1u64 << bit) != 0 {
                        BitValue::One
                    } else {
                        BitValue::Zero
                    }
                })
                .collect(),
        ))
    }

    fn row<'a>(name: &str, changes: &'a [Change]) -> ExportRow<'a> {
        ExportRow {
            name: name.to_string(),
            format: ValueFormat::base(ShowType::Hex),
            width: 8,
            changes,
        }
    }

    #[test]
    fn csv_fields_are_quoted_only_when_they_have_to_be() {
        assert_eq!(csv_field("'h2a"), "'h2a");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field(""), "");
    }

    #[test]
    fn one_row_csv_has_the_header_and_one_line_per_change() {
        let changes = [(0u64, bits(0, 8)), (10, bits(0x2a, 8)), (10, bits(0x2b, 8))];
        let csv = row_csv(&row("top.pc", &changes)).expect("fits");
        assert_eq!(csv, "time,value\n0,'h00\n10,'h2a\n10,'h2b\n");
    }

    #[test]
    fn unknown_bits_are_written_as_the_x_and_z_text_they_render_as() {
        // a one bit signal keeps the bare `x`/`z` the screen shows
        let x = WaveValue::Bits(Bits::new_msb_first(vec![BitValue::X]));
        let z = WaveValue::Bits(Bits::new_msb_first(vec![BitValue::Z]));
        let changes = [(0u64, x), (5, z)];
        let mut one_bit = row("top.reset_n", &changes);
        one_bit.width = 1;
        let csv = row_csv(&one_bit).expect("fits");
        assert_eq!(csv, "time,value\n0,x\n5,z\n");
    }

    #[test]
    fn the_value_column_uses_the_rows_fixed_point_format() {
        let changes = [(0u64, bits(0x10, 8))];
        let mut fixed = row("top.q", &changes);
        fixed.format = ValueFormat {
            show: ShowType::Hex,
            fixed_point: Some(4),
        };
        let csv = row_csv(&fixed).expect("fits");
        // 0x10 / 2^4 = 1.0, the same text the value column shows
        assert_eq!(csv, "time,value\n0,1.0\n");
        // ... and a string with a comma stays a valid CSV field
        let changes = [(3u64, WaveValue::String("a,b".into()))];
        let csv = row_csv(&row("top.msg", &changes)).expect("fits");
        assert_eq!(csv, "time,value\n3,\"a,b\"\n");
    }

    // -- MATLAB export -----------------------------------------------------

    /// Minimal MAT v5 reader for the bytes [`row_mat`] writes: `(name, values)`
    /// for every top-level matrix.
    fn read_mat(bytes: &[u8]) -> Vec<(String, Vec<f64>)> {
        fn tag(bytes: &[u8], offset: &mut usize) -> (u32, usize) {
            let kind = u32::from_le_bytes(bytes[*offset..*offset + 4].try_into().expect("tag"));
            let size =
                u32::from_le_bytes(bytes[*offset + 4..*offset + 8].try_into().expect("size"));
            *offset += 8;
            (kind, size as usize)
        }
        fn pad(size: usize) -> usize {
            (8 - size % 8) % 8
        }
        assert_eq!(&bytes[124..126], &[0x00, 0x01], "version 0x0100");
        assert_eq!(&bytes[126..128], b"IM", "little endian");
        let mut offset = 128;
        let mut out = Vec::new();
        while offset < bytes.len() {
            let (kind, size) = tag(bytes, &mut offset);
            assert_eq!(kind, 14, "miMATRIX");
            let end = offset + size;
            let (flags_kind, flags_size) = tag(bytes, &mut offset);
            assert_eq!((flags_kind, flags_size), (6, 8));
            let class =
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("class")) >> 8;
            assert_eq!(class, 6, "mxDOUBLE_CLASS");
            offset += 8;
            let (dims_kind, dims_size) = tag(bytes, &mut offset);
            assert_eq!((dims_kind, dims_size), (5, 8));
            let rows =
                u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("rows")) as usize;
            offset += 8;
            let (name_kind, name_size) = tag(bytes, &mut offset);
            assert_eq!(name_kind, 1, "miINT8 name");
            let name =
                String::from_utf8(bytes[offset..offset + name_size].to_vec()).expect("utf-8 name");
            offset += name_size + pad(name_size);
            let (data_kind, data_size) = tag(bytes, &mut offset);
            assert_eq!(data_kind, 9, "miDOUBLE data");
            let mut values = Vec::with_capacity(data_size / 8);
            for index in 0..data_size / 8 {
                let start = offset + index * 8;
                values.push(f64::from_le_bytes(
                    bytes[start..start + 8].try_into().expect("value"),
                ));
            }
            offset += data_size + pad(data_size);
            assert_eq!(offset, end, "the element size covers its payload");
            assert_eq!(values.len(), rows);
            out.push((name, values));
        }
        out
    }

    #[test]
    fn a_row_exports_as_a_mat_v5_file_with_time_and_value() {
        let changes = [
            (0u64, bits(0x00, 8)),
            (10, bits(0x2a, 8)),
            (20, bits(0xff, 8)),
        ];
        let bytes = row_mat(&row("top.pc[7:0]", &changes)).expect("fits");
        assert!(
            bytes.len() > 128 && bytes.len().is_multiple_of(8),
            "padded to 8 bytes"
        );
        let variables = read_mat(&bytes);
        assert_eq!(variables.len(), 2);
        assert_eq!(
            variables[0],
            ("wave_time".to_string(), vec![0.0, 10.0, 20.0])
        );
        assert_eq!(variables[1].0, "sig_top_pc_7_0_");
        assert_eq!(variables[1].1, vec![0.0, 42.0, 255.0]);
    }

    #[test]
    fn mat_values_are_signed_and_scaled_like_the_screen_and_unknown_bits_are_nan() {
        let x = WaveValue::Bits(Bits::new_msb_first(vec![BitValue::X, BitValue::X]));
        let changes = [(0u64, bits(0xf0, 8)), (5, x)];
        let mut signed = row("top.q", &changes);
        signed.format = ValueFormat {
            show: ShowType::SInt,
            fixed_point: None,
        };
        let bytes = row_mat(&signed).expect("fits");
        let variables = read_mat(&bytes);
        assert_eq!(variables[1].1[0], -16.0, "0xf0 as a signed 8 bit value");
        assert!(variables[1].1[1].is_nan(), "an unknown bit has no number");
        // fixed point scales the number, like the value column
        let changes = [(0u64, bits(0x10, 8))];
        let mut fixed = row("top.q", &changes);
        fixed.format = ValueFormat {
            show: ShowType::Hex,
            fixed_point: Some(4),
        };
        let bytes = row_mat(&fixed).expect("fits");
        let variables = read_mat(&bytes);
        assert_eq!(variables[1].1, vec![1.0]);
    }

    #[test]
    fn mat_variable_names_are_valid_matlab_identifiers() {
        assert_eq!(mat_variable_name("top.pc[7:0]"), "sig_top_pc_7_0_");
        assert_eq!(mat_variable_name("clk"), "sig_clk");
        let long = mat_variable_name(&"x".repeat(200));
        assert_eq!(long.len(), 63);
        assert!(long.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        assert!(long.starts_with("sig_"));
    }

    #[test]
    fn txt_is_time_space_value_per_line() {
        let changes = [(0u64, bits(1, 1)), (12, bits(0, 1))];
        let mut one_bit = row("top.clk", &changes);
        // a one bit row keeps the bare 0/1 the value column shows
        one_bit.width = 1;
        let txt = row_txt(&one_bit).expect("fits");
        assert_eq!(txt, "0 1\n12 0\n");
    }

    #[test]
    fn all_rows_are_merged_by_time_then_row_order() {
        let a = [(0u64, bits(1, 8)), (10, bits(2, 8))];
        let b = [(5u64, bits(3, 8)), (10, bits(4, 8)), (10, bits(5, 8))];
        let c = [(10u64, bits(6, 8))];
        let rows = [row("top.a", &a), row("top.b", &b), row("top.c", &c)];
        let csv = rows_csv(&rows).expect("fits");
        assert_eq!(
            csv,
            "time,signal,value\n\
0,top.a,'h01\n\
5,top.b,'h03\n\
10,top.a,'h02\n\
10,top.b,'h04\n\
10,top.b,'h05\n\
10,top.c,'h06\n"
        );
        // the same input always produces the same bytes
        assert_eq!(csv, rows_csv(&rows).expect("fits"));
    }

    #[test]
    fn an_empty_row_contributes_nothing_and_does_not_panic() {
        let rows = [row("top.empty", &[])];
        assert_eq!(rows_csv(&rows).expect("fits"), "time,signal,value\n");
        let a = [(1u64, bits(1, 1))];
        let rows = [row("top.a", &a), row("top.empty", &[])];
        assert_eq!(
            rows_csv(&rows).expect("fits"),
            "time,signal,value\n1,top.a,'h1\n"
        );
    }

    #[test]
    fn the_size_guard_refuses_instead_of_building_a_huge_string() {
        let changes = [(0u64, bits(1, 8)), (10, bits(2, 8))];
        let row = row("top.a", &changes);
        // "time,value\n" is 11 bytes and the two lines are 8 + 9: 28 in total
        assert!(row_csv_with_limit(&row, 40).is_ok());
        assert_eq!(
            row_csv_with_limit(&row, 20),
            Err(ExportError::TooLarge { limit: 20 })
        );
        assert_eq!(
            row_csv_with_limit(&row, 12),
            Err(ExportError::TooLarge { limit: 12 })
        );
        assert_eq!(
            row_txt_with_limit(&row, 4),
            Err(ExportError::TooLarge { limit: 4 })
        );
        let rows = [row];
        assert_eq!(
            rows_csv_with_limit(&rows, 12),
            Err(ExportError::TooLarge { limit: 12 })
        );
        // the message names the limit so the status line can explain itself
        assert!(ExportError::TooLarge {
            limit: MAX_EXPORT_BYTES
        }
        .message()
        .contains("200.0 MB"));
    }

    #[test]
    fn file_names_are_sanitized_for_windows_and_carry_the_right_extension() {
        // a bus display name has `[7:0]`, and `:` is not a legal file name
        assert_eq!(
            file_name("dump", Some("top.pc[7:0]"), ExportFormat::Csv),
            "dump-top.pc[7_0].csv"
        );
        assert_eq!(file_name("dump", None, ExportFormat::Csv), "dump-rows.csv");
        assert_eq!(file_name("dump", None, ExportFormat::Txt), "dump-rows.txt");
        // path separators never leak into the name, and a trailing dot goes
        assert_eq!(
            file_name("a/b\\c", None, ExportFormat::Csv),
            "a_b_c-rows.csv"
        );
        assert_eq!(sanitize_file_part(".."), "waveform");
        assert_eq!(sanitize_file_part(""), "waveform");
    }

    #[test]
    fn counts_and_sizes_read_like_the_status_line() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1204), "1,204");
        assert_eq!(format_count(1_234_567), "1,234,567");
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(34_816), "34 KB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2.0 MB");
        assert_eq!(
            row_status("top.pc[7:0]", 1204, 34_816),
            "exported top.pc[7:0]: 1,204 changes, 34 KB"
        );
        assert_eq!(
            rows_status(12, 1204, 34_816),
            "exported 12 rows: 1,204 changes, 34 KB"
        );
    }

    #[test]
    fn a_dump_opened_from_bytes_is_named_after_its_root_scope() {
        // there is no file path for a browser dump, so the stem falls back to
        // the root scope (the FST fixture is the smallest real dump available)
        let waveform = wave_insight_lib::waveform::Waveform::open_bytes(
            include_bytes!("../../../wave-insight-lib/tests/data/io_mode_wave.fst").to_vec(),
        )
        .expect("the fst fixture opens");
        let stem = dump_stem(Some(&waveform));
        assert!(!stem.is_empty());
        assert!(!stem.contains(['/', '\\']));
        // no dump at all still produces a usable name
        assert_eq!(dump_stem(None), "waveform");
    }
}
