//! The open dumps: one entry per tab, with the rows, the viewport and the
//! inspector focus that belong to it.
//!
//! R2/Wave 8 (docs/design/multi-dump.md): the wave rows, the committed
//! viewport/cursors and the inspector selection are *per dump*. This module
//! owns that list and the active index; the selectors in [`super::select`] read
//! the active entry, so the renderer, the time bar and the wave pane keep their
//! current shape.
//!
//! Re-opening a dump whose identity (name + byte size) is already open focuses
//! its tab instead of adding a second one. When the file was re-written under
//! the same name and size (a re-simulated dump), the content fingerprint of the
//! new bytes differs and the reducer replaces the entry's handle in place, so
//! the tab reloads the new waveform and keeps its rows/settings/viewport.
//!
//! A session with no dump open (the sources-only workflow: load .sv files, add
//! signals, no waveform yet) keeps using the `State.rows`/`viewport`/`focus`
//! scratch slices; [`super::select`] prefers the active entry when there is one.
//! That is a deliberate deviation from the design note's "replace the fields"
//! wording: the row list must keep working before the first dump is opened
//! (the `sourcesOnly` QA scenario adds a row with no dump).

use std::rc::Rc;

use super::state::Rows;
use super::types::{DumpHandle, DumpId, Focus, SessionRestore, Viewport};
use wave_insight_lib::waveform::TimeRange;

/// One open dump: the handle (the only copy), its derived time range and the
/// slices that belong to it.
#[derive(Clone, Debug)]
pub struct DumpEntry {
    /// File name + byte size: what the remembered row list is keyed by.
    pub id: DumpId,
    pub handle: DumpHandle,
    pub time_range: Option<TimeRange>,
    pub range_from_body: bool,
    /// Content fingerprint of the bytes that were parsed (see
    /// [`crate::file_load::fingerprint`]), when the transport reports one.
    ///
    /// The identity above is name + size; a re-simulated dump can keep both and
    /// still change every value. Opening such a file again compares this
    /// fingerprint with the open entry's and reloads the tab in place when it
    /// differs, instead of silently keeping the old waveform.
    pub fingerprint: Option<u64>,
    /// The wave rows of this dump (order, per-row settings, selection).
    pub rows: Rows,
    /// The committed zoom/pan and cursors of this dump.
    pub viewport: Viewport,
    /// The inspector selection while this dump is active.
    pub focus: Focus,
    /// What opening this dump restored from the remembered session (the app
    /// turns the skipped count into one line of status text).
    pub session_restore: Option<SessionRestore>,
}

impl DumpEntry {
    /// A fresh entry: no rows, the default viewport and no selection.
    pub fn new(
        id: DumpId,
        handle: DumpHandle,
        time_range: Option<TimeRange>,
        range_from_body: bool,
        fingerprint: Option<u64>,
    ) -> Self {
        Self {
            id,
            handle,
            time_range,
            range_from_body,
            fingerprint,
            rows: Rows::default(),
            viewport: Viewport::default(),
            focus: Focus::default(),
            session_restore: None,
        }
    }
}

/// One tab of the dump strip: everything the shell needs to render it (no
/// handle, no rows, no viewport).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumpTab {
    /// File name the loader read; empty for a dump that arrived as raw bytes.
    pub name: String,
    /// File size, when the loader reported one (the tab tooltip and the memory
    /// readout use it).
    pub bytes: Option<u64>,
}

/// The versioned snapshot the shell subscribes to.
///
/// It carries only the tab list and the active index, and its version is bumped
/// *only* when those change: a cursor or zoom commit changes the active entry's
/// viewport, which must never re-render the shell (the tier-A render-count gate
/// counts shell renders during a gesture).
#[derive(Clone, Debug, Default)]
pub struct DumpsTabsSlice {
    pub version: u64,
    pub items: Rc<Vec<DumpTab>>,
    pub active: usize,
    /// Summed file bytes of the open dumps, for the strip tooltip/status line.
    pub total_bytes: u64,
}

impl DumpsTabsSlice {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn active_index(&self) -> usize {
        self.active.min(self.items.len().saturating_sub(1))
    }
}

impl PartialEq for DumpsTabsSlice {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

/// The store's owned form: the entries, the active index and one version.
#[derive(Clone, Debug, Default)]
pub struct Dumps {
    pub items: Rc<Vec<DumpEntry>>,
    pub active: usize,
    /// Bumped on every change, so a subscriber compares one integer.
    pub version: u64,
    /// A store-wide sequence for `Rows.version`. The per-entry row versions must
    /// be unique across dumps: a subscriber compares only the version, so two
    /// tabs whose lists happen to be at the same version would not re-render
    /// when the active tab changes.
    pub row_seq: u64,
    /// Bumped only when the tab list or the active index changes (never by a
    /// viewport/cursor/focus/rows change): the shell's tab strip subscribes to
    /// this, so a gesture commit cannot re-render the shell.
    pub tabs_version: u64,
}

impl Dumps {
    /// The tab-only snapshot the shell renders (cheap: one `Rc` clone).
    pub fn tabs_slice(&self) -> DumpsTabsSlice {
        DumpsTabsSlice {
            version: self.tabs_version,
            items: Rc::new(
                self.items
                    .iter()
                    .map(|entry| DumpTab {
                        name: entry.id.name.clone(),
                        bytes: entry.id.bytes,
                    })
                    .collect(),
            ),
            active: self.active.min(self.items.len().saturating_sub(1)),
            total_bytes: self.total_bytes(),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The active entry, clamped (a snapshot can be one render behind a close).
    pub fn active(&self) -> Option<&DumpEntry> {
        self.items.get(self.active_index())
    }

    pub fn active_index(&self) -> usize {
        self.active.min(self.items.len().saturating_sub(1))
    }

    pub fn active_mut(&mut self) -> Option<&mut DumpEntry> {
        let index = self.active_index();
        self.items_mut().get_mut(index)
    }

    /// `Rc::make_mut`: the entries are cloned only when another slice is alive.
    pub fn items_mut(&mut self) -> &mut Vec<DumpEntry> {
        Rc::make_mut(&mut self.items)
    }

    /// The index of the tab already showing `id` (the dedupe rule: opening the
    /// same file again focuses its tab instead of adding a second copy).
    pub fn index_of(&self, id: &DumpId) -> Option<usize> {
        self.items.iter().position(|entry| entry.id.matches(id))
    }

    /// Summed file bytes of the open dumps, where the loader reported a size.
    pub fn total_bytes(&self) -> u64 {
        self.items
            .iter()
            .filter_map(|entry| entry.id.bytes)
            .fold(0u64, u64::saturating_add)
    }

    /// Bump the version after a change.
    pub fn touch(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    /// Bump the tab-strip version: only the tab list or the active index changed.
    pub fn touch_tabs(&mut self) {
        self.tabs_version = self.tabs_version.wrapping_add(1);
    }

    /// A fresh, globally unique version for a dump's row list.
    pub fn next_rows_version(&mut self) -> u64 {
        self.row_seq = self.row_seq.wrapping_add(1);
        self.row_seq
    }
}

impl PartialEq for Dumps {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}
