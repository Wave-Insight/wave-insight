//! Named selectors: the slices components subscribe to.
//!
//! Keeping them here (instead of inline closures at every call site) means the
//! granularity of each subscription is one place to review: a component can
//! only re-render on the slices listed in `docs/state-store.md` §2.2.

use super::state::State;
use wave_insight_lib::waveform::TimeRange;

use super::sources::SourcesSlice;
use super::types::{Cursors, DumpHandle, Focus, Goto, Layout, RowsSlice, Status, View, Viewport};

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

/// The wave rows (order, settings and selection). Subscribed by the wave pane.
pub fn rows(state: &State) -> RowsSlice {
    state.rows.slice()
}

/// The open source documents and the active tab. Subscribed by the editor.
pub fn sources(state: &State) -> SourcesSlice {
    state.sources.slice()
}

/// The inspector selection.
pub fn focus(state: &State) -> Focus {
    state.focus.clone()
}

/// The pending (or last) jump request. Subscribed by the editor.
pub fn goto(state: &State) -> Option<Goto> {
    state.nav.goto.clone()
}

/// The committed viewport and cursors (one subscription for both: the wave pane
/// mirrors them for drawing, the time bar renders from them).
pub fn viewport(state: &State) -> Viewport {
    state.viewport
}

/// The zoom/pan alone.
#[allow(dead_code)] // the pane subscribes to the combined viewport
pub fn view(state: &State) -> View {
    state.viewport.view
}

/// The cursors alone.
pub fn cursors(state: &State) -> Cursors {
    state.viewport.cursors
}

/// The opened dump's time range (ruler + fit-to-trace).
pub fn time_range(state: &State) -> Option<TimeRange> {
    state.dump.time_range
}

/// The file the opened dump came from, when the transport knew one.
pub fn dump_name(state: &State) -> Option<String> {
    state.dump.name.clone()
}

/// The opened dump itself. Subscribed by the wave pane (decoding) and mirrored
/// into the inlay-hint context; the handle compares by identity.
pub fn dump_waveform(state: &State) -> Option<DumpHandle> {
    state.dump.waveform.clone()
}
