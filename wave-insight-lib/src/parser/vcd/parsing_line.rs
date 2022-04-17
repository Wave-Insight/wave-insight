use super::parse_state::ParseState;
use super::gen_module::{insert_signal, insert_module, pop_module};
use super::value_change::value_change;

pub fn parsing_line(state: &mut ParseState, line: String) {
    let mut line_item = line.split_whitespace();
    let action = match line_item.next() {
        Some("$var") => insert_signal(line_item),
        Some("$scope") => insert_module(line_item),
        Some("$upscope") => pop_module(),
        Some(this_item) => value_change(line_item, this_item),
        None => None,
    };
    state.update(action);
}
