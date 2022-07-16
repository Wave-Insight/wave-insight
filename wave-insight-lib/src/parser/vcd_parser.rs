use crate::data_struct::{Module, ModuleValue};

use super::vcd::parse_state::ParseState;
use super::vcd::parsing_line::parsing_line;

pub fn vcd_parser(input: String, raw_module: &mut Module) -> (Module, ModuleValue) {
    let lines = input.lines();
    let mut state = ParseState{clk: 0, module: raw_module.clone(), value: ModuleValue::new(), stack: vec![]};
    lines.for_each(|l| parsing_line(&mut state, l.to_string()));
    state.module.end_clock = state.clk;
    (state.module, state.value)
}
