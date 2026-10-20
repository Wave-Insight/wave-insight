//! The rows of the wave view.
//!
//! A row keeps a *handle* to its values, never a copy: the browser path stores
//! the backend's [`DecodedSignal`] and the canvas slices it per frame
//! ([`DecodedSignal::window`]), so a redraw neither touches the dump nor clones
//! a value. Decoding is done by the component in one batched call
//! (`Waveform::decoded_changes_many`) for the rows that are on screen, once.
//!
//! The legacy transports (tauri/server) still push an expanded `ModuleValue`;
//! those values are kept as they arrive ([`LegacySeries`]) and drawn with the
//! legacy formatters, exactly like the old renderer did.

use std::ops::{Index, IndexMut};
use std::rc::Rc;
use std::slice::{Iter, IterMut, SliceIndex};

use wave_insight_lib::data_struct::{BitsData, BoolData, ModuleValue, Signal};
use wave_insight_lib::waveform::{DecodedSignal, SignalKind, WaveValue, Waveform};

use crate::wave_show::geometry::{self, Time};
use crate::wave_show::Settings;

/// One value of a row, borrowed from whichever storage the row uses.
#[derive(Clone, Copy, Debug)]
pub enum RowValue<'a> {
    /// Browser path value.
    Backend(&'a WaveValue),
    /// Legacy one bit value.
    Bool(&'a BoolData),
    /// Legacy bit vector value.
    Bits(&'a BitsData),
}

/// What kind of waveform a row draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// No values (yet): a flat "no data" line.
    Empty,
    /// One bit: drawn as a digital step with an x/z middle level.
    Bool,
    /// A bus: drawn as a band with a value label.
    Bits,
}

/// Values of one row.
#[derive(Clone, Default)]
pub enum RowValues {
    /// No dump loaded, the signal is not in the dump, or the row was dropped
    /// from the decode cache to stay inside the memory budget.
    #[default]
    Empty,
    /// The browser path: a cached, decoded history owned by the backend. Cheap
    /// to clone (one `Arc` bump) and cheap to slice.
    Backend(DecodedSignal),
    /// Expanded values from a legacy transport.
    Legacy(LegacySeries),
}

/// Values that arrive pre-expanded from the tauri/server transports.
#[derive(Clone, Default)]
pub struct LegacySeries {
    /// One bit values, if this row is a single bit signal.
    pub bools: Option<Rc<Vec<(i32, BoolData)>>>,
    /// Bit vector values.
    pub bits: Option<Rc<Vec<(i32, BitsData)>>>,
    /// Width the values really have, when known (the declared width can be
    /// wider than the dump, and the legacy formatters panic on the difference).
    pub bits_width: Option<usize>,
}

impl LegacySeries {
    /// Take whatever `ModuleValue` has for `key`.
    ///
    /// The legacy container stores one bit signals and buses in different maps,
    /// and `get_bool` also answers for a bus (with the flattened chunks), so the
    /// row has to say which shape it expects.
    pub fn from_module_value(values: &ModuleValue, key: &str, is_bool: bool) -> Self {
        if is_bool {
            let bools = values.get_bool(key);
            return if bools.is_empty() {
                Self::default()
            } else {
                Self {
                    bools: Some(Rc::new(bools)),
                    ..Self::default()
                }
            };
        }
        let bits = values.get_bits(key);
        if bits.is_empty() {
            Self::default()
        } else {
            Self {
                bits: Some(Rc::new(bits)),
                ..Self::default()
            }
        }
    }
}

impl RowValues {
    /// `true` if there is nothing to draw.
    pub fn is_empty(&self) -> bool {
        match self {
            RowValues::Empty => true,
            RowValues::Backend(decoded) => decoded.is_empty(),
            RowValues::Legacy(legacy) => legacy.bools.is_none() && legacy.bits.is_none(),
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
            RowValues::Legacy(legacy) => {
                if legacy.bools.is_some() {
                    RowKind::Bool
                } else if legacy.bits.is_some() {
                    RowKind::Bits
                } else {
                    RowKind::Empty
                }
            }
        }
    }

    /// The value in effect at `time`.
    pub fn value_at(&self, time: Time) -> Option<RowValue<'_>> {
        match self {
            RowValues::Empty => None,
            RowValues::Backend(decoded) => decoded.value_at(time).map(RowValue::Backend),
            RowValues::Legacy(LegacySeries { bools: Some(bools), .. }) => {
                geometry::value_at_stamped(bools.as_slice(), time)
                    .map(|(_, value)| RowValue::Bool(value))
            }
            RowValues::Legacy(LegacySeries { bits: Some(bits), .. }) => {
                geometry::value_at_stamped(bits.as_slice(), time)
                    .map(|(_, value)| RowValue::Bits(value))
            }
            RowValues::Legacy(_) => None,
        }
    }

    /// Bit count to format values with: the declared (or dump) width, clamped
    /// to what the values really hold.
    ///
    /// A dump can be narrower than the design (a truncated vector, a `$var`
    /// that is narrower than the declared port) and the legacy formatters panic
    /// when asked for more bits than the value has, so the renderer must clamp.
    pub fn format_width(&self, declared: usize) -> usize {
        let declared = declared.max(1);
        match self {
            RowValues::Empty => declared,
            RowValues::Backend(decoded) => decoded
                .first_change()
                .and_then(|(_, value)| value.width())
                .map(|width| declared.min(width.max(1)))
                .unwrap_or(declared),
            RowValues::Legacy(legacy) => legacy
                .bits_width
                .map(|width| declared.min(width.max(1)))
                .unwrap_or(declared),
        }
    }

    /// Visit the changes of this row inside `start ..= end`, thinned down to one
    /// per pixel of a `width` px surface.
    ///
    /// Backend rows slice the cached history; legacy rows binary search their
    /// expanded vectors. Neither allocates, and the values stay borrowed from
    /// this row for the whole call.
    pub fn for_each_change<'a>(
        &'a self,
        start: Time,
        end: Time,
        view: &geometry::View,
        width: f64,
        mut visit: impl FnMut(f64, RowValue<'a>),
    ) {
        match self {
            RowValues::Empty => {}
            RowValues::Backend(decoded) => {
                let window = decoded.window(start, Some(end));
                geometry::for_each_pixel_change(window, view, width, |x, (_, value)| {
                    visit(x, RowValue::Backend(value));
                });
            }
            RowValues::Legacy(LegacySeries { bools: Some(bools), .. }) => {
                let window = geometry::slice_window_stamped(bools.as_slice(), start, end);
                geometry::for_each_pixel_stamped(window, view, width, |x, (_, value)| {
                    visit(x, RowValue::Bool(value));
                });
            }
            RowValues::Legacy(LegacySeries { bits: Some(bits), .. }) => {
                let window = geometry::slice_window_stamped(bits.as_slice(), start, end);
                geometry::for_each_pixel_stamped(window, view, width, |x, (_, value)| {
                    visit(x, RowValue::Bits(value));
                });
            }
            RowValues::Legacy(_) => {}
        }
    }
}

