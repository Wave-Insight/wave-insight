//! Where the waveform bytes come from.
//!
//! `wellen` needs `BufRead + Seek` and (for the body) an owned `'static` reader,
//! so every source opens a [`ByteReader`] through the [`ByteSource`] trait.
//! Opening the reader is done lazily and freshly for every body read / streaming
//! query, which keeps [`super::Waveform`] `Send + Sync` and allows queries to run
//! from `&self`.
//!
//! The trait is the byte-source seam: today only a file (read on demand) and an
//! owned memory buffer implement it; a desktop memory-mapped backend can add an
//! implementation and a [`WaveformSource`] variant without touching the parser.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where the waveform was loaded from.
#[derive(Clone, Debug)]
pub enum WaveformSource {
    /// A file on disk, read on demand (never memory mapped by us).
    Path(PathBuf),
    /// Bytes already in memory, e.g. `include_bytes!` in tests or a file that
    /// was read by the caller (Tauri dialog, HTTP body, ...).
    Memory(Arc<[u8]>),
}

impl WaveformSource {
    /// Remember `path`, the file is not opened yet.
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        WaveformSource::Path(path.into())
    }

    /// Take ownership of a buffer.
    pub fn from_bytes(bytes: impl Into<Arc<[u8]>>) -> Self {
        WaveformSource::Memory(bytes.into())
    }

    /// Path if the source is a file.
    pub fn path(&self) -> Option<&Path> {
        match self {
            WaveformSource::Path(path) => Some(path),
            WaveformSource::Memory(_) => None,
        }
    }

    /// Number of bytes if known without touching the file system.
    pub fn memory_len(&self) -> Option<usize> {
        match self {
            WaveformSource::Path(_) => None,
            WaveformSource::Memory(bytes) => Some(bytes.len()),
        }
    }

    /// Open a fresh reader. `wellen` seeks around, so every caller gets its own.
    pub(crate) fn reader(&self) -> io::Result<ByteReader> {
        match self {
            WaveformSource::Path(path) => path.open(),
            WaveformSource::Memory(bytes) => bytes.open(),
        }
    }

    /// Source bytes kept alive by this handle (only for the memory variant).
    pub fn retained_bytes(&self) -> usize {
        match self {
            WaveformSource::Path(_) => 0,
            WaveformSource::Memory(bytes) => bytes.len(),
        }
    }
}

/// A seekable byte source for the waveform reader.
///
/// The current library has two implementations, a file (read on demand, never
/// memory mapped) and an owned memory buffer.  Everything that parses a
/// waveform gets its reader through this trait, so a desktop memory-mapped
/// backend only has to implement the trait and add a [`WaveformSource`] variant;
/// no parser code changes.  Readers are opened fresh per body read/streaming
/// query, which keeps [`super::Waveform`] `Send + Sync`.
pub trait ByteSource: std::fmt::Debug + Send + Sync {
    /// Byte length of the source, if known without reading it.
    fn byte_len(&self) -> Option<u64>;

    /// Open a fresh reader at position 0.
    fn open(&self) -> io::Result<ByteReader>;
}

/// A reader over a [`ByteSource`].
///
/// A concrete wrapper (instead of a bare `Box<dyn BufRead + Seek>`, which Rust
/// does not allow) that forwards the three std traits, is `Send + Sync` and
/// `'static`, which is exactly what `wellen` needs for both the header and the
/// body continuation.
pub struct ByteReader {
    inner: Box<dyn ReadSeek + Send + Sync>,
}

trait ReadSeek: BufRead + Seek {}
impl<T: BufRead + Seek> ReadSeek for T {}

impl ByteReader {
    fn new(inner: Box<dyn ReadSeek + Send + Sync>) -> Self {
        Self { inner }
    }
}

impl Read for ByteReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl BufRead for ByteReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.inner.consume(amount);
    }
}

impl Seek for ByteReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

impl ByteSource for Arc<[u8]> {
    fn byte_len(&self) -> Option<u64> {
        Some(self.len() as u64)
    }

    fn open(&self) -> io::Result<ByteReader> {
        Ok(ByteReader::new(Box::new(BufReader::new(Cursor::new(
            Arc::clone(self),
        )))))
    }
}

impl ByteSource for PathBuf {
    fn byte_len(&self) -> Option<u64> {
        std::fs::metadata(self).ok().map(|metadata| metadata.len())
    }

    fn open(&self) -> io::Result<ByteReader> {
        Ok(ByteReader::new(Box::new(BufReader::new(File::open(self)?))))
    }
}

