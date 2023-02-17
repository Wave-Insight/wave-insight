use std::str::SplitWhitespace;
use super::parse_action::ParseAction;

pub fn value_change(mut line_item: SplitWhitespace<'_>, this_item: &str) -> Option<ParseAction> {
    if let Some(clk) = this_item.strip_prefix('#') {
        clk.parse::<i32>().ok()
            .map(ParseAction::Clk)
    }else if let Some(value) = this_item.strip_prefix('b') {
        line_item.next()
            .map(|identify| ParseAction::Value(identify.to_string(),parse_bin_string(value)))
    }else if let Some(identify) = this_item.strip_prefix('1') {
        Some(ParseAction::Value(identify.to_string(),vec![(0, 1)]))
    }else if let Some(identify) = this_item.strip_prefix('0'){ 
        Some(ParseAction::Value(identify.to_string(),vec![(0, 0)]))
    }else if let Some(identify) = this_item.strip_prefix('x'){ 
        Some(ParseAction::Value(identify.to_string(), vec![(1,0)]))
    }else {
        this_item.strip_prefix('0')
            .map(|identify| ParseAction::Value(identify.to_string(),vec![(1,1)]))
    }
    
}

fn parse_bin_string(input: &str) -> Vec<(u8,u8)> {//TODO:perf is bad
    let length = input.len();
    let input_head = if length%8 == 0 {""}
        else if length%8 == 1 {"0000000"}
        else if length%8 == 2 {"000000"}
        else if length%8 == 3 {"00000"}
        else if length%8 == 4 {"0000"}
        else if length%8 == 5 {"000"}
        else if length%8 == 6 {"00"}
        else {"0"};
    let input_convert = input_head.to_string() + input;
    let ret = input_convert.as_bytes()
        .chunks(8)
        .map(|x| x.iter().fold((0,0), |a,&b|
            match b {
                b'0' => (2*a.0, 2*a.1),
                b'1' => (2*a.0, 2*a.1+1),
                b'x' => (2*a.0+1, 2*a.1),
                _ => (2*a.0+1, 2*a.1+1),//TODO:check is z?
            }));
    ret.collect()
}
