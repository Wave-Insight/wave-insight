//! The whole state of the app, one struct.
//!
//! Step 1 of R2 only owns the layout and the status line; the slices for the
//! sources, the design, the dump, the rows and the viewport are added by the
//! later steps (see `docs/state-store.md` §2.3). Keeping the struct here from
//! the start means those steps only add fields and actions, they do not move
//! the store around again.

use super::types::{Layout, Status};

/// State that is not read on every frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nav {
    /// One line of feedback for the top bar; `None` hides the chip.
    pub status: Option<Status>,
}

/// The single source of truth.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub layout: Layout,
    pub nav: Nav,
}