/// Byte length of the FST header block.
///
/// `fst-reader` asserts this exact value, so a "header block" with a different
/// length is not an FST file that wellen could read anyway.
const FST_HEADER_BLOCK_LEN: u64 = 329;
/// First block type of every FST file (`BlockType::Header`).
const FST_BLOCK_HEADER: u8 = 0;
/// `BlockType::GZipWrapper`, an FST file may start with a gzip wrapped block.
const FST_BLOCK_GZIP: u8 = 254;

/// A decoded FST length is plausible when it cannot blow up memory: at most
/// 200x the file (a generous compression ratio) and at most 4 GiB overall.
/// Real dumps are far below this; a corrupted u64 length field is not.
fn plausible_decoded_length(length: u64, file_len: u64) -> bool {
    let cap = file_len
        .saturating_mul(200)
        .clamp(64 * 1024 * 1024, 4 * 1024 * 1024 * 1024);
    length <= cap
}

fn corrupt_fst(detail: impl Into<String>) -> crate::waveform::WaveformError {
    crate::waveform::WaveformError::Corrupt {
        format: crate::waveform::WaveFormat::Fst,
        operation: "check file structure",
        detail: detail.into(),
    }
}

/// Cheap structural check of an FST before wellen/fst-reader sees it.
///
/// The FST reader trusts length fields (time table item counts, decompression
/// lengths) and allocates from them; a single flipped byte can turn one into
/// terabytes, and an allocation failure aborts the process instead of producing
/// an error.  This walk over the top-level block headers and the tiny meta
/// sections rejects lengths that cannot describe this file, so a corrupt dump
/// becomes WaveformError::Corrupt instead of an out-of-memory abort.  It reads a
/// handful of bytes per block and seeks over the payloads.
fn validate_fst(reader: &mut ByteReader) -> crate::waveform::Result<()> {
    use std::io::{Read, Seek, SeekFrom};

    let file_len = reader.seek(SeekFrom::End(0))?;
    reader.seek(SeekFrom::Start(0))?;
    let mut cursor = 0u64;
    let mut header = [0u8; 9];
    let mut word = [0u8; 8];
    loop {
        if cursor >= file_len {
            break;
        }
        if cursor + 9 > file_len {
            return Err(corrupt_fst(format!(
                "truncated block header at offset {cursor}"
            )));
        }
        reader.seek(SeekFrom::Start(cursor))?;
        reader.read_exact(&mut header)?;
        let block_type = header[0];
        let mut length_bytes = [0u8; 8];
        length_bytes.copy_from_slice(&header[1..9]);
        let block_len = u64::from_be_bytes(length_bytes);
        // Block layout: [type u8][section_length u64][payload].  The section
        // length includes its own 8 bytes, so the next block starts at
        // cursor + 1 + section_length.
        let payload_start = cursor + 1;
        if block_len < 8 || block_len > file_len.saturating_sub(payload_start) {
            return Err(corrupt_fst(format!(
                "block at offset {cursor} claims {block_len} bytes beyond the end of the file"
            )));
        }
        match block_type {
            // GZipWrapper: the whole-file gzip stream, followed by its
            // decompressed length.
            254 => {
                if block_len < 16 {
                    return Err(corrupt_fst("gzip wrapper is too short"));
                }
                reader.seek(SeekFrom::Start(payload_start + 8))?;
                reader.read_exact(&mut word)?;
                let uncompressed = u64::from_be_bytes(word);
                if !plausible_decoded_length(uncompressed, file_len) {
                    return Err(corrupt_fst(format!(
                        "gzip wrapper claims an uncompressed length of {uncompressed} bytes"
                    )));
                }
                break;
            }
            // VcData / dynamic alias sections: the time table meta data sits in
            // the last 24 bytes of the section (uncompressed length, compressed
            // length, item count); the reader allocates from these.
            1 | 5 | 8 => {
                if block_len >= 32 {
                    let meta_start = payload_start + block_len - 24;
                    reader.seek(SeekFrom::Start(meta_start))?;
                    let mut meta = [0u8; 24];
                    reader.read_exact(&mut meta)?;
                    let mut value = [0u8; 8];
                    value.copy_from_slice(&meta[0..8]);
                    let uncompressed = u64::from_be_bytes(value);
                    value.copy_from_slice(&meta[8..16]);
                    let compressed = u64::from_be_bytes(value);
                    value.copy_from_slice(&meta[16..24]);
                    let items = u64::from_be_bytes(value);
                    if compressed > block_len
                        || !plausible_decoded_length(uncompressed, file_len)
                        || items > block_len.saturating_mul(8).max(1)
                    {
                        return Err(corrupt_fst(format!(
                            "data section at offset {cursor} has implausible time table lengths"
                        )));
                    }
                }
            }
            // Hierarchy blocks (zlib/lz4/lz4duo): first two u64s are the
            // section length and the uncompressed length.
            4 | 6 | 7 => {
                if block_len < 16 {
                    return Err(corrupt_fst(format!(
                        "hierarchy block at offset {cursor} is too short"
                    )));
                }
                reader.seek(SeekFrom::Start(payload_start))?;
                let mut lengths = [0u8; 16];
                reader.read_exact(&mut lengths)?;
                let mut value = [0u8; 8];
                value.copy_from_slice(&lengths[0..8]);
                let section_len = u64::from_be_bytes(value);
                value.copy_from_slice(&lengths[8..16]);
                let uncompressed = u64::from_be_bytes(value);
                if section_len > block_len
                    || section_len < 16
                    || !plausible_decoded_length(uncompressed, file_len)
                {
                    return Err(corrupt_fst(format!(
                        "hierarchy block at offset {cursor} has implausible lengths"
                    )));
                }
            }
            // Skip with a zero length marks the end of the block list.
            255 if block_len == 0 => break,
            _ => {}
        }
        cursor = cursor + 1 + block_len;
    }
    reader.seek(SeekFrom::Start(0))?;
    Ok(())
}

