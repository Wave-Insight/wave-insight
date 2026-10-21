//! The whole state of the app, one struct.
//!
//! R2 step 2 (B1) adds the wave rows here (order, per-row settings and the
//! multi-selection); step 3 moves the opened dump in (the handle is the source
//! of truth, the time range is derived by the reducer). The decoded values stay
//! with the renderer/backend cache, and the sources/design slices arrive in the
//! next increment (see `docs/state-store.md` §2.3).

use std::rc::Rc;

use super::types::{Dump, Layout, Row, RowsSlice, Settings, Status, Viewport};

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
