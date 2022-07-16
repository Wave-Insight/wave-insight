use crate::data_struct::Module;

use super::parse_action::ParseAction;
use crate::data_struct::ModuleValue;

pub struct ParseState {
    pub clk: i32,
    pub module: Module,
    pub value: ModuleValue,//HashMap<String,Vec<(i32,BigUint)>>,
    pub stack: Vec<(String,Module)>,
}

impl ParseState {
    pub fn update(&mut self, action: Option<ParseAction>) {
        if let Some(act) = action {
            match act {
                ParseAction::Clk(clk) => {
                    self.value.new_clk(clk);
                    self.clk = clk;
                },
                ParseAction::Module(name, module) => {
                    self.stack.push((name, module));
                },
                ParseAction::EndModule => {
                    let stack_out = self.stack.pop().unwrap();
                    if let Some(m) = self.stack.last_mut() {
                        m.1.sub_module.entry(stack_out.0).or_insert(stack_out.1);
                    }else {
                        self.module.sub_module.entry(stack_out.0).or_insert(stack_out.1);//TODO:if already exist, new module will be throw!
                    }
                },
                ParseAction::Signal(name, signal) => {
                    if let Some(m) = self.stack.last_mut() {
                        m.1.signal.entry(name).or_insert(signal);
                    }else {
                        self.module.signal.entry(name).or_insert(signal);
                    }
                },
                ParseAction::Value(key, value) => {
                    self.value.insert(key,value);
                },
            }
        }
    }
}
