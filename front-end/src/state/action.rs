//! Everything that can happen to [`State`](super::State).
//!
//! An action is a fact ("the splitter now sits here"), never a command ("move
//! the splitter"); the reducer decides what the fact does. Actions are the only
//! way to change the store, so every change is one line of `reduce` and one
//! place to look.

use super::types::{Layout, Status};

// The full action surface lands with steps 2-4; keeping it here means those
// steps add reducer arms instead of moving the type around.
#[allow(dead_code)]
pub enum Action {
    /// The whole layout was replaced (e.g. restored, or a drag committed).
    Layout(Layout),
    /// The sidebar splitter moved; `max` is measured by the UI because the
    /// reducer stays pure.
    ResizeSidebar { width: f64, max: f64 },
    /// The hamburger button: show or hide the sidebar.
    ToggleDrawer,
    /// The editor/wave splitter moved, as a ratio of the main pane height.
    SetEditorRatio(f32),
    /// One line of feedback for the top bar.
    Status(Status),
    /// Nothing to report any more.
    ClearStatus,
}

/// Work the reducer may not do itself: I/O, the LSP, the editor.
///
/// Empty in step 1 (layout and status need no outside work); steps 2-3 add the
/// variants that decode rows and keep the language server in sync.
#[derive(Debug)]
pub enum Effect {}
