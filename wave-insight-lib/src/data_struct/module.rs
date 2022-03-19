use crate::data_struct::signal::Signal;
use std::collections::HashMap;

//except for top module, name is key
#[derive(Debug, PartialEq, Clone)]
pub struct Module {
    pub sub_module: HashMap<String,Module>,
    pub signal: HashMap<String,Signal>,
}

impl Module {
    pub fn new() -> Self {
        Self {
            sub_module: HashMap::new(),
            signal: HashMap::new(),
        }
    }
}

impl Default for Module {
    fn default() -> Self {
        Self::new()
    }
}
