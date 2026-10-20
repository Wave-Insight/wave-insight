//! Everything that can happen to [`State`](super::State).
//!
//! An action is a fact ("the splitter now sits here"), never a command ("move
//! the splitter"); the reducer decides what the fact does. Actions are the only
//! way to change the store, so every change is one line of `reduce` and one
//! place to look.
//!
//! Row actions are *intents*: the row list, its order, the settings and the
//! multi-selection are owned by the reducer. The pane dispatches them and then
//! renders from the resulting snapshot; it never mutates its own copy.

use super::types::{Layout, Settings, Status};

// `Layout`, `SetEditorRatio` and `ClearStatus` are the rest of the surface the
// later R2 steps use; keeping the enum stable makes those steps additive.
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

    /// Append one signal row; `width`/`load`/`drive` are resolved by `App` (the
    /// reducer has no access to the dump or the design).
    RowAdded {
        path: String,
        name: String,
        width: usize,
        load: Vec<String>,
        drive: Vec<String>,
    },
    /// Remove the given rows (indices; the reducer sorts and dedupes them).
    RowsRemoved { indices: Vec<usize> },
    /// A name cell was clicked; `ctrl`/`shift` are the modifier state.
    RowSelected { index: usize, ctrl: bool, shift: bool },
    /// A row was dragged to another position.
    RowsReordered { from: usize, to: usize },
    /// A presentation setting was changed for the given rows.
    RowSettingChanged { indices: Vec<usize>, setting: Settings },
}

/// Work the reducer may not do itself: I/O, the LSP, the editor.
///
/// Empty until the later steps (row actions only touch the store); steps 3-4 add
/// the variants that decode and keep the language server in sync.
#[derive(Debug)]
pub enum Effect {}
