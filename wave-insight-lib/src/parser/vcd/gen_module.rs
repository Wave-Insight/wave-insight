use std::str::SplitWhitespace;

use crate::data_struct::{Module, Signal, CodeLocation};

use super::parse_action::ParseAction;

pub fn insert_signal(mut line_item: SplitWhitespace<'_>) -> Option<ParseAction> {
    //  $var wire  1 1 clk $end
    line_item.next();//throw "wire"
    let size = line_item.next().unwrap().parse::<usize>().unwrap();
    let identify = line_item.next().unwrap();
    let name = line_item.next().unwrap().to_string();

    let signal = Signal{
        size,
        value_key: identify.to_string(),
        load: vec![],
        drive: vec![],
        location_define: CodeLocation{file_name:"".to_string(),line:0},
        location_drive: vec![],
        location_load: vec![],
    };
    Some(ParseAction::Signal(name, signal))
}

pub fn insert_module(mut line_item: SplitWhitespace<'_>) -> Option<ParseAction> {
    // $scope module TOP $end
    line_item.next();//throw "module"
    let name = line_item.next().unwrap();
    Some(ParseAction::Module(name.to_string(), Module::new()))
}

pub fn pop_module() -> Option<ParseAction> {
    //$upscope $end
    Some(ParseAction::EndModule)
}