/// Declared VCD variable: bit width and value family, keyed by identifier code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct VcdVariable {
    width: u32,
    family: VcdValueFamily,
}

/// What kind of body value a VCD variable accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VcdValueFamily {
    /// wire/reg/integer/...: scalar or b... bit vectors.
    Bits,
    /// real/realtime: r... values.
    Real,
    /// Anything else (string, event, ...): not validated strictly.
    Other,
}

/// One 9-state VCD bit character.
fn vcd_state_byte(byte: u8) -> bool {
    matches!(
        byte,
        b'0' | b'1'
            | b'x'
            | b'X'
            | b'z'
            | b'Z'
            | b'h'
            | b'H'
            | b'u'
            | b'U'
            | b'w'
            | b'W'
            | b'l'
            | b'L'
            | b'-'
    )
}

fn malformed_vcd(detail: impl Into<String>) -> crate::waveform::WaveformError {
    crate::waveform::WaveformError::Corrupt {
        format: crate::waveform::WaveFormat::Vcd,
        operation: "check file structure",
        detail: detail.into(),
    }
}

/// Structural validation of a VCD before wellen parses or decodes it.
///
/// wellen decodes a signal lazily when a row is first drawn.  Its VCD body
/// decoder trusts the declared width and panics on a body value that is wider
/// (or a short value that cannot be zero/x/z extended); with the release
/// profile's panic = "abort" that is an unrecoverable abort in wasm.  The most
/// common user-data cause is one identifier code declared twice with different
/// widths ($var wire 64 ! big, then $var wire 8 ! small): wellen keeps the last
/// declaration and later decodes the 64 bit body value against 8 bits.
///
/// This walk reads the whole VCD once, before any deferred decode: it rejects
/// conflicting declarations and body values that the decoder would panic on,
/// and accepts legal same-width aliasing (several names for one id).  It is a
/// guard rail, not a full VCD parser: unknown directives/tokens are ignored so
/// vendor extensions (port dumps, ...) keep working.
///
/// Cost: one streaming pass over the file at open time (O(bytes), O(1) memory
/// besides the declaration table).  The text after $enddefinitions is only
/// scanned; it is not parsed into values.
pub(crate) fn validate_vcd(source: &mut ByteReader) -> crate::waveform::Result<()> {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    const MAX_LINE: usize = 1024 * 1024;
    let mut reader = BufReader::new(&mut *source);
    let mut line = String::new();
    let mut variables: HashMap<String, VcdVariable> = HashMap::new();
    let mut in_body = false;
    let mut skipping_comment = false;
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        if line.len() > MAX_LINE {
            return Err(malformed_vcd(format!(
                "line longer than {MAX_LINE} bytes; not a VCD"
            )));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if skipping_comment {
            // A comment body is arbitrary text; it ends at the next $end.
            if trimmed.split_whitespace().any(|token| token == "$end") {
                skipping_comment = false;
            }
            continue;
        }
        if !in_body {
            if let Some(rest) = trimmed.strip_prefix("$var") {
                // $var <type> <width> <id> <name> [range] $end
                let mut tokens = rest.split_whitespace();
                let var_type = tokens.next().unwrap_or_default();
                let width_token = tokens.next().unwrap_or_default();
                let id = tokens.next().unwrap_or_default();
                if id.is_empty() {
                    return Err(malformed_vcd("malformed $var declaration"));
                }
                let Ok(width) = width_token.parse::<u32>() else {
                    return Err(malformed_vcd(format!(
                        "malformed $var declaration for variable id {id}: width {width_token} is not a number"
                    )));
                };
                let family = match var_type {
                    "real" | "realtime" => VcdValueFamily::Real,
                    "wire" | "reg" | "logic" | "bit" | "integer" | "time" | "parameter"
                    | "event" | "supply0" | "supply1" | "tri" | "triand" | "trior" | "trireg"
                    | "tri0" | "tri1" | "wand" | "wor" => VcdValueFamily::Bits,
                    _ => VcdValueFamily::Other,
                };
                if let Some(existing) = variables.get(id) {
                    if existing.width != width {
                        return Err(malformed_vcd(format!(
                            "malformed VCD: variable id {id} is declared with conflicting widths ({} and {})",
                            existing.width, width
                        )));
                    }
                    if existing.family != family {
                        return Err(malformed_vcd(format!(
                            "malformed VCD: variable id {id} is declared with conflicting types ({:?} and {:?})",
                            existing.family, family
                        )));
                    }
                } else {
                    variables.insert(id.to_owned(), VcdVariable { width, family });
                }
                continue;
            }
            if trimmed.starts_with("$enddefinitions") {
                in_body = true;
                continue;
            }
            if trimmed.starts_with("$comment") {
                skipping_comment = !trimmed.split_whitespace().any(|token| token == "$end");
                continue;
            }
            // Other header directives ($date, $version, $scope, $upscope, ...)
            // carry no value validation.
            continue;
        }

        // Body: timestamps, directives and value changes.
        if let Some(rest) = trimmed.strip_prefix('$') {
            if rest.starts_with("comment") {
                skipping_comment = !trimmed.split_whitespace().any(|token| token == "$end");
            }
            // $dumpvars/$dumpall/... are structural markers; their values are
            // ordinary change lines and are validated below.
            continue;
        }
        if trimmed.starts_with('#') {
            continue; // timestamp
        }
        let mut tokens = trimmed.split_whitespace();
        let token = tokens.next().unwrap_or_default();
        if token.is_empty() {
            continue;
        }
        let first = token.as_bytes()[0];
        let (id, value_kind) = match first {
            b'b' | b'B' => {
                let id = tokens.next().unwrap_or_default();
                (id, VcdBodyValue::Binary(&token[1..]))
            }
            b'r' => {
                let id = tokens.next().unwrap_or_default();
                (id, VcdBodyValue::Real)
            }
            b's' => {
                // A string value may contain spaces; the id is the last token.
                let id = trimmed.split_whitespace().last().unwrap_or_default();
                (id, VcdBodyValue::Other)
            }
            b'p' => {
                // Port dump value (IEEE 1364 extension): left to wellen.
                continue;
            }
            _ if vcd_state_byte(first) => (
                token.get(1..).unwrap_or_default(),
                VcdBodyValue::Scalar(first),
            ),
            _ => continue,
        };
        if id.is_empty() {
            continue;
        }
        let Some(variable) = variables.get(id) else {
            return Err(malformed_vcd(format!(
                "malformed VCD: value change for undeclared variable id {id}"
            )));
        };
        validate_vcd_value(variable, id, value_kind)?;
    }
    // A header without $enddefinitions (or a comment that never closes) has no
    // body values to validate; wellen's own header parser reports the malformed
    // VCD with its usual Malformed error, which the caller already turns into a
    // WaveformError.  Do not take that diagnosis away here.
    let _ = (in_body, skipping_comment);
    source.seek(SeekFrom::Start(0))?;
    Ok(())
}

