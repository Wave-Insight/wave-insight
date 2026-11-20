pub use compare_bar::CompareBar;
pub use settings::Settings;
pub use split::SplitPane;
pub use wave::WaveShow;

mod canvas;
pub mod compare_bar;
mod ctrl;
pub mod export;
pub mod find;
mod geometry;
// the tauri build draws native dumps over IPC and never runs the wasm
// pre-flight, so those helpers are dead code there only
#[cfg_attr(feature = "tauri", allow(dead_code))]
pub mod memory;
mod settings;
mod signal;
mod split;
mod time;
mod util;
mod wave;
pub mod windowed;
