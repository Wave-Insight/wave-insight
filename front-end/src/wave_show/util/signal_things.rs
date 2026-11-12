//! The rows of the wave view, as the *renderer* sees them.
//!
//! R2 step 2 (B1) moved the row list, its order, the per-row settings and the
//! multi-selection into the store (`state::Rows`). This type is no longer the
//! owner: it is a view model the component keeps in sync from the store
//! snapshot ([`SignalThings::sync`]), plus the one thing the store must not own —
//! the decoded values.
//!
//! A row keeps a *window*, not a history: Wave 13 replaced the whole-history
//! decode (`Waveform::decoded_changes_many`) with one bounded request per frame
//! ([`crate::wave_show::windowed`]) covering the visible time range plus a
//! margin, capped by the surface width. A dense clock therefore costs a few
//! thousand changes, not its whole history, and the memory is proportional to
//! the pixels instead of the dump.
//!
//! The canvas only needs the value at any time inside the fetched range (so the
//! left edge carries the value from before the window) and the changes inside
//! the visible range; [`RowValues`] exposes exactly that, so `canvas.rs` did not
//! change shape when the histories became windows.

use std::ops::{Index, IndexMut};
use std::slice::{Iter, SliceIndex};

use wave_insight_lib::waveform::{
    BitValue, Bits, Change, DecodedSignal, SignalKind, WaveValue, Waveform,
};

use crate::state::{Row, Settings};
use crate::wave_show::geometry::{self, Time};
use crate::wave_show::windowed::RowWindow;

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
    /// No dump, the signal is not in the dump, the row is outside the viewport,
    /// or the row was dropped from the window cache to stay inside the memory
    /// budget.
    #[default]
    Empty,
    /// The row's bounded window: the changes of the visible time range plus a
    /// margin, and the value carried into it. Cheap to clone (one `Arc` bump).
    Window(RowWindow),
}

