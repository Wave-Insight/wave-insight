//! Companion RTL discovery for the desktop open path.
//!
//! SpinalHDL (and most simulators) write the generated sources next to the
//! waveform. The layout verified in the SpinalHDL sources (dev branch,
//! `SimBootstraps.scala`) is:
//!
//! ```text
//! simWorkspace/<WorkspaceName>/rtl/<generated>.v   <- generated Verilog/SystemVerilog
//! simWorkspace/<WorkspaceName>/<testName>.vcd      <- the waveform of the run
//! simWorkspace/<WorkspaceName>/verilator/ ...      <- backend build directories
//! simWorkspace/<WorkspaceName>/ghdl/ ...
//! ```
//!
//! so the waveform's own directory has an `rtl` sub-directory. A dump that sits
//! in a backend build directory (`simWorkspace/<name>/verilator/test.vcd`) has
//! it one level up, and a waveform copied out of the workspace has nothing. The
//! scanner therefore checks the dump's directory and up to two ancestors.
//!
//! The rules, in one place:
//!
//! * **candidate directories** — `rtl` (the SpinalHDL rule), then `generated`
//!   and `verilog` (accepted aliases other generators use), in that order;
//! * **levels** — nearest first: the dump directory, its parent and its
//!   grandparent. All candidate directories of the first level that yields any
//!   source are used; the ancestors are not consulted then, so a waveform in a
//!   workspace never pulls in the RTL of an unrelated workspace above it;
//! * **files** — `.v` and `.sv`, directly in the candidate directory or one
//!   level below (generators sometimes group modules in sub-directories). The
//!   list is sorted and de-duplicated by absolute path, so two runs of the same
//!   scan are identical;
//! * **caps** — at most [`MAX_SOURCES`] files and [`MAX_BYTES`] bytes in total;
//!   the list is cut in sorted order, so the selection is deterministic;
//! * **skipped** — everything else, notably `.vhd`/`.vhdl`: the front end only
//!   parses Verilog/SystemVerilog, and a VHDL file in the generated directory
//!   would otherwise fail to parse on every open.
//!
//! This module is pure filesystem inspection: no tauri types, so it is unit
//! tested with temporary directories on the host.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// At most this many sources are reported.
pub const MAX_SOURCES: usize = 128;
/// At most this many bytes of sources are reported.
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Ancestor directories above the dump directory that are checked (2).
pub const MAX_ANCESTORS: usize = 2;
/// Sub-directory names checked at each level, primary rule first.
pub const CANDIDATE_DIRS: [&str; 3] = ["rtl", "generated", "verilog"];
/// Extensions the front end can parse (lower case, without the dot).
pub const SOURCE_EXTENSIONS: [&str; 2] = ["v", "sv"];
/// Safety cap on the candidate paths a single scan gathers before sorting; a
/// hand-made directory with a million entries must not stall the scan.
pub const MAX_SCANNED: usize = 4096;

/// The companion sources of a dump: absolute paths, sorted, capped.
///
/// `dump_path` is the absolute path of the waveform the desktop build opened.
/// Anything unreadable (a directory that vanished, a permission error) simply
/// contributes nothing: the scan is best effort and the caller treats an empty
/// answer as "no companion sources".
pub fn companion_sources(dump_path: &Path) -> Vec<String> {
    let Some(dump_dir) = dump_path.parent() else {
        return Vec::new();
    };
    // dump dir, parent, grandparent: nearest first
    let mut level = Some(dump_dir);
    let mut levels = 0usize;
    while let Some(dir) = level {
        if levels > MAX_ANCESTORS {
            break;
        }
        let mut found = Vec::new();
        for candidate in CANDIDATE_DIRS {
            collect_candidate_dir(&dir.join(candidate), &mut found);
        }
        if !found.is_empty() {
            return select(found);
        }
        levels += 1;
        level = dir.parent();
    }
    Vec::new()
}

