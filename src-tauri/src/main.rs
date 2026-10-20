#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

//! Tauri shell.
//!
//! The backend does not parse anything: the file dialogs hand the raw bytes (or
//! the source text) to the webview, which opens the dump with the very same
//! `Waveform::open_bytes` path the browser build uses. That keeps one reader for
//! both targets and means the desktop app inherits the lazy value decoding of
//! the web build.

use std::fs;
use std::path::{Path, PathBuf};

/// One row of the directory listing the open-file dialog shows.
#[derive(serde::Serialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
}

/// Extensions the app can actually open.
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

/// Join the path components the dialog collected onto the working directory.
fn resolve(parts: &[String]) -> std::io::Result<PathBuf> {
    let mut path = std::env::current_dir()?;
    for part in parts {
        path.push(part);
    }
    Ok(path)
}

#[tauri::command]
fn get_file_list(name: Vec<String>) -> Vec<FileEntry> {
    let dest_path = match resolve(&name) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("can not resolve {name:?}: {error}");
            return vec![];
        }
    };

    let paths = match fs::read_dir(&dest_path) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("can not read {}: {error}", dest_path.display());
            return vec![];
        }
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

/// Read a dump and hand the bytes to the webview.
#[tauri::command]
fn choose_vcd(name: Vec<String>) -> Result<Vec<u8>, String> {
    let path = resolve(&name).map_err(|error| format!("{error}"))?;
    read_bytes(&path)
}

/// Read a dump by absolute path (drag and drop, launch argument).
#[tauri::command]
fn choose_vcd_absolute(path: String) -> Result<Vec<u8>, String> {
    read_bytes(Path::new(&path))
}

/// Read a source file as text.
#[tauri::command]
fn choose_verilog(name: Vec<String>) -> Result<String, String> {
    let path = resolve(&name).map_err(|error| format!("{error}"))?;
    read_text(&path)
}

/// Read a source file by absolute path.
#[tauri::command]
fn choose_verilog_absolute(path: String) -> Result<String, String> {
    read_text(Path::new(&path))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn read_text(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("{}: not utf-8", path.display()))
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_file_list,
            choose_vcd,
            choose_vcd_absolute,
            choose_verilog,
            choose_verilog_absolute
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
