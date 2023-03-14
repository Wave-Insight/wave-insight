#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use std::fs;
use std::path::Path;

#[tauri::command]
fn get_file_list(name: &str) -> Vec<String> {
    println!("click!");
    let paths = fs::read_dir(&Path::new(&std::env::current_dir().unwrap())).unwrap();

    dbg!(paths.filter_map(|entry| {
        entry.ok().and_then(|e|
        e.path().file_name()
        .and_then(|n| n.to_str().map(|s| String::from(s)))
    )
    }).collect::<Vec<String>>())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_file_list])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