/// One row of the wave view.
pub struct SignalItem {
    /// Display name, including the `[n:0]` suffix for buses.
    pub name: String,
    /// The signal as the design/legacy model describes it (`value_key` is the
    /// hierarchical path).
    pub signal: Rc<Signal>,
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
fn resolve(signal: &Signal, waveform: Option<&Waveform>) -> Resolved {
    let declared = signal.size.max(1);
    let Some(info) = waveform.and_then(|waveform| waveform.signal_by_path(&signal.value_key)) else {
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

pub struct SignalThings {
    item: Vec<SignalItem>,
}

impl SignalThings {
    pub fn new() -> Self {
        Self { item: Vec::new() }
    }

    /// Append (or keep) one row.
    ///
    /// The same signal can be added from the sidebar, from the source editor
    /// and from a driver list, so duplicates are dropped by hierarchical path.
    pub fn push(&mut self, name: &str, signal: &Rc<Signal>, waveform: Option<&Waveform>) {
        if name.is_empty() || signal.value_key.is_empty() {
            return;
        }
        if self
            .item
            .iter()
            .any(|item| item.signal.value_key == signal.value_key)
        {
            return;
        }

        let declared = signal.size.max(1);
        let resolved = resolve(signal, waveform);
        let name_size = if resolved.is_bool {
            name.to_owned()
        } else {
            format!("{name}[{}:0]", declared - 1)
        };
        let insert_idx = self
            .item
            .iter()
            .enumerate()
            .filter(|(_, item)| item.choose)
            .map(|(idx, _)| idx)
            .last()
            .map(|x| x + 1)
            .unwrap_or(self.item.len());
        self.item.insert(
            insert_idx,
            SignalItem {
                name: name_size,
                signal: Rc::clone(signal),
                index: resolved.index,
                width: resolved.width,
                is_bool: resolved.is_bool,
                setting: Settings::new(),
                load: signal.load.clone(),
                driver: signal.drive.clone(),
                choose: false,
                values: RowValues::Empty,
            },
        );
    }

    /// Re-resolve every row against a new dump and drop the old values.
    ///
    /// Nothing is decoded here: the next redraw asks the backend for the rows
    /// that are actually visible, in one batched call.
    pub fn refresh(&mut self, waveform: Option<&Waveform>) {
        for item in &mut self.item {
            let resolved = resolve(&item.signal, waveform);
            item.index = resolved.index;
            item.width = resolved.width;
            item.is_bool = resolved.is_bool;
            item.values = RowValues::Empty;
        }
    }

    /// Install the values of a legacy transport for the rows that are loaded.
    pub fn refresh_legacy(&mut self, values: Option<&ModuleValue>) {
        for item in &mut self.item {
            item.values = match values {
                Some(values) => {
                    let legacy =
                        LegacySeries::from_module_value(values, &item.signal.value_key, item.is_bool);
                    if legacy.bools.is_none() && legacy.bits.is_none() {
                        RowValues::Empty
                    } else {
                        RowValues::Legacy(legacy)
                    }
                }
                None => RowValues::Empty,
            };
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

    /// Drop every decoded value (a new dump was opened, or the file was closed).
    #[allow(dead_code)]
    pub fn clear_values(&mut self) {
        for item in &mut self.item {
            item.values = RowValues::Empty;
        }
    }

    pub fn remove(&mut self, idx: &[usize]) {
        idx.iter().enumerate().for_each(|(r, &x)| {
            if x - r < self.item.len() {
                self.item.remove(x - r);
            }
        });
    }

    pub fn exchange(&mut self, from: usize, to: usize) {
        if from < self.item.len() && to < self.item.len() {
            let item = self.item.remove(from);
            self.item.insert(to, item);
        }
    }

    pub fn iter(&self) -> Iter<'_, SignalItem> {
        self.item.iter()
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, SignalItem> {
        self.item.iter_mut()
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

    pub fn onchoose(&mut self, idx: usize, ctrl: bool, shift: bool) {
        if idx >= self.item.len() {
            return;
        }
        if ctrl {
            self.item[idx].choose = !self.item[idx].choose
        } else if shift {
            let choosed_idx = self.get_choose_idx();
            if !choosed_idx.is_empty() {
                let head_idx = choosed_idx[0];
                let tail_idx = *choosed_idx.last().unwrap();
                if choosed_idx.contains(&idx) {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                } else if tail_idx < idx {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                    self.item
                        .get_mut(head_idx..(idx + 1))
                        .unwrap()
                        .iter_mut()
                        .for_each(|i| i.choose = true);
                } else if idx < head_idx {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                    self.item
                        .get_mut(idx..(tail_idx + 1))
                        .unwrap()
                        .iter_mut()
                        .for_each(|i| i.choose = true);
                } else if ((head_idx as isize) - (idx as isize)).abs()
                    <= ((idx as isize) - (tail_idx as isize)).abs()
                {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                    self.item
                        .get_mut(head_idx..(idx + 1))
                        .unwrap()
                        .iter_mut()
                        .for_each(|i| i.choose = true);
                } else {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                    self.item
                        .get_mut(idx..(tail_idx + 1))
                        .unwrap()
                        .iter_mut()
                        .for_each(|i| i.choose = true);
                }
            } else {
                self.item[idx].choose = true;
            }
        } else {
            let choose = self.item[idx].choose;
            self.item.iter_mut().for_each(|i| i.choose = false);
            self.item[idx].choose = !choose;
        }
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
    use super::*;
    use wave_insight_lib::data_struct::CodeLocation;

    fn signal(path: &str, size: usize) -> Rc<Signal> {
        Rc::new(Signal {
            size,
            value_key: path.to_string(),
            drive: Vec::new(),
            load: Vec::new(),
            location_define: CodeLocation::new(),
            location_drive: Vec::new(),
            location_load: Vec::new(),
        })
    }

    fn row(path: &str, index: Option<usize>, values: RowValues) -> SignalItem {
        SignalItem {
            name: path.to_string(),
            signal: signal(path, 1),
            index,
            width: 1,
            is_bool: true,
            setting: Settings::new(),
            load: Vec::new(),
            driver: Vec::new(),
            choose: false,
            values,
        }
    }

    fn legacy_values() -> RowValues {
        RowValues::Legacy(LegacySeries {
            bools: Some(Rc::new(vec![
                (0, BoolData::Zero),
                (10, BoolData::One),
            ])),
            bits: None,
            bits_width: Some(1),
        })
    }

    #[test]
    fn pending_lists_only_rows_that_need_a_decode() {
        let mut things = SignalThings::new();
        things.item.push(row("a", Some(7), RowValues::Empty));
        things.item.push(row("b", Some(8), legacy_values()));
        things.item.push(row("c", None, RowValues::Empty));
        things.item.push(row("d", Some(9), RowValues::Empty));

        //only the empty rows with a signal index are worth asking the backend
        assert_eq!(things.pending(0..4), vec![(0, 7), (3, 9)]);
        //and only inside the requested range
        assert_eq!(things.pending(1..3), Vec::new());
        assert_eq!(things.pending(3..9), vec![(3, 9)]);
    }

    #[test]
    fn drop_outside_frees_rows_that_left_the_viewport() {
        let mut things = SignalThings::new();
        things.item.push(row("a", Some(1), legacy_values()));
        things.item.push(row("b", Some(2), legacy_values()));
        things.item.push(row("c", Some(3), legacy_values()));

        //keep row 1 only: the others go back to "no data"
        let unload = things.drop_outside(1..2);
        assert!(!things[1].values.is_empty(), "row inside the window survives");
        assert!(things[0].values.is_empty(), "row before the window was dropped");
        assert!(things[2].values.is_empty(), "row after the window was dropped");
        //legacy rows have no backend cache entry to unload
        assert!(unload.is_empty());
    }

    #[test]
    fn format_width_clamps_to_what_the_values_hold() {
        //the dump is narrower than the design: format with the dump's width or
        //the legacy formatters would subtract and panic
        let values = legacy_values();
        assert_eq!(values.format_width(16), 1);
        //a wider value than declared keeps the declared width
        let wide = RowValues::Legacy(LegacySeries {
            bools: None,
            bits: Some(Rc::new(Vec::new())),
            bits_width: Some(32),
        });
        assert_eq!(wide.format_width(8), 8);
        //no values at all: the declared width, never zero
        assert_eq!(RowValues::Empty.format_width(0), 1);
    }
}