/// Gather the source files of one candidate directory (flat and one level deep).
fn collect_candidate_dir(dir: &Path, found: &mut Vec<(PathBuf, u64)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut files: Vec<PathBuf> = Vec::new();
    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => subdirs.push(path),
            Ok(file_type) if file_type.is_file() => files.push(path),
            // a symlink is followed like the entry it points at only when the
            // metadata below says what it is; skipping it keeps the scan cheap
            _ => {}
        }
    }
    for path in files {
        push_source(path, found);
    }
    // one level below the candidate directory, for generators that group files
    subdirs.truncate(MAX_SCANNED.saturating_sub(found.len()));
    for subdir in subdirs {
        let Ok(entries) = fs::read_dir(&subdir) else {
            continue;
        };
        for entry in entries.flatten() {
            if found.len() >= MAX_SCANNED {
                return;
            }
            let path = entry.path();
            if entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
                push_source(path, found);
            }
        }
    }
}

/// Record one file when it is a Verilog/SystemVerilog source the front end can
/// parse and its size is readable.
fn push_source(path: PathBuf, found: &mut Vec<(PathBuf, u64)>) {
    if found.len() >= MAX_SCANNED {
        return;
    }
    if !is_source(&path) {
        return;
    }
    let Ok(metadata) = fs::metadata(&path) else {
        return;
    };
    if metadata.is_file() {
        found.push((path, metadata.len()));
    }
}

/// `true` for a `.v` or `.sv` path (case-insensitive; `.vhd`/`.vhdl` are not).
pub fn is_source(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    SOURCE_EXTENSIONS.contains(&extension.as_str())
}

