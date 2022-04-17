use std::str::SplitWhitespace;

use num::BigUint;

use super::parse_action::ParseAction;

pub fn value_change(mut line_item: SplitWhitespace<'_>, this_item: &str) -> Option<ParseAction> {
    if let Some(clk) = this_item.strip_prefix('#') {
        clk.parse::<i32>().ok()
            .map(ParseAction::Clk)
    }else if let Some(value) = this_item.strip_prefix('b') {
        let identify = line_item.next()?;
        Some(ParseAction::Value(identify.to_string(),BigUint::parse_bytes(value.as_bytes(),2).unwrap()))
    }else if let Some(identify) = this_item.strip_prefix('1') {
        Some(ParseAction::Value(identify.to_string(),BigUint::new(vec![1])))
    }else { 
        this_item.strip_prefix('0')
            .map(|identify| ParseAction::Value(identify.to_string(),BigUint::new(vec![0])))
    }
}
