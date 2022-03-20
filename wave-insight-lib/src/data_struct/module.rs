use crate::data_struct::signal::Signal;
use std::collections::HashMap;

type SignalPath = (Vec<String>,String);
type ModulePath = [String];

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
    pub fn get_module(&mut self, path: &ModulePath) -> Option<&mut Module> {
        path.iter()
            .fold(Some(self),|m,p| {
                m.and_then(|mm| mm.sub_module.get_mut(p))
            })
    }
    pub fn get_signal(&mut self, (module_path,signal_path): &SignalPath) -> Option<&mut Signal> {
        self.get_module(module_path).and_then(|m| m.signal.get_mut(signal_path))
    }
}

impl Default for Module {
    fn default() -> Self {
        Self::new()
    }
}
