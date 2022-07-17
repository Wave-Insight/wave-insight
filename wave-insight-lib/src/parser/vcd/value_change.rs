use std::str::SplitWhitespace;
use super::parse_action::ParseAction;

pub fn value_change(mut line_item: SplitWhitespace<'_>, this_item: &str) -> Option<ParseAction> {
    if let Some(clk) = this_item.strip_prefix('#') {
        clk.parse::<i32>().ok()
            .map(ParseAction::Clk)
    }else if let Some(value) = this_item.strip_prefix('b') {
        let identify = line_item.next()?;
        Some(ParseAction::Value(identify.to_string(),parse_bin_string(value)))
    }else if let Some(identify) = this_item.strip_prefix('1') {
        Some(ParseAction::Value(identify.to_string(),vec![1]))
    }else { 
        this_item.strip_prefix('0')
            .map(|identify| ParseAction::Value(identify.to_string(),vec![0]))
    }
}

fn parse_bin_string(input: &str) -> Vec<u8> {//TODO:perf is bad
    let length = input.len();
    let input_head = if length%8 == 0 {""}
        else if length%8 == 1 {"0000000"}
        else if length%8 == 2 {"000000"}
        else if length%8 == 3 {"00000"}
        else if length%8 == 4 {"0000"}
        else if length%8 == 5 {"000"}
        else if length%8 == 6 {"00"}
        else {"0"};
    let input_convert = input_head.to_string() + &input.to_string();
    let ret = input_convert.as_bytes()
        .chunks(8)
        .map(|x| x.iter().fold(0, |a,&b| 2*a+(b-48)));
    //let drop_prefix_zero: Vec<u8> = ret.skip_while(|&x| x == 0).collect();
    //drop_prefix_zero
    ret.collect()
}
