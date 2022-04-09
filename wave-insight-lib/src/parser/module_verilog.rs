use std::collections::HashMap;

#[derive(Debug, PartialEq, Clone)]
pub struct ModuleVerilog {
    pub name: String,
    pub sub_module: HashMap<String,String>,
    pub signal: Vec<String>,
    pub assignment: Vec<(Vec<String>,Vec<String>)>,
}

impl ModuleVerilog {
    pub fn new() -> Self {
        Self {
            name: "".to_string(),
            sub_module: HashMap::new(),
            signal: Vec::new(),
            assignment: Vec::new(),
        }
    }
}

impl Default for ModuleVerilog {
    fn default() -> Self {
        Self::new()
    }
}
