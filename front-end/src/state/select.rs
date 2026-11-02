//! Named selectors: the slices components subscribe to.
//!
//! Keeping them here (instead of inline closures at every call site) means the
//! granularity of each subscription is one place to review: a component can
//! only re-render on the slices listed in `docs/state-store.md` §2.2.

use super::state::State;
use wave_insight_lib::waveform::TimeRange;

use super::dumps::DumpsTabsSlice;
use super::sources::SourcesSlice;
use super::split::Split;
use super::types::{
    Cursors, Dialog, DumpHandle, Focus, Goto, Layout, Picker, RowsSlice, SessionRestore, Status,
    TreeSelection, View, Viewport,
};

/// The two CSS variables and the drawer flag. Subscribed by `App` only.
pub fn layout(state: &State) -> Layout {
    state.layout
}

/// The top bar line. Subscribed by `TopBar` only.
pub fn status(state: &State) -> Option<Status> {
    state.nav.status.clone()
}

/// Language-server problems in the open sources (top bar chip).
pub fn problems(state: &State) -> u32 {
    state.nav.problems
}

/// The drawer flag alone, for anything that only cares whether the sidebar is
/// visible (e.g. the splitter).
#[allow(dead_code)] // used by the store tests and by the step-4 splitter
pub fn drawer_open(state: &State) -> bool {
    state.layout.drawer_open
}

/// The sidebar width alone.
#[allow(dead_code)] // the step-4 splitter subscribes to this
pub fn sidebar_width(state: &State) -> f64 {
    state.layout.sidebar_w
}

/// The editor ratio alone.
#[allow(dead_code)] // the step-4 editor/wave splitter subscribes to this
pub fn editor_ratio(state: &State) -> f32 {
    state.layout.editor_ratio
}

/// The wave rows (order, settings and selection) of the active dump, or the
/// no-dump scratch list. Subscribed by the wave pane.
pub fn rows(state: &State) -> RowsSlice {
    state.rows_slice()
}

/// The module-tree multi-selection (bulk add). Subscribed by the sidebar.
pub fn tree_selection(state: &State) -> TreeSelection {
    state.nav.tree_selection.clone()
}

/// The side-by-side split, when it is open.
pub fn split(state: &State) -> Option<Split> {
    state.split
}

/// The rows of one specific dump (the split's right pane); the active-dump
/// selectors above are unchanged.
pub fn rows_of(state: &State, dump: usize) -> RowsSlice {
    state
        .dumps
        .items
        .get(dump)
        .map(|entry| entry.rows.slice())
        .unwrap_or_default()
}

/// The committed viewport of one specific dump.
pub fn viewport_of(state: &State, dump: usize) -> Viewport {
    state
        .dumps
        .items
        .get(dump)
        .map(|entry| entry.viewport)
        .unwrap_or_default()
}

/// The time range of one specific dump.
pub fn time_range_of(state: &State, dump: usize) -> Option<TimeRange> {
    state
        .dumps
        .items
        .get(dump)
        .and_then(|entry| entry.time_range)
}

/// The file name of one specific dump.
pub fn dump_name_of(state: &State, dump: usize) -> Option<String> {
    state
        .dumps
        .items
        .get(dump)
        .map(|entry| entry.id.name.clone())
}

/// The handle of one specific dump (the split's right pane decodes from it).
pub fn dump_waveform_of(state: &State, dump: usize) -> Option<DumpHandle> {
    state
        .dumps
        .items
        .get(dump)
        .map(|entry| entry.handle.clone())
}

/// The dump tab strip: the tab list and the active index, and nothing else.
/// Subscribed by the shell; its version moves only when the tabs or the active
/// index change, never on a viewport/cursor/focus commit (tier-A gate).
pub fn dump_tabs(state: &State) -> DumpsTabsSlice {
    state.dumps.tabs_slice()
}

/// The open source documents and the active tab. Subscribed by the editor.
pub fn sources(state: &State) -> SourcesSlice {
    state.sources.slice()
}

/// The inspector selection of the active dump (or the no-dump scratch one).
pub fn focus(state: &State) -> Focus {
    state.focus_slice()
}

/// The path picker, when it is open. Subscribed by the shell.
pub fn picker(state: &State) -> Option<Picker> {
    state.nav.picker.clone()
}

/// The settings dialog, when it is open. Subscribed by the wave pane.
pub fn dialog(state: &State) -> Option<Dialog> {
    state.nav.dialog.clone()
}

/// The sidebar tree (the active dump's hierarchy or the elaborated design).
/// Subscribed by the sidebar.
pub fn tree(state: &State) -> Option<crate::nav::SidebarTree> {
    state.tree.clone()
}

/// The design database's version; a component that queries `State.design`
/// subscribes to this to re-render when a source was parsed.
pub fn design_version(state: &State) -> u64 {
    state.design.version()
}

/// The pending (or last) jump request. Subscribed by the editor.
pub fn goto(state: &State) -> Option<Goto> {
    state.nav.goto.clone()
}

/// The committed viewport and cursors of the active dump (one subscription for
/// both: the wave pane mirrors them for drawing, the time bar renders from them).
pub fn viewport(state: &State) -> Viewport {
    state.viewport_slice()
}

/// The zoom/pan alone.
#[allow(dead_code)] // the pane subscribes to the combined viewport
pub fn view(state: &State) -> View {
    state.viewport_slice().view
}

/// The cursors alone.
pub fn cursors(state: &State) -> Cursors {
    state.viewport_slice().cursors
}

/// The active dump's time range (ruler + fit-to-trace).
pub fn time_range(state: &State) -> Option<TimeRange> {
    state.dumps.active().and_then(|entry| entry.time_range)
}

/// The file the active dump came from, when the transport knew one.
pub fn dump_name(state: &State) -> Option<String> {
    state.dumps.active().map(|entry| entry.id.name.clone())
}

/// The active dump itself. Subscribed by the wave pane (decoding) and mirrored
/// into the inlay-hint context; the handle compares by identity.
pub fn dump_waveform(state: &State) -> Option<DumpHandle> {
    state.dumps.active().map(|entry| entry.handle.clone())
}

/// What opening the active dump restored from the remembered session (the app
/// turns the skipped count into one line of status text).
pub fn session_restore(state: &State) -> Option<SessionRestore> {
    state.dumps.active().and_then(|entry| entry.session_restore)
}
