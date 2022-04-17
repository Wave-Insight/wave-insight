use crate::data_struct::{Module};

use super::vcd::parse_state::ParseState;
use super::vcd::parsing_line::parsing_line;

pub fn vcd_parser(input: &str, raw_module: &mut Module) -> Module {
    let lines = input.lines();
    let mut state = ParseState{clk: 0, module: raw_module.clone(), stack: vec![]};
    lines.for_each(|l| parsing_line(&mut state, l.to_string()));
    state.module
}
