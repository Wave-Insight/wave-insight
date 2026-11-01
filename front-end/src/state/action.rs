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

use super::types::{Cursors, DumpHandle, DumpId, Focus, Layout, Settings, Status, View};

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
    /// How many language-server problems the open sources currently have
    /// (the bridge counts the published diagnostics).
    Problems(u32),

    /// Append one signal row; `width`/`load`/`drive` are resolved by `App` (the
    /// reducer has no access to the dump or the design). `bit` adds the row as
    /// one bit of the bus at `path` (the expand-bus view).
    RowAdded {
        path: String,
        name: String,
        width: usize,
        load: Vec<String>,
        drive: Vec<String>,
        bit: Option<u8>,
    },
    /// Show the bits of the bus at `index` as their own rows right below it.
    /// `width` is the width the dump resolved (the reducer cannot read the dump).
    RowsBitExpanded { index: usize, width: usize },
    /// Remove the bit rows that belong to the bus at `index`.
    RowsBitCollapsed { index: usize },
    /// Remove the given rows (indices; the reducer sorts and dedupes them).
    RowsRemoved { indices: Vec<usize> },
    /// A name cell was clicked; `ctrl`/`shift` are the modifier state.
    RowSelected {
        index: usize,
        ctrl: bool,
        shift: bool,
    },
    /// `Ctrl+A`: every row is selected (so one Delete removes all of them).
    RowsAllSelected,
    /// `Esc`: nothing is selected any more.
    RowsCleared,
    /// A row was dragged to another position.
    RowsReordered { from: usize, to: usize },
    /// A presentation setting was changed for the given rows.
    RowSettingChanged {
        indices: Vec<usize>,
        setting: Settings,
    },

    /// A dump was opened: it becomes a tab (or, when the same identity is
    /// already open, that tab is focused). The store owns the handle and derives
    /// the time range; the identity (`name` + `bytes`) is what the remembered row
    /// list is keyed by, so reopening the same file restores its rows.
    DumpAdded { id: DumpId, handle: DumpHandle },
    /// A dump tab was clicked: it becomes the active dump.
    DumpTabActivated { index: usize },
    /// A dump tab was closed. Closing the active tab activates its neighbour;
    /// closing the last one returns to the empty state.
    DumpClosed { index: usize },
    /// The zoom/pan was committed (one dispatch per gesture, never per move).
    ViewCommitted(View),
    /// The cursors were set or cleared by a click.
    CursorsChanged(Cursors),

    /// A source file was read: it becomes an open editor document (and an LSP
    /// document). Re-opening a file that is already open replaces its text.
    SourceOpened { file: String, text: String },
    /// An already open source was read again (the picker re-read the file).
    SourceReloaded { file: String, text: String },
    /// A tab was closed: the document leaves the editor and the LSP table, but
    /// stays in the design database, so declarations keep resolving.
    SourceClosed { index: usize },
    /// A tab was clicked.
    TabActivated { index: usize },
    /// The inspector selection changed (the signal selected in the wave view).
    FocusChanged { focus: Focus },
    /// A position was asked for (a declaration, a driver/load, an LSP answer).
    /// The reducer validates the file is open and assigns the token.
    JumpRequested { file: String, line: u32, col: u32 },
    /// The editor applied jump `token`; a matching pending request is cleared.
    JumpHandled { token: u32 },
    /// The elaborated top module changed (the app reads it from the design).
    TopChanged { top: Option<String> },

    /// Several signals cover a click: the picker shows the candidates.
    PickerOpened { title: String, paths: Vec<String> },
    /// The user picked candidate `index`. The store closes the picker; the app
    /// resolves the path and dispatches the row (it owns the dump/design).
    PickerChosen { index: usize },
    /// The picker was dismissed.
    PickerCancelled,
    /// The settings dialog opened for these rows.
    DialogOpened { indices: Vec<usize> },
    /// The settings dialog closed (applied, deleted or dismissed).
    DialogClosed,
}

/// Work the reducer may not do itself: I/O, the LSP, the editor.
///
/// `reduce` returns these and `App` drains them after the dispatch
/// (docs/state-store.md §2.5). The store is the single owner of the open
/// documents; the bridge's document table is a *mirror* kept in sync by the
/// first three variants, which is what keeps
/// `window.__waveInsightLsp.stats().documents` reporting the open-document
/// count (the QA matrix asserts it: two after two sources, one after a close).
#[derive(Debug, PartialEq)]
pub enum Effect {
    /// `textDocument/didOpen` for a document that just became open.
    LspOpen { file: String, text: String },
    /// `textDocument/didChange` for a document whose buffer changed.
    LspChange { file: String, text: String },
    /// `textDocument/didClose` for a document that is no longer open.
    LspClose { file: String },
    /// The server is told which module to elaborate from.
    LspTop { top: Option<String> },
    /// A jump was requested. The `nav.goto` slice already drives the editor; this
    /// variant is the seam step 4 uses to focus the editor window explicitly.
    MonacoShow { file: String, line: u32, col: u32 },
}
