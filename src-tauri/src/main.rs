#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::fs;
use std::path::PathBuf;
use wave_insight_lib::{
    parser::vcd_parser::vcd_parser,
    //parser::verilog_parser::verilog_parser,
    data_struct::Module,
    data_struct::ModuleValue};

struct State {
    path: PathBuf,
    module: Module,
    module_value: ModuleValue,
}

#[tauri::command]
fn get_file_list(state: tauri::State<State>, name: Vec<String>) -> Vec<String> {
    println!("click!");
    let mut dest_path = state.path.clone();
    name.into_iter().for_each(|x| dest_path.push(&x));
    let paths = fs::read_dir(&dest_path).unwrap();

    paths.filter_map(|entry| {
        entry.ok().and_then(|e|
        e.path().file_name()
        .and_then(|n| n.to_str().map(|s| String::from(s)))
    )
    }).collect::<Vec<String>>()
}

fn main() {
    tauri::Builder::default()
        .manage(State {
            path: std::env::current_dir().unwrap(),
            module: Module::new(),
            module_value: ModuleValue::new()
        })
        .invoke_handler(tauri::generate_handler![get_file_list])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
