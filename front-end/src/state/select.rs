//! Named selectors: the slices components subscribe to.
//!
//! Keeping them here (instead of inline closures at every call site) means the
//! granularity of each subscription is one place to review: a component can
//! only re-render on the slices listed in `docs/state-store.md` §2.2.

use super::state::State;
use super::types::{Layout, Status};

/// The two CSS variables and the drawer flag. Subscribed by `App` only.
pub fn layout(state: &State) -> Layout {
    state.layout
}

/// The top bar line. Subscribed by `TopBar` only.
pub fn status(state: &State) -> Option<Status> {
    state.nav.status.clone()
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