/// Sort, de-duplicate and cap the gathered files (pure: the caps are here so the
/// budget rules are unit tested without creating 17 MiB of temporary files).
fn select(found: Vec<(PathBuf, u64)>) -> Vec<String> {
    let mut unique: BTreeMap<PathBuf, u64> = BTreeMap::new();
    for (path, bytes) in found {
        // the same file reached twice (a symlinked candidate directory) keeps
        // the smallest size reported, which is never above the real one
        unique
            .entry(path)
            .and_modify(|known| *known = (*known).min(bytes))
            .or_insert(bytes);
    }
    let mut total = 0u64;
    let mut selected = Vec::new();
    for (path, bytes) in unique {
        if selected.len() >= MAX_SOURCES {
            break;
        }
        if total.saturating_add(bytes) > MAX_BYTES {
            // the list is sorted, so skipping one file here keeps the selection
            // deterministic; the remaining files are simply not loaded
            continue;
        }
        total = total.saturating_add(bytes);
        selected.push(path.display().to_string());
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A unique temporary directory that is removed when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "wave-insight-companion-{tag}-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create the temp dir");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn file(&self, relative: &str, bytes: usize) -> PathBuf {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create the file's directory");
            }
            fs::write(&path, vec![b' '; bytes]).expect("write the temp file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(paths: &[String]) -> Vec<String> {
        paths
            .iter()
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn the_spinalhdl_layout_is_found_next_to_the_vcd() {
        let temp = TempDir::new("spinal");
        let vcd = temp.file("simWorkspace/MyDesign/test.vcd", 8);
        temp.file("simWorkspace/MyDesign/rtl/MyDesign.v", 64);
        temp.file("simWorkspace/MyDesign/rtl/MyDesign_regs.sv", 32);
        // one level below the rtl directory is part of the rule
        temp.file("simWorkspace/MyDesign/rtl/ip/MyDesign_rom.sv", 16);
        // unrelated directories must not leak in
        temp.file("simWorkspace/MyDesign/verilator/obj_dir/VMyDesign.cpp", 16);
        temp.file("simWorkspace/Other/rtl/Other.v", 16);

        let sources = companion_sources(&vcd);
        assert_eq!(
            names(&sources),
            vec!["MyDesign.v", "MyDesign_regs.sv", "MyDesign_rom.sv"]
        );
        for source in &sources {
            assert!(Path::new(source).is_absolute(), "not absolute: {source}");
            assert!(source.contains("simWorkspace"), "{source}");
            assert!(source.contains("MyDesign"), "{source}");
        }
    }

    #[test]
    fn a_dump_in_a_backend_subdirectory_finds_the_rtl_one_level_up() {
        let temp = TempDir::new("backend");
        let vcd = temp.file("simWorkspace/MyDesign/verilator/test.vcd", 8);
        temp.file("simWorkspace/MyDesign/rtl/MyDesign.v", 32);
        assert_eq!(names(&companion_sources(&vcd)), vec!["MyDesign.v"]);
    }

    #[test]
    fn the_nearest_level_wins_over_an_ancestor() {
        let temp = TempDir::new("nearest");
        let vcd = temp.file("workspace/design/run/test.vcd", 8);
        // the design's own rtl exists...
        temp.file("workspace/design/run/rtl/run.v", 16);
        // ...and an ancestor has one too; the ancestor must not be mixed in
        temp.file("workspace/rtl/ancestor.v", 16);
        assert_eq!(names(&companion_sources(&vcd)), vec!["run.v"]);
    }

    #[test]
    fn generated_and_verilog_are_accepted_aliases() {
        let temp = TempDir::new("aliases");
        let generated = temp.file("a/generated/top.v", 16);
        let verilog = temp.file("b/verilog/top.sv", 16);
        assert_eq!(names(&companion_sources(&generated)), vec!["top.v"]);
        assert_eq!(names(&companion_sources(&verilog)), vec!["top.sv"]);
    }

    #[test]
    fn vhdl_and_unrelated_extensions_are_skipped() {
        let temp = TempDir::new("vhdl");
        let vcd = temp.file("run/test.vcd", 8);
        temp.file("run/rtl/top.vhd", 16);
        temp.file("run/rtl/top.vhdl", 16);
        temp.file("run/rtl/readme.txt", 16);
        temp.file("run/rtl/top.sv", 16);
        assert_eq!(names(&companion_sources(&vcd)), vec!["top.sv"]);
    }

    #[test]
    fn nothing_found_is_an_empty_answer_not_an_error() {
        let temp = TempDir::new("empty");
        let vcd = temp.file("plain/test.vcd", 8);
        assert!(companion_sources(&vcd).is_empty());
        // a path with no parent at all cannot be scanned
        assert!(companion_sources(Path::new("test.vcd")).is_empty());
        // a directory that does not exist contributes nothing
        assert!(companion_sources(&temp.path().join("missing/run.vcd")).is_empty());
    }

    #[test]
    fn the_answer_is_sorted_and_deduplicated() {
        let temp = TempDir::new("sorted");
        let vcd = temp.file("run/test.vcd", 8);
        temp.file("run/rtl/b.v", 16);
        temp.file("run/rtl/a.sv", 16);
        temp.file("run/rtl/c.v", 16);
        let sources = companion_sources(&vcd);
        let sorted = {
            let mut sorted = sources.clone();
            sorted.sort();
            sorted
        };
        assert_eq!(sources, sorted);
        // the same file cannot appear twice even when two candidate directories
        // resolve to the same place
        let mut all = sources.clone();
        all.extend(sources.clone());
        all.sort();
        all.dedup();
        assert_eq!(all.len(), sources.len());
    }

    #[test]
    fn the_file_count_is_capped() {
        let temp = TempDir::new("cap-count");
        let vcd = temp.file("run/test.vcd", 8);
        for index in 0..(MAX_SOURCES + 10) {
            temp.file(&format!("run/rtl/file{index:04}.v"), 4);
        }
        assert_eq!(companion_sources(&vcd).len(), MAX_SOURCES);
    }

    #[test]
    fn the_byte_budget_is_capped_deterministically() {
        // a fabricated list: 3 files of 6 MiB, the 16 MiB budget admits two
        let found: Vec<(PathBuf, u64)> = (0..3)
            .map(|index| {
                (
                    PathBuf::from(format!("C:/rtl/file{index}.v")),
                    6 * 1024 * 1024,
                )
            })
            .collect();
        let selected = select(found.clone());
        assert_eq!(selected.len(), 2);
        // and the same input always produces the same answer
        assert_eq!(select(found), selected);
        // one file larger than the whole budget on its own is dropped
        let too_big = vec![(PathBuf::from("C:/rtl/big.v"), MAX_BYTES + 1)];
        assert!(select(too_big).is_empty());
    }

    #[test]
    fn source_extensions_are_matched_case_insensitively() {
        assert!(is_source(Path::new("a.v")));
        assert!(is_source(Path::new("a.SV")));
        assert!(is_source(Path::new("a.sV")));
        assert!(!is_source(Path::new("a.vhd")));
        assert!(!is_source(Path::new("a.vhdl")));
        assert!(!is_source(Path::new("a.svh")));
        assert!(!is_source(Path::new("no-extension")));
    }
}
