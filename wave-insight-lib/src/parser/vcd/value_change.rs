use std::str::SplitWhitespace;

use crate::data_struct::ValueType;

use super::parse_action::ParseAction;

pub fn value_change(mut line_item: SplitWhitespace<'_>, this_item: &str) -> Option<ParseAction> {
    if let Some(clk) = this_item.strip_prefix('#') {
        clk.parse::<i32>().ok()
            .map(ParseAction::Clk)
    }else if let Some(value) = this_item.strip_prefix('b') {
        let identify = line_item.next()?;
        Some(ParseAction::Value(identify.to_string(),ValueType::parse_bin_string(value)))
    }else if let Some(identify) = this_item.strip_prefix('1') {
        Some(ParseAction::Value(identify.to_string(),ValueType::from_u8(1)))
    }else { 
        this_item.strip_prefix('0')
            .map(|identify| ParseAction::Value(identify.to_string(),ValueType::from_u8(0)))
    }
}
