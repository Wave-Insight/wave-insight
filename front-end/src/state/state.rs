//! The whole state of the app, one struct.
//!
//! R2 step 2 (B1) adds the wave rows here (order, per-row settings and the
//! multi-selection); the decoded values stay with the renderer/backend cache.
//! The sources, design, dump and viewport slices are added by the following
//! steps (see `docs/state-store.md` §2.3).

use std::rc::Rc;

use wave_insight_lib::waveform::TimeRange;

use super::types::{Layout, Row, RowsSlice, Settings, Status, Viewport};

/// The wave rows, versioned so a subscriber compares one integer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rows {
    pub items: Rc<Vec<Row>>,
    /// Bumped on every change; the subscription compares this, never the rows.
    pub version: u64,
}

impl Rows {
    /// The snapshot the renderer subscribes to.
    pub fn slice(&self) -> RowsSlice {
        RowsSlice { version: self.version, items: Rc::clone(&self.items) }
    }
}

/// The opened dump's derived data. The handle itself is still a prop until the
/// following step; what the panes need from it today is the time range.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dump {
    pub time_range: Option<TimeRange>,
}

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
    pub rows: Rows,
    pub dump: Dump,
    pub viewport: Viewport,
}

impl State {
    /// Replace the rows and bump the version: every row action funnels through
    /// here, so a subscription sees exactly one change per action.
    pub(crate) fn set_rows(&mut self, items: Vec<Row>) {
        self.rows.items = Rc::new(items);
        self.rows.version = self.rows.version.wrapping_add(1);
    }

    /// The settings a newly added row starts with.
    pub(crate) fn default_row_setting() -> Settings {
        Settings::new()
    }
}