/// One body value of a VCD change, as far as validation needs it.
#[derive(Clone, Copy, Debug)]
enum VcdBodyValue<'a> {
    Binary(&'a str),
    Real,
    Scalar(u8),
    Other,
}

/// Reject body values that wellen's VCD decoder would panic on.
fn validate_vcd_value(
    variable: &VcdVariable,
    id: &str,
    value: VcdBodyValue<'_>,
) -> crate::waveform::Result<()> {
    match variable.family {
        VcdValueFamily::Real if !matches!(value, VcdBodyValue::Real | VcdBodyValue::Other) => {
            return Err(malformed_vcd(format!(
                "malformed VCD: variable id {id} is real but has a bit-vector value"
            )));
        }
        VcdValueFamily::Bits if matches!(value, VcdBodyValue::Real) => {
            return Err(malformed_vcd(format!(
                "malformed VCD: variable id {id} is not real but has a real value"
            )));
        }
        _ => {}
    }
    let width = variable.width;
    match value {
        VcdBodyValue::Binary(bits) => {
            // wellen special-cases a leading 0b (pymtl3).
            let bits = if bits.len() > 2 && bits.starts_with("0b") {
                &bits[2..]
            } else {
                bits
            };
            if width <= 1 {
                // len 1: the decoder only looks at the last character.
                if let Some(last) = bits.as_bytes().last() {
                    if !vcd_state_byte(*last) {
                        return Err(invalid_vcd_bits(id, bits));
                    }
                }
                return Ok(());
            }
            if bits.is_empty() {
                return Err(malformed_vcd(format!(
                    "malformed VCD: variable id {id} has an empty bit-vector value"
                )));
            }
            if !bits.bytes().all(vcd_state_byte) {
                return Err(invalid_vcd_bits(id, bits));
            }
            if bits.len() > width as usize {
                return Err(malformed_vcd(format!(
                    "malformed VCD: value for variable id {id} has {} bits but the declared width is {width}",
                    bits.len()
                )));
            }
            if bits.len() < width as usize
                && !matches!(bits.as_bytes()[0], b'0' | b'1' | b'x' | b'X' | b'z' | b'Z')
            {
                return Err(malformed_vcd(format!(
                    "malformed VCD: value for variable id {id} has {} bits but the declared width is {width}, and only 0/1/x/z values can be extended",
                    bits.len()
                )));
            }
            Ok(())
        }
        VcdBodyValue::Scalar(byte) => {
            if width <= 1 {
                return Ok(());
            }
            if !matches!(byte, b'0' | b'1' | b'x' | b'X' | b'z' | b'Z') {
                return Err(malformed_vcd(format!(
                    "malformed VCD: scalar value for variable id {id} cannot be extended to the declared width {width}"
                )));
            }
            Ok(())
        }
        VcdBodyValue::Real | VcdBodyValue::Other => Ok(()),
    }
}

