#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::{fs, str::FromStr};
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use std::rc::Rc;
use wave_insight_lib::waveform::Waveform;
use wave_insight_lib::{
    parser::verilog_parser::verilog_parser,
    data_struct::Module,
    data_struct::ModuleValue};

struct State {
    path: PathBuf,
    module: Mutex<Module>,
    module_value: Mutex<ModuleValue>,
}

///one row of the directory listing the open-file dialog shows
#[derive(serde::Serialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
}

///extensions the app can actually open
fn is_openable(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".vcd")
        || lower.ends_with(".fst")
        || lower.ends_with(".ghw")
        || lower.ends_with(".v")
        || lower.ends_with(".sv")
        || lower.ends_with(".svh")
        || lower.ends_with(".vh")
}

#[tauri::command]
fn get_file_list(state: tauri::State<State>, name: Vec<String>) -> Vec<FileEntry> {
    let mut dest_path = state.path.clone();
    name.into_iter().for_each(|x| dest_path.push(&x));

    let paths = match fs::read_dir(&dest_path) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("can not read {}: {error}", dest_path.display());
            return vec![];
        },
    };

    let mut ret = paths
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_str()?.to_string();
            let is_dir = path.is_dir();
            if !is_dir && !is_openable(&name) {
                return None;
            }
            Some(FileEntry { name, is_dir })
        })
        .collect::<Vec<FileEntry>>();
    //folders first, then names
    ret.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    ret
}

///load a waveform dump, whatever the format: vcd, fst and ghw all go through
///the waveform backend now
fn load_waveform(state: &tauri::State<State>, path: std::path::PathBuf) -> Module {
    match Waveform::open_path(&path).and_then(|waveform| {
        let module = waveform.legacy_module()?;
        let values = waveform.legacy_module_value()?;
        Ok((module, values))
    }) {
        Ok((module, values)) => {
            *state.module.lock().unwrap() = module.clone();
            *state.module_value.lock().unwrap() = values;
            module
        },
        Err(error) => {
            eprintln!("can not open {}: {}", path.display(), error.message());
            Module::new()
        },
    }
}

#[tauri::command]
fn choose_vcd(state: tauri::State<State>, name: Vec<String>) -> Module {
    let mut dest_path = state.path.clone();
    name.into_iter().for_each(|x| dest_path.push(&x));
    load_waveform(&state, dest_path)
}

#[tauri::command]
fn choose_vcd_absolute(state: tauri::State<State>, path: String) -> Module {
    match PathBuf::from_str(&path) {
        Ok(dest_path) => load_waveform(&state, dest_path),
        Err(error) => {
            eprintln!("bad path `{path}`: {error}");
            Module::new()
        },
    }
}

#[tauri::command]
fn choose_verilog(state: tauri::State<State>, name: Vec<String>) -> (String, Module) {
    println!("verilog");
    let mut dest_path = state.path.clone();
    name.into_iter().for_each(|x| dest_path.push(&x));
    let mut file = std::fs::File::open(dest_path).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    let now_module = state.module.lock().unwrap().clone();
    let module_raw = verilog_parser(&contents, Rc::new(now_module));
    *state.module.lock().unwrap() = module_raw.clone();
    (contents, module_raw)
}

#[tauri::command]
fn choose_verilog_absolute(state: tauri::State<State>, path: String) -> (String, Module) {
    println!("verilog drop");
    let dest_path = PathBuf::from_str(&path).unwrap();//TODO:do not unwrap
    let mut file = std::fs::File::open(dest_path).unwrap();
    let mut contents = String::new();
    file.read_to_string(&mut contents).unwrap();
    let now_module = state.module.lock().unwrap().clone();
    let module_raw = verilog_parser(&contents, Rc::new(now_module));
    *state.module.lock().unwrap() = module_raw.clone();
    (contents, module_raw)
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
        .invoke_handler(tauri::generate_handler![get_file_list,
                choose_vcd,
                choose_vcd_absolute,
                choose_verilog,
                choose_verilog_absolute,
                get_value])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
