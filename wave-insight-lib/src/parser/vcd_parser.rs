use std::{vec, str::SplitWhitespace};
use std::collections::HashMap;

use crate::data_struct::{Module, Signal, CodeLocation};

type SignalPath = (Vec<String>,String);
type ModulePath = Vec<String>;
type FuncType = (Module,HashMap<String,SignalPath>,ModulePath,i32);
//TODO: perf:insert signal or module should not find the destiny module each time
pub fn vcd_parser(input: &str, raw_module: Module) -> Module {
    let lines = input.lines();
    let dump_out = lines.fold((raw_module,HashMap::new(),vec![],0),|(module,identify_table,module_path,clock),line|
        parsing_line((module,identify_table,module_path,clock),line.to_string())
    );
    dump_out.0
}

fn parsing_line(input: FuncType, line: String) -> FuncType {
    let mut line_item = line.split_whitespace();
    match line_item.next() {
        Some("$var") => insert_signal(input, line_item),
        Some("$scope") => insert_module(input, line_item),
        Some("$upscope") => pop_module(input),
        Some(_) => input,
        None => input,
    }
}

fn insert_signal((module,identify_table,module_path,clock): FuncType, mut line_item: SplitWhitespace) -> FuncType {
    //  $var wire  1 1 clk $end
    let mut module_out = module;
    let mut identify_table_out = identify_table;

    line_item.next();//throw "wire"
    let size = line_item.next().unwrap().parse::<usize>().unwrap();
    let identify = line_item.next().unwrap();
    let name = line_item.next().unwrap().to_string();

    let same_value_signal = identify_table_out.get(identify).cloned();
    identify_table_out.entry(identify.to_string()).or_insert((module_path.clone(),name.clone()));
    let new_signal = Signal{
        size,
        value_change: vec![],
        same_value_signal,
        module_path: module_path.clone(),
        location_define: CodeLocation{file_name:"".to_string(),line:0},
        location_drive: vec![],
        location_load: vec![],
    };

    let destiny_module = (&module_path).iter()
                        .fold(&mut module_out,|m,p| m.sub_module.get_mut(p).unwrap());
    destiny_module.signal.entry(name).or_insert(new_signal);

    (module_out,identify_table_out,module_path,clock)
}

fn insert_module((module,identify_table,module_path,clock): FuncType, mut line_item: SplitWhitespace) -> FuncType {
    // $scope module TOP $end
    let mut module_out = module;
    let mut module_path_out = module_path;

    line_item.next();//throw "module"
    let name = line_item.next().unwrap();
    let destiny_module = (&module_path_out).iter()
                        .fold(&mut module_out,|m,p| m.sub_module.get_mut(p).unwrap());
    destiny_module.sub_module.entry(name.to_string()).or_insert_with(Module::new);
    module_path_out.push(name.to_string());
    (module_out,identify_table,module_path_out,clock)
}

fn pop_module((module,identify_table,module_path,clock): FuncType) -> FuncType {
    //$upscope $end
    let mut module_path_out = module_path;
    module_path_out.pop();
    (module,identify_table,module_path_out,clock)
}
