use std::{rc::Rc, slice::{SliceIndex, Iter}, ops::{Index, IndexMut}};
use wave_insight_lib::data_struct::Signal;
use crate::wave_show::Settings;

pub struct SignalItem {
    pub name: String,
    pub signal: Rc<Signal>,
    pub is_bool: bool,
    pub setting: Settings,
    pub load: Vec<String>,
    pub driver: Vec<String>,
}

pub struct SignalThings {

    item: Vec<SignalItem>,
}

impl SignalThings {
    pub fn new() -> Self {
        Self { item: Vec::new() }
    }
    pub fn push(&mut self, name: &str, signal: &Rc<Signal>) {
        if !name.is_empty() {//TODO:!self.signal.contains(signal)
            let bool_signal = signal.size==1;
            let name_size = 
                if bool_signal {name.to_owned()}
                else {name.to_owned()+"["+&(signal.size-1).to_string()+":0]"};
            self.item.push(SignalItem {
                name: name_size,
                signal: Rc::clone(signal),
                is_bool: bool_signal,
                setting: Settings::new(),
                load: signal.load.clone(),
                driver: signal.drive.clone() })
        }
    }
    pub fn remove(&mut self, idx: usize) {
        self.item.remove(idx);
    }
    pub fn exchange(&mut self, from: usize, to: usize) {
        let item = self.item.remove(from);
        self.item.insert(to, item);
    }
    pub fn iter(&self) -> Iter<SignalItem> {
        self.item.iter()
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
