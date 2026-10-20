//! The rows of the wave view, as the *renderer* sees them.
//!
//! R2 step 2 (B1) moved the row list, its order, the per-row settings and the
//! multi-selection into the store (`state::Rows`). This type is no longer the
//! owner: it is a view model the component keeps in sync from the store
//! snapshot ([`SignalThings::sync`]), plus the one thing the store must not own —
//! the decoded values.
//!
//! A row keeps a *handle* to its values, never a copy: the backend's
//! [`DecodedSignal`] is sliced per frame by the canvas
//! ([`DecodedSignal::window`]), so a redraw neither touches the dump nor clones
//! a value. Decoding is done by the component in one batched call
//! (`Waveform::decoded_changes_many`) for the rows that are on screen, once.

use std::ops::{Index, IndexMut};
use std::slice::{Iter, SliceIndex};

use wave_insight_lib::waveform::{DecodedSignal, SignalKind, WaveValue, Waveform};

use crate::state::{Row, Settings};
use crate::wave_show::geometry::{self, Time};

/// What kind of waveform a row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// No values (yet): a muted "no data" line.
    Empty,
    /// One bit: a digital step, with `x`/`z` on the middle level.
    Bool,
    /// A bus: a band with a value label when the segment is wide enough.
    Bits,
}

/// Values of one row.
#[derive(Clone, Default)]
pub enum RowValues {
    /// No dump, the signal is not in the dump, or the row was dropped from the
    /// decode cache to stay inside the memory budget.
    #[default]
    Empty,
    /// A cached, decoded history owned by the backend. Cheap to clone (one
    /// `Arc` bump) and cheap to slice.
    Backend(DecodedSignal),
}

impl RowValues {
    /// `true` if there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        match self {
            RowValues::Empty => true,
            RowValues::Backend(decoded) => decoded.is_empty(),
        }
    }

    /// Which shape this row is drawn as.
    pub fn kind(&self) -> RowKind {
        match self {
            RowValues::Empty => RowKind::Empty,
            RowValues::Backend(decoded) => match decoded.first_change().map(|(_, value)| value) {
                Some(value) if value.width() == Some(1) => RowKind::Bool,
                Some(_) => RowKind::Bits,
                None => RowKind::Empty,
            },
        }
    }

    /// The value in effect at `time`.
    pub fn value_at(&self, time: Time) -> Option<&WaveValue> {
        match self {
            RowValues::Empty => None,
            RowValues::Backend(decoded) => decoded.value_at(time),
        }
    }

    /// Visit the changes of this row inside `start ..= end`, thinned down to one
    /// per pixel of a `width` px surface. Nothing is allocated.
    pub fn for_each_change<'a>(
        &'a self,
        start: Time,
        end: Time,
        view: &geometry::View,
        width: f64,
        mut visit: impl FnMut(f64, &'a WaveValue),
    ) {
        let RowValues::Backend(decoded) = self else {
            return;
        };
        let window = decoded.window(start, Some(end));
        geometry::for_each_pixel_change(window, view, width, |x, (_, value)| visit(x, value));
    }

    /// Bit count to format values with: the declared (or dump) width, clamped
    /// to what the values really hold.
    ///
    /// A dump can be narrower than the design (a truncated vector, a `$var`
    /// that is narrower than the declared port), and the formatter must not be
    /// asked for more bits than the value has.
    pub fn format_width(&self, declared: usize) -> usize {
        let declared = declared.max(1);
        match self {
            RowValues::Empty => declared,
            RowValues::Backend(decoded) => decoded
                .first_change()
                .and_then(|(_, value)| value.width())
                .map(|width| declared.min(width.max(1)))
                .unwrap_or(declared),
        }
    }
}

