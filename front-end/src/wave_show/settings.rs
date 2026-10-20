//! The per-row presentation settings moved into the store in R2 step 2 (B1):
//! `state::Settings` owns them, the renderer still names the type through this
//! module so the pane code reads the same as before.

pub use crate::state::Settings;