impl RowValues {
    /// `true` if there is nothing to draw.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        match self {
            RowValues::Empty => true,
            RowValues::Window(window) => window.is_empty(),
        }
    }

    /// Which shape this row is drawn as.
    pub fn kind(&self) -> RowKind {
        match self {
            RowValues::Empty => RowKind::Empty,
            RowValues::Window(window) => match window.first_value().and_then(WaveValue::width) {
                Some(1) => RowKind::Bool,
                Some(_) => RowKind::Bits,
                None => RowKind::Empty,
            },
        }
    }

    /// The value in effect at `time`, inside the fetched window.
    pub fn value_at(&self, time: Time) -> Option<&WaveValue> {
        match self {
            RowValues::Empty => None,
            RowValues::Window(window) => window.value_at(time),
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
        visit: impl FnMut(f64, &'a WaveValue),
    ) {
        let RowValues::Window(window) = self else {
            return;
        };
        window.for_each_change(start, end, view, width, visit);
    }

    /// The changes of this row inside `start ..= end`, as a borrowed slice of the
    /// cached window.
    pub fn window(&self, start: Time, end: Time) -> &[Change] {
        match self {
            RowValues::Empty => &[],
            RowValues::Window(window) => window.window(start, end),
        }
    }

    /// The cached window itself, when the row has one.
    pub fn cached(&self) -> Option<&RowWindow> {
        match self {
            RowValues::Empty => None,
            RowValues::Window(window) => Some(window),
        }
    }

    /// Bit count to format values with: the declared (or dump) width, clamped
    /// to what the values really hold.
    ///
    /// A dump can be narrower than the design (a truncated vector, a `$var` that
    /// is narrower than the declared port), and the formatter must not be asked
    /// for more bits than the value has.
    pub fn format_width(&self, declared: usize) -> usize {
        let declared = declared.max(1);
        match self {
            RowValues::Empty => declared,
            RowValues::Window(window) => window
                .first_value()
                .and_then(WaveValue::width)
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
    /// `Some(bit)` for a row that shows one bit of the bus at `path` (the
    /// expanded-bus view).
    pub bit: Option<u8>,
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

/// The leaf signal name of a hierarchical path, for the label column.
///
/// The wave rows carry the full instance path (`top.u_core.pc`); the default
/// label is just the signal name (`pc`), and the full path is one checkbox
/// away (and always in the hover tooltip). Escaped Verilog identifiers may
/// contain dots (`\foo.bar `), so a dot inside an escaped region is not a
/// hierarchy separator; the trailing space of an escaped identifier is
/// trimmed because it is VCD syntax, not part of the name.
pub fn leaf_name(name: &str) -> &str {
    let mut in_escaped = false;
    let mut start = 0usize;
    for (index, character) in name.char_indices() {
        match character {
            '\\' => in_escaped = true,
            // whitespace terminates an escaped identifier
            character if character.is_whitespace() => in_escaped = false,
            '.' if !in_escaped => start = index + 1,
            _ => {}
        }
    }
    name[start..].trim_end()
}

/// The history of one bit of a decoded bus.
///
/// Costs one pass over the parent's changes and one small value per time the bit
/// actually changes (a bit that never toggles costs nothing but its first
/// value). The result is a normal [`DecodedSignal`], so the renderer, the value
/// column and the edge snapping treat an expanded bit exactly like a signal that
/// is one bit wide in the dump.
pub fn derive_bit(parent: &DecodedSignal, bit: usize) -> DecodedSignal {
    let mut out: Vec<Change> = Vec::new();
    let mut last: Option<BitValue> = None;
    for (time, value) in parent.changes() {
        let next = value.as_bits().and_then(|bits| bits.get(bit));
        if next == last {
            continue;
        }
        last = next;
        out.push((
            *time,
            WaveValue::Bits(Bits::new_msb_first(vec![next.unwrap_or(BitValue::X)])),
        ));
    }
    DecodedSignal::from_changes(out)
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
            let existing = previous
                .iter()
                .position(|item| item.path == row.path && item.bit == row.bit);
            let mut item = match existing {
                Some(index) => previous.remove(index),
                None => SignalItem {
                    name: String::new(),
                    path: row.path.clone(),
                    bit: row.bit,
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
            item.bit = row.bit;
            item.declared = row.width.max(1);
            item.index = resolved.index;
            if let Some(bit) = row.bit {
                // one bit of the bus: the parent's dump signal, drawn as a bit
                item.width = 1;
                item.is_bool = true;
                item.name = format!("{}[{}]", row.name, bit);
            } else {
                item.width = resolved.width;
                item.is_bool = resolved.is_bool;
                item.name = if resolved.is_bool {
                    row.name.clone()
                } else {
                    format!("{}[{}:0]", row.name, item.declared - 1)
                };
            }
            item.setting = row.setting.clone();
            item.load = row.load.clone();
            item.driver = row.drive.clone();
            item.choose = row.chosen;
            next.push(item);
        }
        // Windowed rows own their cache: drawing never fills the backend's
        // decode cache, so nothing has to be unloaded from the dump here.
        self.item = next;
        Vec::new()
    }

    /// Re-resolve every row against a new dump and drop the old values.
    ///
    /// Nothing is decoded here: the next redraw asks the backend for the rows
    /// that are actually visible, in one batched call.
    pub fn refresh(&mut self, waveform: Option<&Waveform>) {
        for item in &mut self.item {
            let resolved = resolve(&item.path, item.declared, waveform);
            item.index = resolved.index;
            if item.bit.is_some() {
                // a bit row stays one bit wide whatever the bus says
                item.width = 1;
                item.is_bool = true;
            } else {
                item.width = resolved.width;
                item.is_bool = resolved.is_bool;
            }
            item.values = RowValues::Empty;
        }
    }

    /// Append the time stamps of the value changes of `rows` inside
    /// `start ..= end` to `out`.
    ///
    /// Cursor snapping needs the edges that are on screen; collecting them per
    /// gesture (not per mouse move) is cheap: only the rows in the given range
    /// are looked at and only their window is walked. The caller sorts and
    /// de-duplicates the result.
    pub fn edges(&self, rows: std::ops::Range<usize>, start: Time, end: Time, out: &mut Vec<Time>) {
        for row in rows {
            let Some(item) = self.item.get(row) else {
                break;
            };
            let Some(window) = item.values.cached() else {
                continue;
            };
            out.extend(window.window(start, end).iter().map(|(time, _)| *time));
        }
    }

    /// Store the bounded window of one row.
    pub fn set_window(&mut self, row: usize, window: RowWindow) {
        if let Some(item) = self.item.get_mut(row) {
            item.values = RowValues::Window(window);
        }
    }

    /// The cached window of one row, when it has one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn window_of(&self, row: usize) -> Option<&RowWindow> {
        self.item.get(row).and_then(|item| item.values.cached())
    }

    /// Bytes all cached windows in `rows` hold together (the memory budget).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn window_bytes(&self, rows: std::ops::Range<usize>) -> usize {
        let end = rows.end.min(self.item.len());
        let start = rows.start.min(end);
        self.item[start..end]
            .iter()
            .filter_map(|item| item.values.cached())
            .map(RowWindow::bytes)
            .sum()
    }

    /// Drop the cached windows of every row outside `keep`.
    ///
    /// A row that scrolls back in fetches one bounded window again, so this is
    /// the cheapest way to give memory back for a long row list.
    pub fn drop_outside(&mut self, keep: std::ops::Range<usize>) {
        for (row, item) in self.item.iter_mut().enumerate() {
            if keep.contains(&row) || matches!(item.values, RowValues::Empty) {
                continue;
            }
            item.values = RowValues::Empty;
        }
    }

    /// Keep the window cache inside `budget`, dropping the rows farthest from
    /// the viewport first (least recently looked at), then the edges of the
    /// visible range. Returns the bytes still cached.
    pub fn trim_windows(&mut self, keep: std::ops::Range<usize>, budget: usize) -> usize {
        let budget = budget.max(1);
        let mut total: usize = self
            .item
            .iter()
            .filter_map(|item| item.values.cached())
            .map(RowWindow::bytes)
            .sum();
        if total <= budget {
            return total;
        }
        let rows = self.item.len();
        let mut outside: Vec<usize> = (0..rows).filter(|row| !keep.contains(row)).collect();
        outside.sort_by_key(|row| {
            if *row < keep.start {
                keep.start - *row
            } else {
                row.saturating_sub(keep.end)
            }
        });
        for row in outside {
            if total <= budget {
                break;
            }
            if let Some(item) = self.item.get_mut(row) {
                if let RowValues::Window(window) = &item.values {
                    total = total.saturating_sub(window.bytes());
                    item.values = RowValues::Empty;
                }
            }
        }
        // still over budget (a viewport full of very dense rows): drop from the
        // edges of the visible range, which fetch again when scrolled to
        let mut low = keep.start.min(rows);
        let mut high = keep.end.min(rows);
        while total > budget && low < high {
            let row = low;
            low += 1;
            if let Some(item) = self.item.get_mut(row) {
                if let RowValues::Window(window) = &item.values {
                    total = total.saturating_sub(window.bytes());
                    item.values = RowValues::Empty;
                }
            }
            if total <= budget || low >= high {
                break;
            }
            high -= 1;
            if let Some(item) = self.item.get_mut(high) {
                if let RowValues::Window(window) = &item.values {
                    total = total.saturating_sub(window.bytes());
                    item.values = RowValues::Empty;
                }
            }
        }
        total
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
    use crate::wave_show::windowed::{RowWindow, WindowRequest};
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
            bit: None,
        }
    }

    fn synced(paths: &[(&str, usize)]) -> SignalThings {
        let rows: Vec<Row> = paths
            .iter()
            .map(|(path, width)| store_row(path, *width))
            .collect();
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
    fn the_leaf_name_drops_the_instance_path_but_keeps_the_rest() {
        // the wave label column shows the signal name by default
        assert_eq!(leaf_name("top.clk"), "clk");
        assert_eq!(leaf_name("top.u_core.pc[7:0]"), "pc[7:0]");
        assert_eq!(leaf_name("top.u_core.pc"), "pc");
        // a name without a hierarchy is unchanged
        assert_eq!(leaf_name("clk"), "clk");
        assert_eq!(leaf_name("pc[7:0]"), "pc[7:0]");
        // an array element keeps its index: it is part of the signal name
        assert_eq!(leaf_name("top.u_core.regs[3]"), "regs[3]");
        // an escaped Verilog identifier may contain dots; they are not hierarchy
        // separators, and the VCD terminator space is trimmed from the display
        assert_eq!(leaf_name("top.\\foo.bar "), "\\foo.bar");
        assert_eq!(leaf_name("\\root.dot "), "\\root.dot");
        // an empty name stays empty instead of panicking
        assert_eq!(leaf_name(""), "");
    }

    #[test]
    fn sync_keeps_the_values_of_rows_that_survive_a_reorder() {
        let mut things = synced(&[("a", 1), ("b", 1)]);
        things.set_window(0, window(&[0, 10, 20], 0, 100));
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
        things.set_window(1, window(&[0, 10], 0, 100));
        let unload = things.sync(&[store_row("a", 1)], None);
        // drawing caches windows, not backend histories: nothing to unload
        assert!(unload.is_empty());
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
    fn drop_outside_frees_rows_that_left_the_viewport() {
        let mut things = synced(&[("a", 1), ("b", 1), ("c", 1)]);
        things[0].index = Some(1);
        things[1].index = Some(2);
        things[2].index = Some(3);
        for row in 0..3 {
            things.set_window(row, window(&[0, 10, 20], 0, 100));
        }

        //keep row 1 only: the others give their window back
        things.drop_outside(1..2);
        assert!(
            !things[1].values.is_empty(),
            "row inside the window survives"
        );
        assert!(
            things[0].values.is_empty(),
            "row before the window was dropped"
        );
        assert!(
            things[2].values.is_empty(),
            "row after the window was dropped"
        );
    }

    #[test]
    fn refresh_drops_the_values_and_re_resolves() {
        let mut things = synced(&[("a", 1)]);
        things[0].index = Some(3);
        things.set_window(0, window(&[0, 10], 0, 100));
        things.refresh(None);
        assert!(things[0].values.is_empty());
        assert_eq!(things[0].index, None);
        assert!(things[0].is_bool);
    }

    #[test]
    fn edges_lists_the_changes_of_the_cached_rows_in_the_window() {
        // three rows, only the first has a window (values at 0, 10, 20)
        let mut things = synced(&[("a", 1), ("b", 8), ("c", 1)]);
        things[0].index = Some(1);
        things[1].index = Some(2);
        things.set_window(0, window(&[0, 10, 20], 0, 100));

        let mut edges = Vec::new();
        things.edges(0..3, 5, 25, &mut edges);
        assert_eq!(edges, vec![10, 20]);
        // the window bounds are inclusive and the undecoded rows are skipped
        edges.clear();
        things.edges(0..3, 0, 10, &mut edges);
        assert_eq!(edges, vec![0, 10]);
        edges.clear();
        things.edges(1..2, 0, 25, &mut edges);
        assert!(edges.is_empty());
        // out of range rows never panic
        things.edges(2..9, 0, 25, &mut edges);
        assert!(edges.is_empty());
    }

    #[test]
    fn format_width_clamps_to_what_the_values_hold() {
        //the dump is narrower than the design: format with the dump's width
        let values = RowValues::Window(width_window(5, &[0]));
        assert_eq!(values.format_width(16), 5);
        //a wider value than declared keeps the declared width
        assert_eq!(values.format_width(3), 3);
        //no values at all: the declared width, never zero
        assert_eq!(RowValues::Empty.format_width(0), 1);
    }

    #[test]
    fn row_kind_follows_the_dump_width() {
        assert_eq!(RowValues::Empty.kind(), RowKind::Empty);
        assert_eq!(
            RowValues::Window(width_window(1, &[0])).kind(),
            RowKind::Bool
        );
        assert_eq!(
            RowValues::Window(width_window(8, &[0])).kind(),
            RowKind::Bits
        );
        //a window with no changes and no carried value has nothing to draw
        assert_eq!(
            RowValues::Window(width_window(1, &[])).kind(),
            RowKind::Empty
        );
    }

    /// A window of `width`-bit unknown values at the given time stamps.
    fn width_window(width: usize, times: &[u64]) -> RowWindow {
        let value = WaveValue::Bits(Bits::all_unknown(width));
        let changes: Arc<[Change]> = times.iter().map(|time| (*time, value.clone())).collect();
        RowWindow::new(
            changes,
            None,
            WindowRequest {
                start: 0,
                end: 100,
                visible_start: 0,
                visible_end: 100,
                cap: 64,
            },
            false,
        )
    }

    /// A bounded window holding one change per time stamp.
    fn window(times: &[u64], start: u64, end: u64) -> RowWindow {
        let value = WaveValue::Bits(Bits::all_unknown(1));
        let changes: Arc<[Change]> = times.iter().map(|time| (*time, value.clone())).collect();
        RowWindow::new(
            changes,
            None,
            WindowRequest {
                start,
                end,
                visible_start: start,
                visible_end: end,
                cap: 64,
            },
            false,
        )
    }

    #[test]
    fn a_row_caches_one_bounded_window() {
        let mut things = synced(&[("a", 1)]);
        assert!(things.window_of(0).is_none());
        things.set_window(0, window(&[0, 10, 20], 0, 100));
        let cached = things.window_of(0).expect("the window is cached");
        assert_eq!(cached.req.start, 0);
        assert_eq!(cached.req.end, 100);
        // the value at the left edge is carried from the window
        assert!(things[0].values.value_at(15).is_some());
        // and the window reports its bytes for the budget
        assert_eq!(
            things.window_bytes(0..1),
            3 * crate::wave_show::windowed::BYTES_PER_CHANGE
        );
        things.drop_outside(0..1);
        assert!(things.window_of(0).is_some());
    }

    #[test]
    fn trim_windows_evicts_the_rows_farthest_from_the_viewport() {
        let mut things = synced(&[("a", 1), ("b", 1), ("c", 1), ("d", 1)]);
        for row in 0..4 {
            things.set_window(row, window(&[0, 10, 20, 30], 0, 100));
        }
        let bytes = crate::wave_show::windowed::BYTES_PER_CHANGE;
        let all = things.window_bytes(0..4);
        assert_eq!(all, 16 * bytes);
        // room for two rows: the two outside the viewport go first
        let left = things.trim_windows(1..3, 8 * bytes);
        assert!(left <= 8 * bytes);
        assert!(
            things.window_of(0).is_none(),
            "row before the viewport dropped"
        );
        assert!(
            things.window_of(3).is_none(),
            "row after the viewport dropped"
        );
        assert!(things.window_of(1).is_some());
        assert!(things.window_of(2).is_some());
        // a budget that cannot hold even the visible rows keeps trimming them
        let left = things.trim_windows(1..3, bytes);
        assert!(left <= bytes);
        assert_eq!(
            things.window_bytes(0..4),
            left,
            "the cache reports what it still holds"
        );
    }

    /// A 4-bit value with the given low bits (msb first).
    fn nibble(low: [BitValue; 2]) -> WaveValue {
        WaveValue::Bits(Bits::new_msb_first(vec![
            BitValue::Zero,
            BitValue::Zero,
            low[1],
            low[0],
        ]))
    }

    #[test]
    fn derive_bit_keeps_only_the_changes_of_that_bit() {
        // 0 -> 1 -> 2 -> 3: bit 0 toggles at every change, bit 1 only twice
        let parent = DecodedSignal::from_changes(vec![
            (0, nibble([BitValue::Zero, BitValue::Zero])),
            (5, nibble([BitValue::One, BitValue::Zero])),
            (9, nibble([BitValue::Zero, BitValue::One])),
            (12, nibble([BitValue::One, BitValue::One])),
        ]);
        let times = |signal: &DecodedSignal| {
            signal
                .changes()
                .iter()
                .map(|(time, _)| *time)
                .collect::<Vec<_>>()
        };
        assert_eq!(times(&derive_bit(&parent, 0)), vec![0, 5, 9, 12]);
        // bit 1 is 0,0,1,1: only the first value and the jump at 9
        assert_eq!(times(&derive_bit(&parent, 1)), vec![0, 9]);
        // a bit that never changes still has its initial value
        let constant = derive_bit(&parent, 3);
        assert_eq!(times(&constant), vec![0]);
        // and a derived bit is one bit wide, so it is drawn as a digital row
        assert_eq!(
            constant.first_change().map(|(_, value)| value.width()),
            Some(Some(1))
        );
        // a bit the bus does not have (an empty history) does not panic
        assert!(derive_bit(&parent, 40).is_empty());
    }
}
