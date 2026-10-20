//! Two way navigation between the waveform, the source pane and the design
//! database.
//!
//! The modules here are glue, not logic: [`DesignState`] owns the
//! [`wave_insight_lib::design::DesignDb`] and the elaborated design and answers
//! the four questions the UI has ("where is this declared", "who drives it",
//! "what is under the cursor", "which signals match this word"), while
//! [`SignalInspector`] and [`PathPicker`] only render those answers.
//!
//! Everything the components exchange is cheap to clone and compare: the
//! database is behind a [`SharedDesign`] identity handle and the opened dump
//! behind a [`SharedWaveform`], so a re-render never deep compares a design.

pub use design_state::{DesignState, SignalDetails, SourceRow};
pub use inspector::SignalInspector;
pub use picker::PathPicker;
pub use tree::{SidebarTree, SignalRow};

mod design_state;
mod inspector;
mod picker;
mod tree;

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use wave_insight_lib::waveform::{SignalInfo, Waveform};

/// A request to bring one source position into view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goto {
    /// File name exactly as it was handed to `DesignDb::add_source`, which is
    /// also the label of the editor tab.
    pub file: String,
    /// 1 based line.
    pub line: u32,
    /// 1 based column; `0` means "no column, just the line".
    pub col: u32,
    /// Bumped for every request, so clicking the same declaration twice still
    /// scrolls the editor.
    pub token: u32,
}

/// A click on an identifier inside the source editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceClick {
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// The identifier the user clicked, used for the fallback lookup when the
    /// elaborated design has nothing at that exact position.
    pub word: String,
}

/// Shared handle on the [`DesignState`].
///
/// Compares by identity, not by content: re-rendering the app must not walk a
/// whole design database, and the version counter is what tells the inspector
/// that the content behind the handle changed.
#[derive(Clone)]
pub struct SharedDesign(Rc<RefCell<DesignState>>);

impl SharedDesign {
    pub fn new(state: DesignState) -> Self {
        Self(Rc::new(RefCell::new(state)))
    }

    pub fn borrow(&self) -> Ref<'_, DesignState> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, DesignState> {
        self.0.borrow_mut()
    }
}

impl PartialEq for SharedDesign {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for SharedDesign {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.0.borrow();
        f.debug_struct("SharedDesign")
            .field("version", &state.version())
            .field("top", &state.top())
            .finish()
    }
}

impl Default for SharedDesign {
    fn default() -> Self {
        Self::new(DesignState::new())
    }
}

/// Shared handle on the opened waveform file.
///
/// [`Waveform`] has no `PartialEq` (it caches decoded data behind a mutex), so
/// the handle compares by pointer: it only changes when another file is opened.
#[derive(Clone)]
pub struct SharedWaveform(Rc<Waveform>);

impl SharedWaveform {
    pub fn new(waveform: Waveform) -> Self {
        Self(Rc::new(waveform))
    }

    pub fn as_waveform(&self) -> &Waveform {
        &self.0
    }

    pub fn signal_by_path(&self, path: &str) -> Option<&SignalInfo> {
        self.0.signal_by_path(path)
    }
}

impl PartialEq for SharedWaveform {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for SharedWaveform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedWaveform")
            .field("format", &self.0.file_format())
            .field("signals", &self.0.signal_count())
            .finish()
    }
}
