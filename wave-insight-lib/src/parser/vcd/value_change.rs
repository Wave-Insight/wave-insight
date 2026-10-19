use std::str::SplitWhitespace;
use super::{parse_action::ParseAction, parse_state::ParseState};

pub fn value_change(mut line_item: SplitWhitespace<'_>, this_item: &str, state: &mut ParseState) -> Option<ParseAction> {
    if let Some(clk) = this_item.strip_prefix('#') {
        clk.parse::<i32>().ok()
            .map(ParseAction::Clk)
    }else if let Some(value) = this_item.strip_prefix('b') {
        if value.is_empty() {
            //`b<identify>` without any binary digit, nothing to store
            return None;
        }
        if let Some(identify) = line_item.next() {
            let (clk, v) = state.value.value.entry(identify.to_owned()).or_insert_with(|| (Vec::new(), Vec::new()));
            clk.push(state.clk);
            parse_bin_string(value, v);
        }
        None
    }else {
        //single bit value: the first char is the value, the rest is the identifier
        let mut chars = this_item.chars();
        let digit = chars.next()?;
        let value = match digit {
            '1' | 'h' | 'H' => (0, 1),
            '0' | 'l' | 'L' => (0, 0),
            //VCD also allows u/w/- for "unknown", show them as x
            'x' | 'X' | 'u' | 'U' | 'w' | 'W' | '-' => (1, 0),
            'z' | 'Z' => (1, 1),
            _ => return None,
        };
        Some(ParseAction::ValueSingle(chars.as_str().to_string(), value))
    }
    
}

fn parse_bin_string(input: &str, value: &mut Vec<(u8, u8)>) {//TODO:perf is bad
    let length = input.len();
    let input_head = "0".repeat((8 - length % 8) % 8);
    let input_convert = input_head + input;
    input_convert.as_bytes()
        .chunks(8)
        .map(|x| x.iter().fold((0,0), |a,&b|
            match b {
                b'1' | b'h' | b'H' => (2*a.0, 2*a.1+1),
                //VCD also allows u/w/- for "unknown", show them as x
                b'x' | b'X' | b'u' | b'U' | b'w' | b'W' | b'-' => (2*a.0+1, 2*a.1),
                b'z' | b'Z' => (2*a.0+1, 2*a.1+1),
                _ => (2*a.0, 2*a.1),//b'0', b'l', b'L' and anything unknown
            }))
        .for_each(|x| value.push(x))
}
