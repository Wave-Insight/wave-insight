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
            WaveformSource::Path(path) => {
                Ok(AnyReader::File(BufReader::new(File::open(path)?)))
            }
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

/// Detect the file format from the magic bytes, *before* wellen gets to see it.
///
/// This is deliberately stricter than `wellen::viewers::detect_file_format`:
/// wellen's FST probe only walks block headers, so arbitrary binary garbage can
/// be mistaken for FST and then panics inside `fst-reader`. Gating the input
/// here keeps the promise that a corrupt file yields an error instead of a
/// crash. Everything that passes the gate is then parsed by wellen as usual.
pub(crate) fn detect_format(reader: &mut AnyReader) -> crate::waveform::Result<crate::waveform::WaveFormat> {
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
                return Ok(WaveFormat::Fst);
            }
        }
        if first == FST_BLOCK_GZIP {
            return Ok(WaveFormat::Fst);
        }
    }

    //GHW is always gzip compressed
    if head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b {
        return Ok(WaveFormat::Ghw);
    }

    Err(WaveformError::UnknownFormat)
}
