use std::collections::HashMap;
use num::BigUint;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ModuleValue {
    pub value: HashMap<String,(Vec<i32>,Vec<u8>)>,
    clk: i32,
}

impl ModuleValue {
    pub fn new() -> Self {
        Self {
            value: HashMap::new(),
            clk: 0,
        }
    }
    pub fn get(&self, key: &str) -> Vec<(i32,BigUint)> {
        let data = self.value.get(key).unwrap_or(&(Vec::new(),Vec::new())).to_owned();
        let chunk_size = data.1.len()/data.0.len();
        let to_big = data.1.chunks(chunk_size).map(BigUint::from_bytes_be);
        data.0.into_iter().zip(to_big).map(|(l,r)| (l,r)).collect()
    }
    pub fn new_clk(&mut self, clk: i32) {
        self.clk=clk;
    }
    pub fn insert(&mut self, key: String, data: Vec<u8>) {
        let temp = self.value.entry(key).or_insert_with(|| (Vec::new(), Vec::new()));
        temp.0.push(self.clk);
        temp.1.extend(data);
    }
}

impl Default for ModuleValue {
    fn default() -> Self {
        Self::new()
    }
}
