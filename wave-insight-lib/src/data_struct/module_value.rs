use std::collections::HashMap;
use num::BigUint;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Clone, Deserialize, Serialize)]
pub struct ModuleValue {
    pub value: HashMap<String,Vec<(i32,BigUint)>>,
    clk: i32,
}

impl ModuleValue {
    pub fn new() -> Self {
        Self {
            value: HashMap::new(),
            clk: 0,
        }
    }
    pub fn get(&self, key: &str) -> Vec<(i32, BigUint)> {
        self.value.get(key).unwrap_or(&Vec::new()).to_vec()
    }
    pub fn new_clk(&mut self, clk: i32) {
        self.clk=clk;
    }
    pub fn insert(&mut self, key: String, data: BigUint) {
        self.value.entry(key).or_insert_with(Vec::new)
            .push((self.clk, data))
    }
}

impl Default for ModuleValue {
    fn default() -> Self {
        Self::new()
    }
}
