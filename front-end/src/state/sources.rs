//! The open source documents: the store's copy of everything the editor holds.
//!
//! R2 step 3 (docs/state-store.md §2.4): this slice replaces the app's own
//! source list. The store is the single owner of *which* documents are open, their buffer
//! text, their version and the active tab; the language server's document table
//! is a mirror that the reducer keeps in sync through [`Effect`]s
//! (`LspOpen`/`LspChange`/`LspClose`), executed by `App`.
//!
//! Scope of this step: the sources, the inspector focus and the jump request
//! (with its token counter in `State.nav`). The design slice,
//! `focus.instance` and the module-tree selection stay in
//! `App`/`ModuleStruct` until step 3b (that module is owned by another agent
//! this wave); `State.design` does not exist yet, so the reducer never reads
//! the elaborated design.
//!
//! The slice is versioned like [`super::state::Rows`]: cloning is one `Rc` bump
//! and comparing is one integer, so the editor only re-renders when the
//! documents really changed.

use std::rc::Rc;

/// The monaco language id every design source is opened as.
///
/// The language server is told the same string in `textDocument/didOpen`; the
/// editor opens its models with it too.
pub const SOURCE_LANGUAGE: &str = "systemverilog";

/// One open source document.
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    /// File name exactly as it was handed to `DesignState::add_source`, which
    /// is also the label of the editor tab.
    pub name: String,
    /// The buffer text the app last handed to the editor and the language
    /// server.
    pub text: String,
    /// The monaco language id (always [`SOURCE_LANGUAGE`] today).
    pub language: String,
    /// Bumped every time the text is replaced, so a reload is observable.
    pub version: u64,
}

impl Source {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            text: text.into(),
            language: SOURCE_LANGUAGE.to_string(),
            version: 1,
        }
    }
}

/// The versioned snapshot the panes subscribe to.
#[derive(Clone, Debug, Default)]
pub struct SourcesSlice {
    pub version: u64,
    pub items: Rc<Vec<Source>>,
    /// Index of the active tab; always a valid index when `items` is not empty.
    pub active: usize,
}

#[allow(dead_code)] // the wave/step-3b panes use the full surface
impl SourcesSlice {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The active tab, clamped to the list (a snapshot can be one render behind
    /// a close).
    pub fn active_index(&self) -> usize {
        self.active.min(self.items.len().saturating_sub(1))
    }

    pub fn active_source(&self) -> Option<&Source> {
        self.items.get(self.active_index())
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|source| source.name == name)
    }
}

impl PartialEq for SourcesSlice {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

/// The store's owned form of the slice.
#[derive(Clone, Debug, Default)]
pub struct Sources {
    pub items: Rc<Vec<Source>>,
    /// Bumped on every change; the subscription compares this, never the list.
    pub version: u64,
    /// Index of the active tab.
    pub active: usize,
}

#[allow(dead_code)] // `len`/`is_empty` are the selectors step 3b will use
impl Sources {
    pub fn slice(&self) -> SourcesSlice {
        SourcesSlice {
            version: self.version,
            items: Rc::clone(&self.items),
            active: self.active.min(self.items.len().saturating_sub(1)),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|source| source.name == name)
    }
}

impl PartialEq for Sources {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}
