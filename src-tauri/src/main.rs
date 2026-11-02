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

use dumps::{DumpRegistry, DumpSummary, FoundValue, HierarchyPage, WindowDto};
use wave_insight_lib::waveform::ShowType;

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

/// Join the path components the dialog collected onto `base`.
///
/// This is the dialog's containment rule: a component that is empty or
/// contains a separator is ignored, so a pick can only ever walk down from the
/// base directory. Pure on purpose: [`resolve`] feeds it [`base_dir`], the
/// native open command feeds it the same way, and the unit tests feed it a
/// temporary base.
fn resolve_from(base: &Path, parts: &[String]) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in parts {
        // a component with a separator would jump out of the navigation the
        // dialog collected; the dialog never produces one
        if part.is_empty() || part.contains('/') || part.contains('\\') {
            continue;
        }
        path.push(part);
    }
    path
}

/// Join the path components the dialog collected onto [`base_dir`].
fn resolve(parts: &[String]) -> std::io::Result<PathBuf> {
    Ok(resolve_from(&base_dir(), parts))
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
///
/// `path` must be absolute (drag and drop, launch arguments). A path picked in
/// the in-app dialog is relative to [`base_dir`]; use [`open_dump_parts`] for
/// that one, otherwise the path would resolve against the process working
/// directory (System32 / the app directory on Windows) and the native open
/// would fail.
#[tauri::command]
fn open_dump(
    registry: tauri::State<'_, DumpRegistry>,
    path: String,
) -> Result<DumpSummary, String> {
    registry.open(&path)
}

/// Resolve the in-app dialog's path parts for a native open.
///
/// Same rules as the byte-returning dialog commands ([`resolve`]): the parts
/// are joined onto [`base_dir`] and empty/separator-containing components are
/// ignored. An empty pick is an error here because there is no dump to open.
fn resolve_dump_parts(parts: &[String]) -> Result<PathBuf, String> {
    if parts.iter().all(|part| part.trim().is_empty()) {
        return Err("open_dump_parts: no path components given".to_string());
    }
    resolve(parts).map_err(|error| format!("open_dump_parts: {error}"))
}

/// Open a dump the in-app dialog picked, by its path parts relative to the
/// dialog's base directory (USERPROFILE/HOME).
#[tauri::command]
fn open_dump_parts(
    registry: tauri::State<'_, DumpRegistry>,
    parts: Vec<String>,
) -> Result<DumpSummary, String> {
    let path = resolve_dump_parts(&parts)?;
    registry.open(&path.display().to_string())
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

/// Find the first change of a signal whose rendered value matches a query.
///
/// `from` is a time cursor: forwards starts strictly after it (0 includes the
/// start of the trace), backwards finds the last match strictly before it
/// (u64::MAX means "from the end"). The matching rules are documented on
/// DumpRegistry::find; the search streams the signal, so memory stays O(1) and
/// it stops at the first match (worst case: a full trace scan when nothing
/// matches or for a backwards search).
#[tauri::command]
fn dump_find(
    registry: tauri::State<'_, DumpRegistry>,
    id: u64,
    signal: usize,
    query: String,
    from: u64,
    backwards: bool,
    show: ShowType,
) -> Result<Option<FoundValue>, String> {
    registry.find(id, signal, &query, from, backwards, &show)
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
            open_dump_parts,
            dump_hierarchy,
            dump_window,
            dump_value_at,
            dump_find,
            close_dump
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const SMALL_VCD: &str = "$date dialog test $end\n$version wave-insight dialog parts $end\n$timescale 1ns $end\n$scope module top $end\n$var wire 1 ! clk $end\n$upscope $end\n$enddefinitions $end\n$dumpvars\n0!\n$end\n#10\n1!\n";

    #[test]
    fn resolve_from_joins_the_dialog_parts() {
        let base = Path::new("C:/Users/tester");
        let parts = ["Downloads".to_string(), "foo.fst".to_string()];
        assert_eq!(
            resolve_from(base, &parts),
            PathBuf::from("C:/Users/tester/Downloads/foo.fst")
        );
    }

    #[test]
    fn resolve_from_ignores_empty_and_separator_parts() {
        let base = Path::new("C:/Users/tester");
        let parts = [
            String::new(),
            "Downloads".to_string(),
            "..\\evil".to_string(),
            "a/b".to_string(),
            "C:\\Windows".to_string(),
            "foo.fst".to_string(),
        ];
        assert_eq!(
            resolve_from(base, &parts),
            PathBuf::from("C:/Users/tester/Downloads/foo.fst")
        );
        assert_eq!(resolve_from(base, &[]), base);
    }

    #[test]
    fn resolve_is_absolute_and_keeps_the_parts() {
        let resolved = resolve(&["Downloads".to_string(), "foo.fst".to_string()])
            .expect("resolve the dialog parts");
        assert!(resolved.is_absolute(), "{}", resolved.display());
        assert!(
            resolved.ends_with("Downloads/foo.fst") || resolved.ends_with("Downloads\\foo.fst"),
            "{}",
            resolved.display()
        );
        assert!(base_dir().is_dir(), "the dialog base is a directory");
    }

    #[test]
    fn empty_parts_are_a_clean_error_for_the_native_open() {
        let error = resolve_dump_parts(&[]).expect_err("empty pick");
        assert!(error.contains("no path components"), "{error}");
        let error = resolve_dump_parts(&[String::new(), " ".to_string()]).expect_err("blank pick");
        assert!(error.contains("no path components"), "{error}");
    }

    #[test]
    fn dialog_parts_resolve_to_a_file_the_registry_can_open() {
        // the command's core: join the parts onto a base, then open the
        // resolved file through the same registry path as open_dump
        let base = std::env::temp_dir().join(format!("wi-dialog-parts-{}", std::process::id()));
        let downloads = base.join("Downloads");
        std::fs::create_dir_all(&downloads).expect("create the temp dialog dir");
        let picked = downloads.join("picked.vcd");
        let mut file = std::fs::File::create(&picked).expect("create picked.vcd");
        file.write_all(SMALL_VCD.as_bytes())
            .expect("write picked.vcd");

        let parts = ["Downloads".to_string(), "picked.vcd".to_string()];
        let path = resolve_from(&base, &parts);
        assert_eq!(path, picked);

        let registry = DumpRegistry::new();
        let summary = registry
            .open(&path.display().to_string())
            .expect("open the resolved dump natively");
        assert_eq!(summary.name, "picked.vcd");
        assert_eq!(summary.format, "VCD");
        assert_eq!(summary.signal_count, 1);
        registry.close(summary.id).expect("close");
    }
}