fn invalid_vcd_bits(id: &str, bits: &str) -> crate::waveform::WaveformError {
    malformed_vcd(format!(
        "malformed VCD: value for variable id {id} contains a non-9-state character: {bits}"
    ))
}

/// Detect the file format from the magic bytes, *before* wellen gets to see it.
///
/// This is deliberately stricter than `wellen::viewers::detect_file_format`:
/// wellen's FST probe only walks block headers, so arbitrary binary garbage can
/// be mistaken for FST and then panics inside `fst-reader`. Gating the input
/// here keeps the promise that a corrupt file yields an error instead of a
/// crash. Everything that passes the gate is then parsed by wellen as usual.
pub(crate) fn detect_format(
    reader: &mut ByteReader,
) -> crate::waveform::Result<crate::waveform::WaveFormat> {
    use crate::waveform::{WaveFormat, WaveformError};

    let mut head = [0u8; 16];
    let mut filled = 0usize;
    while filled < head.len() {
        let read = reader.read(&mut head[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    reader.seek(SeekFrom::Start(0))?;
    let head = &head[..filled];

    //VCD: the first non whitespace byte is the `$` of `$date`/`$version`/...
    if head.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'$') {
        return Ok(WaveFormat::Vcd);
    }

    //FST: a header block (`type == 0`, big endian u64 length == 329) or a
    //gzip wrapped block
    if let Some(&first) = head.first() {
        if first == FST_BLOCK_HEADER && head.len() >= 9 {
            let mut length = [0u8; 8];
            length.copy_from_slice(&head[1..9]);
            if u64::from_be_bytes(length) == FST_HEADER_BLOCK_LEN {
                validate_fst(reader)?;
                return Ok(WaveFormat::Fst);
            }
        }
        if first == FST_BLOCK_GZIP {
            validate_fst(reader)?;
            return Ok(WaveFormat::Fst);
        }
    }

    //GHW is always gzip compressed
    if head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b {
        return Ok(WaveFormat::Ghw);
    }

    Err(WaveformError::UnknownFormat)
}
