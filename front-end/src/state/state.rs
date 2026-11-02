//! The whole state of the app, one struct.
//!
//! R2 step 2 (B1) adds the wave rows here (order, per-row settings and the
//! multi-selection); step 3 moves the opened dump in (the handle is the source
//! of truth, the time range is derived by the reducer). The decoded values stay
//! with the renderer/backend cache, and the sources/design slices arrive in the
//! next increment (see `docs/state-store.md` §2.3).

use std::rc::Rc;

use super::design::DesignState;
use super::dumps::Dumps;
use super::persist::{RowPrefs, SessionRow, Sessions};
use super::sources::{Source, Sources};
use super::split::Split;
use super::types::{
    Dialog, Focus, Goto, Layout, Picker, Row, RowsSlice, Status, TreeSelection, Viewport,
};

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
        RowsSlice {
            version: self.version,
            items: Rc::clone(&self.items),
        }
    }
}

/// State that is not read on every frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nav {
    /// One line of feedback for the top bar; `None` hides the chip.
    pub status: Option<Status>,
    /// Language-server problems in the open sources; the top bar shows a
    /// chip when it is non-zero. Kept out of `status` so the two cannot
    /// overwrite each other.
    pub problems: u32,
    /// The pending (or last) jump request. The token makes a repeat observable.
    pub goto: Option<Goto>,
    /// Counter the next jump token is taken from; the store owns it so the shell
    /// no longer keeps one.
    pub jump_token: u32,
    /// The module the language server elaborates from, mirrored to it through
    /// [`super::action::Effect::LspTop`].
    pub top: Option<String>,
    /// The path picker, when the user has to choose between candidates.
    pub picker: Option<Picker>,
    /// The settings dialog, when it is open.
    pub dialog: Option<Dialog>,
    /// The module-tree multi-selection (bulk add). `focus.instance` stays the
    /// single selection the inspector follows.
    pub tree_selection: TreeSelection,
}

/// The single source of truth.
// The design database is not Clone/PartialEq (it owns parsed source text and the
// elaboration cache), and nothing compares whole states: subscribers compare
// slices. `Default` is all the store needs to build one.
#[derive(Default)]
pub struct State {
    pub layout: Layout,
    pub nav: Nav,
    /// The open dumps (one tab each) with their own rows/viewport/focus.
    pub dumps: Dumps,
    /// The rows of a session with **no dump open** (the sources-only workflow).
    /// The selectors prefer the active dump's entry when there is one; these
    /// scratch slices are what the panes read before the first dump is opened.
    pub rows: Rows,
    /// The no-dump viewport (the wave pane is hidden until a dump is open).
    pub viewport: Viewport,
    /// The per-signal display settings remembered across sessions (see
    /// [`super::persist`]); the reducer applies them when a row is added and
    /// records them when one changes, the store persists them.
    pub row_prefs: RowPrefs,
    /// The row lists remembered per dump identity (see [`super::persist`]). The
    /// reducer restores one when the same file is opened again and records the
    /// current list on every row change; the store persists it.
    pub sessions: Sessions,
    /// The open source documents, their buffer text and the active tab.
    pub sources: Sources,
    /// The inspector selection of a session with no dump open (see `rows`); the
    /// active dump's entry owns the selection while one is open.
    pub focus: Focus,
    /// The design database and its elaboration cache. The reducer owns it: every
    /// source action updates it here and rebuilds the sidebar tree below.
    pub design: DesignState,
    /// The sidebar tree, derived in the reducer from the opened dump (its scope
    /// hierarchy wins) or from the elaborated design. A versioned cache: the
    /// components subscribe instead of rebuilding it per render.
    pub tree: Option<crate::nav::SidebarTree>,
    /// The side-by-side split: which dump the read-only right pane shows and
    /// whether the two panes share a time window.
    pub split: Option<Split>,
}

impl State {
    /// Replace the rows and bump the version: every row action funnels through
    /// here, so a subscription sees exactly one change per action.
    ///
    /// It is also the single place the row list of the open dump is recorded
    /// (see [`State::remember_rows`]), so no row action can forget to persist.
    pub(crate) fn set_rows(&mut self, items: Vec<Row>) {
        if self.dumps.active().is_some() {
            let version = self.dumps.next_rows_version();
            let entry = self.dumps.active_mut().expect("checked above");
            entry.rows.items = Rc::new(items);
            entry.rows.version = version;
            let id = entry.id.clone();
            self.remember_rows(&id);
            self.dumps.touch();
        } else {
            self.rows.items = Rc::new(items);
            self.rows.version = self.rows.version.wrapping_add(1);
        }
    }

    /// The rows the panes see right now: the active dump's, or the no-dump
    /// scratch list.
    pub(crate) fn rows_items(&self) -> Vec<Row> {
        match self.dumps.active() {
            Some(entry) => (*entry.rows.items).clone(),
            None => (*self.rows.items).clone(),
        }
    }

    /// The row snapshot the renderer subscribes to.
    pub(crate) fn rows_slice(&self) -> RowsSlice {
        match self.dumps.active() {
            Some(entry) => entry.rows.slice(),
            None => self.rows.slice(),
        }
    }

    /// The inspector selection: the active dump's, or the no-dump scratch one.
    pub(crate) fn focus_slice(&self) -> Focus {
        match self.dumps.active() {
            Some(entry) => entry.focus.clone(),
            None => self.focus.clone(),
        }
    }

    /// Set the inspector selection on the active dump (or the scratch slice).
    pub(crate) fn set_focus(&mut self, focus: Focus) {
        match self.dumps.active_mut() {
            Some(entry) => entry.focus = focus,
            None => self.focus = focus,
        }
    }

    /// The committed viewport: the active dump's, or the no-dump scratch one.
    pub(crate) fn viewport_slice(&self) -> Viewport {
        match self.dumps.active() {
            Some(entry) => entry.viewport,
            None => self.viewport,
        }
    }

    /// Set the committed viewport on the active dump (or the scratch slice).
    pub(crate) fn set_viewport(&mut self, viewport: Viewport) {
        match self.dumps.active_mut() {
            Some(entry) => {
                entry.viewport = viewport;
                self.dumps.touch();
            }
            None => self.viewport = viewport,
        }
    }

    /// Replace the open source documents and the active tab, bumping the
    /// version: every source action funnels through here, so a subscription
    /// sees exactly one change per action.
    pub(crate) fn set_sources(&mut self, items: Vec<Source>, active: usize) {
        self.sources.items = Rc::new(items);
        self.sources.active = active;
        self.sources.version = self.sources.version.wrapping_add(1);
    }

    /// Record the current row list under the open dump's identity, so reopening
    /// the same file restores it.
    ///
    /// A dump with no name has no identity (it arrived as raw bytes) and nothing
    /// is remembered for it; with no dump open there is nothing to key the rows
    /// by either.
    fn remember_rows(&mut self, id: &super::types::DumpId) {
        let rows = match self.dumps.active() {
            Some(entry) => (*entry.rows.items).clone(),
            None => return,
        };
        let rows = rows.iter().map(SessionRow::from_row).collect();
        self.sessions.record(id.clone(), rows);
    }
}