/// One row of the wave view, as the renderer draws it.
///
/// Built by [`SignalThings::sync`] from a store [`Row`]; never mutated by the
/// UI (the store is the owner now).
pub struct SignalItem {
    /// Display name, including the `[n:0]` suffix for buses.
    pub name: String,
    /// The row's identity in the store (the hierarchical path).
    pub path: String,
    /// Width the design declares (the suffix is derived from it).
    pub declared: usize,
    /// Index in the opened waveform, resolved once so a draw never hashes a path.
    pub index: Option<usize>,
    /// The dump's width for this row (falls back to the declared width).
    pub width: usize,
    /// Draw this row as a single bit (taken from the dump when there is one).
    pub is_bool: bool,
    pub setting: Settings,
    pub load: Vec<String>,
    pub driver: Vec<String>,
    pub choose: bool,
    pub values: RowValues,
}

impl SignalItem {
    /// Bit count to format this row's values with: the row width, clamped to
    /// what the values really hold.
    pub fn format_width(&self) -> usize {
        self.values.format_width(self.width)
    }
}

/// Resolution of one row against the opened dump.
struct Resolved {
    index: Option<usize>,
    width: usize,
    is_bool: bool,
}

/// Look a row up in the dump. Only the hierarchy is read here (cheap, cached by
/// the library), no values are decoded.
fn resolve(path: &str, declared: usize, waveform: Option<&Waveform>) -> Resolved {
    let declared = declared.max(1);
    let Some(info) = waveform.and_then(|waveform| waveform.signal_by_path(path)) else {
        //no dump (or the signal is not in it): fall back to the design width
        return Resolved {
            index: None,
            width: declared,
            is_bool: declared == 1,
        };
    };
    Resolved {
        index: Some(info.index()),
        width: (info.width() as usize).max(1),
        //the dump decides what a row is drawn as: a source can declare a wider
        //vector than the trace really holds (and vice versa)
        is_bool: info.kind() == SignalKind::BitVector && info.width() == 1,
    }
}

/// The rows the renderer draws: a local view model synced from the store.
pub struct SignalThings {
    item: Vec<SignalItem>,
}

impl SignalThings {
    pub fn new() -> Self {
        Self { item: Vec::new() }
    }

    /// Rebuild the view model from the store's rows.
    ///
    /// The store owns order/settings/selection; this method only maps them onto
    /// the render model, keeping the decoded values of rows that survived (they
    /// are keyed by path, not by index, so a reorder never drops a decode).
    ///
    /// Returns the signal indices whose rows disappeared: the caller unloads
    /// them from the backend cache.
    pub fn sync(&mut self, rows: &[Row], waveform: Option<&Waveform>) -> Vec<usize> {
        let mut previous = std::mem::take(&mut self.item);
        let mut next = Vec::with_capacity(rows.len());
        for row in rows {
            let existing = previous.iter().position(|item| item.path == row.path);
            let mut item = match existing {
                Some(index) => previous.remove(index),
                None => SignalItem {
                    name: String::new(),
                    path: row.path.clone(),
                    declared: row.width.max(1),
                    index: None,
                    width: row.width.max(1),
                    is_bool: row.width <= 1,
                    setting: Settings::new(),
                    load: Vec::new(),
                    driver: Vec::new(),
                    choose: false,
                    values: RowValues::Empty,
                },
            };
            let resolved = resolve(&row.path, row.width, waveform);
            item.path = row.path.clone();
            item.declared = row.width.max(1);
            item.index = resolved.index;
            item.width = resolved.width;
            item.is_bool = resolved.is_bool;
            item.name = if resolved.is_bool {
                row.name.clone()
            } else {
                format!("{}[{}:0]", row.name, item.declared - 1)
            };
            item.setting = row.setting.clone();
            item.load = row.load.clone();
            item.driver = row.drive.clone();
            item.choose = row.chosen;
            next.push(item);
        }
        // rows that are gone give their cached signal back to the backend
        let mut unload = Vec::new();
        for item in previous {
            if let (RowValues::Backend(_), Some(index)) = (&item.values, item.index) {
                unload.push(index);
            }
        }
        self.item = next;
        unload
    }

