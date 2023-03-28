#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use wave_insight_lib::{
    parser::vcd_parser::vcd_parser,
    //parser::verilog_parser::verilog_parser,
    data_struct::Module,
    data_struct::ModuleValue};

struct State {
    path: PathBuf,
    module: Mutex<Module>,
    module_value: Mutex<ModuleValue>,
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
        .and_then(|n| n.to_str())
        .and_then(|n| {
            let metadata = fs::metadata(dest_path.join(n)).unwrap();
            if metadata.is_dir() || n.strip_suffix(".v").is_some() || n.strip_suffix(".vcd").is_some() {
                Some(String::from(n))
            }else {
                None
            }
        })
    )
    }).collect::<Vec<String>>()
}

#[tauri::command]
fn choose_vcd(state: tauri::State<State>, name: Vec<String>) -> Module {
    println!("vcd!");
    let mut dest_path = state.path.clone();
    name.into_iter().for_each(|x| dest_path.push(&x));
    let mut file = std::fs::File::open(dest_path).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    let (module_raw, signal_value_raw) = vcd_parser(contents, &mut Module::new());
    *state.module.lock().unwrap() = module_raw.clone();
    *state.module_value.lock().unwrap() = signal_value_raw;
    module_raw
}

#[tauri::command]
fn get_value(state: tauri::State<State>, key: String) -> Option<(String, (Vec<i32>, Vec<(u8, u8)>))> {
    println!("signal add: {key}");
    state.module_value.lock().unwrap()
        .value.get(&key)
        .map(|x| (key, x.clone()))
}

fn main() {
    tauri::Builder::default()
        .manage(State {
            path: std::env::current_dir().unwrap(),
            module: Module::new().into(),
            module_value: ModuleValue::new().into(),
        })
        .invoke_handler(tauri::generate_handler![get_file_list, choose_vcd, get_value])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
