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
    pub choose: bool,
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
            let insert_idx = self.item.iter()
                .enumerate()
                .filter(|(_,i)| i.choose)
                .map(|(idx,_)| idx)
                .last()
                .map(|x| x+1)
                .unwrap_or(self.item.len());
            self.item.insert(insert_idx, SignalItem {
                name: name_size,
                signal: Rc::clone(signal),
                is_bool: bool_signal,
                setting: Settings::new(),
                load: signal.load.clone(),
                driver: signal.drive.clone(),
                choose: false,
            })
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
    pub fn iter(&self) -> Iter<SignalItem> {
        self.item.iter()
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
