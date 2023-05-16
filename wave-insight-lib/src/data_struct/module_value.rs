use std::collections::HashMap;
use serde::{Deserialize, Serialize};

use super::{BitsData, BoolData};

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ModuleValue {
                                          // xz,number
    pub value: HashMap<String,(Vec<i32>,Vec<(u8,u8)>)>,//when xz == 1, num == 0 -> x and num == 1 -> z
    clk: i32,
}

impl ModuleValue {
    pub fn new() -> Self {
        Self {
            value: HashMap::new(),
            clk: 0,
        }
    }
    pub fn get_bool(&self, key: &str) -> Vec<(i32, BoolData)> {
        let data = self.value.get(key).unwrap_or(&(Vec::new(),Vec::new())).to_owned();
        let sig = data.1.into_iter()
            .map(BoolData::new);
        data.0.into_iter().zip(sig).map(|(l,r)| (l,r)).collect()
    }
    pub fn get_bits(&self, key: &str) -> Vec<(i32, BitsData)> {
        let data = self.value.get(key).unwrap_or(&(Vec::new(),Vec::new())).to_owned();
        if data.0.is_empty() {
            Vec::new()
        }else {
            let chunk_size = data.1.len()/data.0.len();
            let sig = data.1.chunks(chunk_size)
                .map(|x| BitsData::new(x.to_vec()));
            data.0.into_iter().zip(sig).map(|(l,r)| (l,r)).collect()
        }
    }
    pub fn new_clk(&mut self, clk: i32) {
        self.clk=clk;
    }
    pub fn insert(&mut self, key: String, data: Vec<(u8,u8)>) {
        let temp = self.value.entry(key).or_insert_with(|| (Vec::new(), Vec::new()));
        temp.0.push(self.clk);
        temp.1.extend(data);
    }
    pub fn insert_single(&mut self, key: String, data: (u8,u8)) {
        let temp = self.value.entry(key).or_insert_with(|| (Vec::new(), Vec::new()));
        temp.0.push(self.clk);
        temp.1.push(data);
    }
}

impl Default for ModuleValue {
    fn default() -> Self {
        Self::new()
    }
}
