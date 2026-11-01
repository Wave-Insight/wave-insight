//! Where the waveform bytes come from.
//!
//! `wellen` needs `BufRead + Seek` and (for the body) an owned `'static` reader,
//! so we normalise both supported sources (a path and an in-memory buffer) into
//! one reader type. Opening the reader is done lazily and freshly for every
//! body read / streaming query, which keeps [`super::Waveform`] `Send + Sync`
//! and allows queries to run from `&self`.

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
    pub(crate) fn reader(&self) -> io::Result<AnyReader> {
        match self {
            WaveformSource::Path(path) => Ok(AnyReader::File(BufReader::new(File::open(path)?))),
            WaveformSource::Memory(bytes) => Ok(AnyReader::Memory(Cursor::new(bytes.clone()))),
        }
    }
}

/// A reader over either a file or a memory buffer.
///
/// `wellen` is generic over `BufRead + Seek`, and different entry points would
/// otherwise force different monomorphised types on us.
pub(crate) enum AnyReader {
    File(BufReader<File>),
    Memory(Cursor<Arc<[u8]>>),
}

impl Read for AnyReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            AnyReader::File(r) => r.read(buf),
            AnyReader::Memory(r) => r.read(buf),
        }
    }
}

impl BufRead for AnyReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        match self {
            AnyReader::File(r) => r.fill_buf(),
            AnyReader::Memory(r) => r.fill_buf(),
        }
    }

    fn consume(&mut self, amount: usize) {
        match self {
            AnyReader::File(r) => r.consume(amount),
            AnyReader::Memory(r) => r.consume(amount),
        }
    }
}

impl Seek for AnyReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match self {
            AnyReader::File(r) => r.seek(pos),
            AnyReader::Memory(r) => r.seek(pos),
        }
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
fn validate_fst(reader: &mut AnyReader) -> crate::waveform::Result<()> {
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

/// Detect the file format from the magic bytes, *before* wellen gets to see it.
///
/// This is deliberately stricter than `wellen::viewers::detect_file_format`:
/// wellen's FST probe only walks block headers, so arbitrary binary garbage can
/// be mistaken for FST and then panics inside `fst-reader`. Gating the input
/// here keeps the promise that a corrupt file yields an error instead of a
/// crash. Everything that passes the gate is then parsed by wellen as usual.
pub(crate) fn detect_format(
    reader: &mut AnyReader,
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
