//! Opening files: the picker/drop zone, the tauri file dialog and the chunked
//! read that feeds the parsers.
//!
//! The browser path reads a dump in ~8 MB slices with a progress line, a cancel
//! action and the size guard (see file.rs), while the status texts, the guard
//! thresholds and the pure percentage/size formatting live in progress.rs so
//! they are unit tested on the host target.
#[cfg(any(feature = "tauri", feature = "web-remote"))]
pub use file::parse_dump;
pub use file::{drop_overlay_html, FileLoad, LoadReport};
#[cfg(feature = "tauri")]
pub use filelist::current_webview;
// the wasm-fallback handler and the agent reply path use `invoke`; in the
// web-remote build it resolves against the shim's no-op __TAURI__.core.invoke
#[cfg(any(feature = "tauri", feature = "web-remote"))]
pub use filelist::dump_bytes;
#[cfg(any(feature = "tauri", feature = "web-remote"))]
pub use filelist::invoke;
#[cfg(feature = "tauri")]
pub use filelist::FileList;

pub mod file;
// compiled for web-remote for the `invoke` binding only; the desktop file
// dialog component itself is never mounted there (see file.rs / app.rs)
#[cfg(any(feature = "tauri", feature = "web-remote"))]
#[cfg_attr(feature = "web-remote", allow(dead_code))]
mod filelist;
// the chunked reader is browser-only (the tauri backend hands back whole
// files), so the pieces only it uses would warn as dead code in that build
#[cfg_attr(feature = "tauri", allow(dead_code))]
pub mod progress;
