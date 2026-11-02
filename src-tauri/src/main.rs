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

use tauri::ipc::Response;

mod dumps;

use dumps::{DumpRegistry, DumpSummary, HierarchyPage, WindowDto};

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

/// The directory the in-app file browser starts in.
///
/// It used to be the process working directory, which is `System32` (or the
/// app directory) when the desktop app is started from the Start menu, so the
/// dialog opened somewhere the user has no files. The home directory is what
/// the user expects; `..` still walks up from there, and a part that is empty
/// or a separator is ignored.
fn base_dir() -> PathBuf {
    for key in ["USERPROFILE", "HOME"] {
        if let Ok(value) = std::env::var(key) {
            let path = PathBuf::from(value);
            if path.is_dir() {
                return path;
            }
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Join the path components the dialog collected onto [`base_dir`].
fn resolve(parts: &[String]) -> std::io::Result<PathBuf> {
    let mut path = base_dir();
    for part in parts {
        // a component with a separator would jump out of the navigation the
        // dialog collected; the dialog never produces one
        if part.is_empty() || part.contains('/') || part.contains('\\') {
            continue;
        }
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

/// Read a dump and hand the bytes to the webview as a raw IPC body.
///
/// The return type is deliberately `tauri::ipc::Response`, not `Vec<u8>`:
/// Tauri's default IPC path serializes any `Serialize` value to JSON
/// (`impl<T: Serialize> IpcResponse for T` -> `serde_json::to_string`), so a
/// `Vec<u8>` becomes a JSON array of numbers - one number per byte, several
/// bytes of JSON each. A 100 MB dump therefore becomes a ~300-400 MB JSON
/// string in the native process and a 100M-element JS array in the webview
/// (several hundred MB to >1 GB), which is the memory blow-up the desktop
/// report describes.
///
/// `Response` carries an `InvokeResponseBody::Raw` instead: the webview's
/// `invoke` resolves to an `ArrayBuffer` (see `tauri::ipc::channel`: the raw
/// body is delivered as `new Uint8Array(...).buffer` / `response.arrayBuffer()`),
/// so each side holds exactly one copy. The front end turns it into a
/// `Vec<u8>` with one `Uint8Array::to_vec()` copy
/// (`front-end/src/file_load/filelist.rs`).
#[tauri::command]
fn choose_vcd(name: Vec<String>) -> Result<Response, String> {
    let path = resolve(&name).map_err(|error| format!("{error}"))?;
    read_bytes(&path).map(Response::new)
}

/// Read a dump by absolute path (drag and drop; a launch argument is planned).
///
/// Same raw-body contract as [`choose_vcd`]; the drag-and-drop path in
/// `front-end/src/app.rs` decodes it with the same helper.
#[tauri::command]
fn choose_vcd_absolute(path: String) -> Result<Response, String> {
    read_bytes(Path::new(&path)).map(Response::new)
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

// ---------------------------------------------------------------------------
// Native dump backend (Wave14-B).
//
// The desktop build opens dumps natively (memory mapped when the platform
// allows it) and serves only the rendered windows over IPC, so the file bytes
// never go through the webview. The byte-returning commands above stay
// untouched until the front end switches to the remote renderer.
// ---------------------------------------------------------------------------

/// Open a dump natively; returns the tab summary (id, size, format, range, ...).
#[tauri::command]
fn open_dump(
    registry: tauri::State<'_, DumpRegistry>,
    path: String,
) -> Result<DumpSummary, String> {
    registry.open(&path)
}

/// One page of the flattened hierarchy (scopes and signals) for the sidebar.
#[tauri::command]
fn dump_hierarchy(
    registry: tauri::State<'_, DumpRegistry>,
    id: u64,
    offset: usize,
    limit: usize,
) -> Result<HierarchyPage, String> {
    registry.hierarchy(id, offset, limit)
}

/// The bounded value window of the requested signals, rendered to text.
#[tauri::command]
fn dump_window(
    registry: tauri::State<'_, DumpRegistry>,
    id: u64,
    signals: Vec<usize>,
    start: u64,
    end: Option<u64>,
    max_changes_per_signal: Option<usize>,
    show: Option<String>,
) -> Result<Vec<WindowDto>, String> {
    registry.window(
        id,
        &signals,
        start,
        end,
        max_changes_per_signal,
        show.as_deref(),
    )
}

/// The rendered value of one signal at one time.
#[tauri::command]
fn dump_value_at(
    registry: tauri::State<'_, DumpRegistry>,
    id: u64,
    signal: usize,
    time: u64,
    show: Option<String>,
) -> Result<Option<String>, String> {
    registry.value_at(id, signal, time, show.as_deref())
}

/// Close one native dump and free its mapping.
#[tauri::command]
fn close_dump(registry: tauri::State<'_, DumpRegistry>, id: u64) -> Result<(), String> {
    registry.close(id)
}

fn main() {
    tauri::Builder::default()
        .manage(DumpRegistry::new())
        .invoke_handler(tauri::generate_handler![
            get_file_list,
            choose_vcd,
            choose_vcd_absolute,
            choose_verilog,
            choose_verilog_absolute,
            open_dump,
            dump_hierarchy,
            dump_window,
            dump_value_at,
            close_dump
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