    /// Re-resolve every row against a new dump and drop the old values.
    ///
    /// Nothing is decoded here: the next redraw asks the backend for the rows
    /// that are actually visible, in one batched call.
    pub fn refresh(&mut self, waveform: Option<&Waveform>) {
        for item in &mut self.item {
            let resolved = resolve(&item.path, item.declared, waveform);
            item.index = resolved.index;
            item.width = resolved.width;
            item.is_bool = resolved.is_bool;
            item.values = RowValues::Empty;
        }
    }

    /// Rows in `rows` that still need a decode: `(row, signal index)`.
    pub fn pending(&self, rows: std::ops::Range<usize>) -> Vec<(usize, usize)> {
        let mut pending = Vec::new();
        for row in rows {
            let Some(item) = self.item.get(row) else {
                break;
            };
            if !item.values.is_empty() {
                continue;
            }
            if let Some(index) = item.index {
                pending.push((row, index));
            }
        }
        pending
    }

    /// Store the decoded history of one row.
    pub fn set_backend(&mut self, row: usize, decoded: DecodedSignal) {
        if let Some(item) = self.item.get_mut(row) {
            item.values = RowValues::Backend(decoded);
        }
    }

    /// Drop the values of every row outside `keep`, returning the signal indices
    /// that the caller should unload from the backend cache.
    pub fn drop_outside(&mut self, keep: std::ops::Range<usize>) -> Vec<usize> {
        let mut unload = Vec::new();
        for (row, item) in self.item.iter_mut().enumerate() {
            if keep.contains(&row) || matches!(item.values, RowValues::Empty) {
                continue;
            }
            if let (RowValues::Backend(_), Some(index)) = (&item.values, item.index) {
                unload.push(index);
            }
            item.values = RowValues::Empty;
        }
        unload
    }

    pub fn iter(&self) -> Iter<'_, SignalItem> {
        self.item.iter()
    }

    pub fn len(&self) -> usize {
        self.item.len()
    }

    pub fn get_choose_idx(&self) -> Vec<usize> {
        self.item
            .iter()
            .enumerate()
            .filter(|(_, item)| item.choose)
            .map(|(idx, _)| idx)
            .collect()
    }
}

impl Default for SignalThings {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: SliceIndex<[SignalItem]>> Index<I> for SignalThings {
    type Output = I::Output;

