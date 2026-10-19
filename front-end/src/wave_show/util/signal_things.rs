use std::{rc::Rc, slice::{SliceIndex, Iter}, ops::{Index, IndexMut}};

use wave_insight_lib::data_struct::{BitsData, BoolData, Signal};
use wave_insight_lib::waveform::Waveform;

use crate::wave_show::Settings;

/// Decoded values of one row, in the shape the renderer already understands.
///
/// The rows are decoded one signal at a time, when the user adds them, instead
/// of expanding the whole dump: opening a file only parses its header, so a
/// large VCD costs nothing until a signal is on screen. `Waveform::all_changes`
/// decodes exactly one variable (the time table is shared and cached).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series {
    /// Values of a one bit signal.
    pub bools: Option<Rc<Vec<(i32, BoolData)>>>,
    /// Values of a bit vector.
    pub bits: Option<Rc<Vec<(i32, BitsData)>>>,
}

impl Series {
    /// Decode the whole history of `path`; `None` when the dump has no such
    /// signal (e.g. sources from another testbench were loaded).
    pub fn load(waveform: &Waveform, path: &str) -> Option<Self> {
        let info = waveform.signal_by_path(path)?;
        let changes = waveform.all_changes(info.index()).ok()?;

        let mut bools = Vec::new();
        let mut bits = Vec::new();
        for (time, value) in changes {
            // the renderer works in `i32` time steps, like the old model
            let time = (time as i64).min(i32::MAX as i64) as i32;
            if let Some(value) = value.to_bool_data() {
                bools.push((time, value));
            } else if let Some(value) = value.to_bits_data() {
                bits.push((time, value));
            }
        }

        let mut series = Series::default();
        if !bools.is_empty() {
            series.bools = Some(Rc::new(bools));
        }
        if !bits.is_empty() {
            series.bits = Some(Rc::new(bits));
        }
        Some(series)
    }
}

/// Handle on a [`Series`] that compares by identity.
///
/// The rows hold up to one entry per value change; comparing them by content on
/// every render (Yew compares props) would be a full scan of the trace, so the
/// props only ever compare the pointer.
#[derive(Clone, Debug, Default)]
pub struct SeriesRef(pub Rc<Series>);

impl SeriesRef {
    pub fn new(series: Series) -> Self {
        Self(Rc::new(series))
    }
}

impl std::ops::Deref for SeriesRef {
    type Target = Series;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq for SeriesRef {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

pub struct SignalItem {
    pub name: String,
    pub signal: Rc<Signal>,
    pub is_bool: bool,
    pub setting: Settings,
    pub load: Vec<String>,
    pub driver: Vec<String>,
    pub choose: bool,
    /// Values of this row, empty while no dump is loaded.
    pub series: SeriesRef,
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
        if name.is_empty() || signal.value_key.is_empty() {//TODO:!self.signal.contains(signal)
            return;
        }
        if self
            .item
            .iter()
            .any(|item| item.signal.value_key == signal.value_key)
        {
            return;
        }

        let size = signal.size.max(1);
        let bool_signal = size == 1;
        let name_size = 
            if bool_signal {name.to_owned()}
            else {name.to_owned()+"["+&(size-1).to_string()+":0]"};
        let insert_idx = self.item.iter()
            .enumerate()
            .filter(|(_,i)| i.choose)
            .map(|(idx,_)| idx)
            .last()
            .map(|x| x+1)
            .unwrap_or(self.item.len());
        let series = waveform
            .and_then(|waveform| Series::load(waveform, &signal.value_key))
            .map(SeriesRef::new)
            .unwrap_or_default();
        self.item.insert(insert_idx, SignalItem {
            name: name_size,
            signal: Rc::clone(signal),
            is_bool: bool_signal,
            setting: Settings::new(),
            load: signal.load.clone(),
            driver: signal.drive.clone(),
            choose: false,
            series,
        })
    }

    /// Decode every row again, e.g. after another dump was opened.
    pub fn refresh(&mut self, waveform: Option<&Waveform>) {
        for item in &mut self.item {
            item.series = waveform
                .and_then(|waveform| Series::load(waveform, &item.signal.value_key))
                .map(SeriesRef::new)
                .unwrap_or_default();
        }
    }

    pub fn remove(&mut self, idx: &[usize]) {
        idx.iter().enumerate().for_each(|(r,&x)| {
            self.item.remove(x-r);
        });
    }
    pub fn exchange(&mut self, from: usize, to: usize) {
        let item = self.item.remove(from);
        self.item.insert(to, item);
    }
    pub fn iter(&self) -> Iter<'_, SignalItem> {
        self.item.iter()
    }
    pub fn len(&self) -> usize {
        self.item.len()
    }
    pub fn is_empty(&self) -> bool {
        self.item.is_empty()
    }
    pub fn get_choose_idx(&self) -> Vec<usize> {
        self.item.iter()
                .enumerate()
                .filter(|(_,i)| i.choose)
                .map(|(idx,_)| idx)
                .collect()
    }
    pub fn onchoose(&mut self, idx: usize, ctrl: bool, shift: bool) {
        if ctrl {
            self.item[idx].choose = !self.item[idx].choose
        }else if shift {
            let choosed_idx = self.get_choose_idx();
            if !choosed_idx.is_empty() {
                let head_idx = choosed_idx[0];
                let tail_idx = choosed_idx.last().unwrap();
                if choosed_idx.contains(&idx) {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                }else if *tail_idx < idx {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                    self.item.get_mut(head_idx..(idx+1))
                        .unwrap()
                        .iter_mut()
                        .for_each(|i| i.choose = true);
                }else if idx < head_idx {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                    self.item.get_mut(idx..(tail_idx+1))
                        .unwrap()
                        .iter_mut()
                        .for_each(|i| i.choose = true);
                }else if ((head_idx as isize) - (idx as isize)).abs() <= ((idx as isize) - (*tail_idx as isize)).abs() {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                    self.item.get_mut(head_idx..(idx+1))
                        .unwrap()
                        .iter_mut()
                        .for_each(|i| i.choose = true);
                }else {
                    self.item.iter_mut().for_each(|i| i.choose = false);
                    self.item.get_mut(idx..(tail_idx+1))
                        .unwrap()
                        .iter_mut()
                        .for_each(|i| i.choose = true);
                }
            }else {
                self.item[idx].choose = true;
            }
        }else {
            let choose = self.item[idx].choose;
            self.item.iter_mut().for_each(|i| i.choose = false);
            self.item[idx].choose = !choose;
        }
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
