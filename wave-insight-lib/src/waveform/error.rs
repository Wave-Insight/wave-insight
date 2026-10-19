//! Error type for the waveform backend.
//!
//! Everything in [`crate::waveform`] returns [`Result`] instead of panicking on
//! malformed input: the magic bytes are checked before wellen sees a file, and
//! a panic raised inside `wellen`/`fst-reader` while parsing is caught and
//! reported as [`WaveformError::Corrupt`] (they contain `assert!`/`todo!()` on
//! data they consider impossible, which a hand crafted or half written file can
//! reach). The only panics that are *not* caught are the ones raised by a
//! caller supplied callback.

use std::fmt;

use super::WaveFormat;

/// Result alias used by the waveform backend.
pub type Result<T> = std::result::Result<T, WaveformError>;

/// Everything that can go wrong when opening or querying a waveform file.
#[derive(Debug)]
pub enum WaveformError {
    /// The file could not be opened/read from disk.
    Io(std::io::Error),
    /// `wellen` refused the file: unknown/unsupported version, corrupt header,
    /// truncated body, ...
    Malformed(wellen::WellenError),
    /// The backend library panicked while parsing this file.
    ///
    /// `wellen`/`fst-reader` contain a few `assert!`/`todo!()` on data they
    /// consider impossible; a hand crafted or half written FST can reach one.
    /// We catch those panics and report them as a corrupt file instead of
    /// taking the whole application down.
    Corrupt {
        /// Format that was being parsed.
        format: WaveFormat,
        /// What was being done, e.g. `"read header"`.
        operation: &'static str,
        /// Panic message, for bug reports.
        detail: String,
    },
    /// The magic bytes do not match VCD, FST or GHW.
    UnknownFormat,
    /// No variable with that path (or index) exists in the hierarchy.
    UnknownSignal(String),
    /// The operation is not available for this file format / this signal.
    Unsupported {
        /// File format the operation was attempted on.
        format: WaveFormat,
        /// Short description of what was attempted, e.g. `"stream_changes"`.
        operation: &'static str,
    },
    /// The backend could not complete the request for an internal reason, e.g.
    /// a signal without value encoding information.
    Backend(String),
}

impl WaveformError {
    /// Short, user facing message without the error chain.
    pub fn message(&self) -> String {
        match self {
            WaveformError::Io(e) => format!("io error: {e}"),
            WaveformError::Malformed(e) => format!("malformed waveform: {e}"),
            WaveformError::Corrupt {
                format,
                operation,
                detail,
            } => format!(
                "corrupt {format} file: `{operation}` failed while parsing it ({detail})"
            ),
            WaveformError::UnknownFormat => {
                "unknown waveform format (expected VCD, FST or GHW)".to_string()
            }
            WaveformError::UnknownSignal(path) => format!("no such signal: `{path}`"),
            WaveformError::Unsupported { format, operation } => {
                format!("`{operation}` is not supported for {format} waveforms")
            }
            WaveformError::Backend(msg) => format!("waveform backend error: {msg}"),
        }
    }
}

impl fmt::Display for WaveformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for WaveformError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WaveformError::Io(e) => Some(e),
            WaveformError::Malformed(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WaveformError {
    fn from(value: std::io::Error) -> Self {
        WaveformError::Io(value)
    }
}

impl From<wellen::WellenError> for WaveformError {
    fn from(value: wellen::WellenError) -> Self {
        match value {
            //`open_and_detect_file_format` panics on unknown formats, `read_header`
            //reports them - give them a dedicated, friendlier variant
            wellen::WellenError::UnknownFileFormat => WaveformError::UnknownFormat,
            other => WaveformError::Malformed(other),
        }
    }
}

/// Extract a readable message from a panic payload.
pub(crate) fn panic_detail(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| message.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Run a `wellen` parsing step, turning a panic inside the backend into a
/// [`WaveformError::Corrupt`].
///
/// `wellen`/`fst-reader` contain `assert!`/`todo!()` on data they consider
/// impossible. A corrupt file must not take the viewer down, so a panic here is
/// a corrupt file, not a crash. Only use this around *library* code - never
/// around a caller supplied callback, whose panics must propagate.
pub(crate) fn catch_parse<T>(
    format: WaveFormat,
    operation: &'static str,
    parse: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(parse)) {
        Ok(result) => result,
        Err(payload) => Err(WaveformError::Corrupt {
            format,
            operation,
            detail: panic_detail(payload.as_ref()),
        }),
    }
}