    #[inline]
    fn index(&self, index: I) -> &Self::Output {
        self.item.index(index)
    }
}

impl<I: SliceIndex<[SignalItem]>> IndexMut<I> for SignalThings {
    #[inline]
    fn index_mut(&mut self, index: I) -> &mut Self::Output {
        self.item.index_mut(index)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use wave_insight_lib::waveform::{Bits, Change};

    fn store_row(path: &str, width: usize) -> Row {
        Row {
            path: path.to_string(),
            name: path.to_string(),
            width,
            setting: Settings::new(),
            chosen: false,
            load: Vec::new(),
            drive: Vec::new(),
        }
    }

    fn decoded(width: usize, changes: usize) -> DecodedSignal {
        let value = WaveValue::Bits(Bits::all_unknown(width));
        let changes: Arc<[Change]> = (0..changes)
            .map(|index| (index as u64 * 10, value.clone()))
            .collect();
        DecodedSignal::from_changes(changes)
    }

    fn synced(paths: &[(&str, usize)]) -> SignalThings {
        let rows: Vec<Row> = paths.iter().map(|(path, width)| store_row(path, *width)).collect();
        let mut things = SignalThings::new();
        things.sync(&rows, None);
        things
    }

    #[test]
    fn sync_maps_the_store_rows_onto_the_render_model() {
        let things = synced(&[("top.clk", 1), ("top.pc", 8)]);
        assert_eq!(things.len(), 2);
        // a 1-bit signal has no suffix, a bus carries its declared width
        assert_eq!(things[0].name, "top.clk");
        assert!(things[0].is_bool);
        assert_eq!(things[1].name, "top.pc[7:0]");
        assert!(!things[1].is_bool);
        assert_eq!(things[1].declared, 8);
    }

    #[test]
    fn sync_keeps_the_values_of_rows_that_survive_a_reorder() {
        let mut things = synced(&[("a", 1), ("b", 1)]);
        things.set_backend(0, decoded(1, 3));
        // the store reorders: the decoded row must follow its path, not its index
        let rows = vec![store_row("b", 1), store_row("a", 1)];
        let unload = things.sync(&rows, None);
        assert!(unload.is_empty());
        assert_eq!(things[1].path, "a");
        assert!(!things[1].values.is_empty(), "the decode followed the row");
        assert!(things[0].values.is_empty());
    }

    #[test]
    fn sync_reports_the_signals_of_rows_that_are_gone() {
        let mut things = synced(&[("a", 1), ("b", 1)]);
        // pretend both were resolved against a dump
        things[0].index = Some(4);
        things[1].index = Some(5);
        things.set_backend(1, decoded(1, 2));
        let unload = things.sync(&[store_row("a", 1)], None);
        assert_eq!(unload, vec![5], "only the decoded, removed row is unloaded");
        assert_eq!(things.len(), 1);
    }

    #[test]
    fn sync_carries_settings_and_the_selection_over() {
        let mut rows = vec![store_row("a", 1)];
        rows[0].setting.fixed_active = true;
        rows[0].chosen = true;
        let mut things = SignalThings::new();
        things.sync(&rows, None);
        assert!(things[0].setting.fixed_active);
        assert!(things[0].choose);
        assert_eq!(things.get_choose_idx(), vec![0]);
    }

    #[test]
    fn pending_lists_only_rows_that_need_a_decode() {
        let mut things = synced(&[("a", 1), ("b", 1), ("c", 1), ("d", 1)]);
        things[0].index = Some(7);
        things[1].index = Some(8);
        things[3].index = Some(9);
        things.set_backend(1, decoded(1, 3));

        //only the empty rows with a signal index are worth asking the backend
        assert_eq!(things.pending(0..4), vec![(0, 7), (3, 9)]);
        //and only inside the requested range
        assert_eq!(things.pending(1..3), Vec::new());
        assert_eq!(things.pending(3..9), vec![(3, 9)]);
    }

    #[test]
    fn drop_outside_frees_rows_that_left_the_viewport() {
        let mut things = synced(&[("a", 1), ("b", 1), ("c", 1)]);
        things[0].index = Some(1);
        things[1].index = Some(2);
        things[2].index = Some(3);
        for row in 0..3 {
            things.set_backend(row, decoded(1, 2));
        }

        //keep row 1 only: the others go back to "no data" and are unloaded
        let unload = things.drop_outside(1..2);
        assert_eq!(unload, vec![1, 3]);
        assert!(!things[1].values.is_empty(), "row inside the window survives");
        assert!(things[0].values.is_empty(), "row before the window was dropped");
        assert!(things[2].values.is_empty(), "row after the window was dropped");
    }

    #[test]
    fn refresh_drops_the_values_and_re_resolves() {
        let mut things = synced(&[("a", 1)]);
        things[0].index = Some(3);
        things.set_backend(0, decoded(1, 2));
        things.refresh(None);
        assert!(things[0].values.is_empty());
        assert_eq!(things[0].index, None);
        assert!(things[0].is_bool);
    }

    #[test]
    fn format_width_clamps_to_what_the_values_hold() {
        //the dump is narrower than the design: format with the dump's width
        let values = RowValues::Backend(decoded(5, 1));
        assert_eq!(values.format_width(16), 5);
        //a wider value than declared keeps the declared width
        assert_eq!(values.format_width(3), 3);
        //no values at all: the declared width, never zero
        assert_eq!(RowValues::Empty.format_width(0), 1);
    }

    #[test]
    fn row_kind_follows_the_dump_width() {
        assert_eq!(RowValues::Empty.kind(), RowKind::Empty);
        assert_eq!(RowValues::Backend(decoded(1, 1)).kind(), RowKind::Bool);
        assert_eq!(RowValues::Backend(decoded(8, 1)).kind(), RowKind::Bits);
        //a vector with no changes has nothing to draw
        assert_eq!(RowValues::Backend(decoded(1, 0)).kind(), RowKind::Empty);
    }
}
