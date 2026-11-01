//! Opening files: the picker/drop zone, the tauri file dialog and the chunked
//! read that feeds the parsers.
//!
//! The browser path reads a dump in ~8 MB slices with a progress line, a cancel
//! action and the size guard (see file.rs), while the status texts, the guard
//! thresholds and the pure percentage/size formatting live in progress.rs so
//! they are unit tested on the host target.
#[cfg(feature = "tauri")]
pub use file::parse_dump;
pub use file::{FileLoad, LoadReport};
#[cfg(feature = "tauri")]
pub use filelist::current_webview;
#[cfg(feature = "tauri")]
pub use filelist::invoke;
#[cfg(feature = "tauri")]
pub use filelist::FileList;

pub mod file;
#[cfg(feature = "tauri")]
mod filelist;
// the chunked reader is browser-only (the tauri backend hands back whole
// files), so the pieces only it uses would warn as dead code in that build
#[cfg_attr(feature = "tauri", allow(dead_code))]
pub mod progress;
