//! FSDB (Synopsys Verdi waveform) support through the vendor reader.
//!
//! FSDB has no public specification. Every open-source viewer that reads it
//! either links Synopsys's free FSDB Reader SDK (GTKWave built with
//! `--enable-fsdb`, Vaporview's optional addon, `ondas`/`wavepeek`) or shells out
//! to a Verdi tool. This module is the seam for both:
//!
//! * **this slice** - `ToolConverter` runs
//!   `fsdb2vcd <in.fsdb> -o <out.vcd>` (Verdi's CLI reads a trace without a
//!   licence) and the result is cached as a VCD, which the existing wellen
//!   pipeline then reads. No vendor library is linked, nothing is bundled and the
//!   default build is unchanged.
//! * **next slice** - a `sdk` module behind `--features fsdb` that implements the
//!   same [`Converter`] trait on the FSDB Reader SDK (`VERDI_HOME`). The trait,
//!   the cache and the provenance below are the seam it plugs into; it does not
//!   exist yet and [`default_converter`] returns `None` for it.
//!
//! A conversion is identified by its input (canonical path, length, mtime) and
//! the converter, so a re-simulated dump converts again. The cache is a
//! temporary artifact and may be deleted at any time.

mod cache;
mod tool;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::error::{Result, WaveformError};

#[allow(unused_imports)]
pub use super::is_fsdb_path;

/// How long a concurrent conversion may be waited for before the caller gets an
/// error instead (another process owns the same cache entry).
const WAIT_FOR_CONVERSION: Duration = Duration::from_secs(120);
/// Poll interval while another process converts.
const WAIT_POLL: Duration = Duration::from_millis(100);

/// One backend that turns an FSDB file into a VCD.
///
/// The CLI tool and (next slice) the vendor SDK implement this; the tests use a
/// small in-memory implementation. Implementations write exactly to `output` and
/// report a user-readable error without a trailing newline.
pub trait Converter {
    /// Short name that is part of the cache key, e.g. `"fsdb2vcd"`.
    fn name(&self) -> &'static str;

    /// Convert `input` into a VCD at `output`.
    fn convert(&self, input: &Path, output: &Path) -> std::result::Result<(), String>;
}

/// The result of a conversion: where the VCD is and what produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    /// Cached VCD that wellen can read.
    pub vcd: PathBuf,
    /// Name of the converter that produced it.
    pub converter: String,
    /// Milliseconds the conversion took; `0.0` when a cached VCD was reused.
    pub convert_ms: f64,
}

/// The converter this build would use, `None` when none is available.
///
/// Order: the test seam (unit tests only), then the vendor-SDK hook behind
/// `--features fsdb` (not implemented in this slice), then the CLI tool found on
/// `PATH` or named by `WAVE_INSIGHT_FSDB2VCD`.
pub fn default_converter() -> Option<Box<dyn Converter>> {
    #[cfg(test)]
    if let Some(converter) = testing::converter() {
        return Some(converter);
    }
    #[cfg(feature = "fsdb")]
    if let Some(converter) = sdk_converter() {
        return Some(converter);
    }
    tool::ToolConverter::discover().map(|converter| Box::new(converter) as Box<dyn Converter>)
}

/// The next slice's backend: a `Converter` over the FSDB Reader SDK, built
/// against `VERDI_HOME`. Deliberately absent so `--features fsdb` builds still
/// work (and fall back to the CLI tool).
#[cfg(feature = "fsdb")]
fn sdk_converter() -> Option<Box<dyn Converter>> {
    None
}

/// Convert `path` with the default converter of this build.
pub fn prepare(path: &Path) -> Result<Prepared> {
    // A missing/unreadable file is reported as such before converters are
    // discussed: "no converter" for a typo would send the user chasing PATH.
    std::fs::metadata(path).map_err(|error| WaveformError::FsdbConversion {
        path: path.to_path_buf(),
        detail: format!("cannot read the file: {error}"),
    })?;
    let converter = default_converter().ok_or_else(|| WaveformError::FsdbUnavailable {
        detail: format!(
            "no FSDB converter for {} (fsdb2vcd was not found and the fsdb feature is off)",
            path.display()
        ),
    })?;
    prepare_with(path, converter.as_ref())
}

/// Convert `path` with an explicit converter (the tests inject one).
///
/// The cache entry is computed from the canonical input path, its length, its
/// modification time and the converter name. A ready entry is returned without
/// running the converter; otherwise the conversion runs under a lock and is
/// published with a rename.
pub fn prepare_with(path: &Path, converter: &dyn Converter) -> Result<Prepared> {
    let converter_name = converter.name();
    let input = std::fs::canonicalize(path).map_err(|error| WaveformError::FsdbConversion {
        path: path.to_path_buf(),
        detail: format!("cannot read the file: {error}"),
    })?;
    let metadata = std::fs::metadata(&input).map_err(|error| WaveformError::FsdbConversion {
        path: path.to_path_buf(),
        detail: format!("cannot read the file: {error}"),
    })?;
    let entry = cache::entry_path(&input, &metadata, converter_name);
    if cache::is_ready(&entry) {
        return Ok(Prepared {
            vcd: entry,
            converter: converter_name.to_string(),
            convert_ms: 0.0,
        });
    }
    if let Some(parent) = entry.parent() {
        std::fs::create_dir_all(parent).map_err(|error| WaveformError::FsdbConversion {
            path: path.to_path_buf(),
            detail: format!(
                "cannot create the cache directory {}: {error}",
                parent.display()
            ),
        })?;
    }

    let lock = cache::lock_path(&entry);
    let _lock = match acquire_lock(&lock, &entry, path)? {
        Lock::Owned(guard) => guard,
        Lock::Ready => {
            return Ok(Prepared {
                vcd: entry,
                converter: converter_name.to_string(),
                convert_ms: 0.0,
            })
        }
    };
    // Another process may have finished between the readiness check and the
    // lock; the second check keeps that from converting again.
    if cache::is_ready(&entry) {
        return Ok(Prepared {
            vcd: entry,
            converter: converter_name.to_string(),
            convert_ms: 0.0,
        });
    }

    let part = cache::part_path(&entry);
    let mut part_guard = PartGuard {
        path: part.clone(),
        armed: true,
    };
    let started = Instant::now();
    // The converter gets the caller's spelling of the path, not the canonical
    // one: on Windows canonicalize adds a `\\?\` prefix that vendor tools do not
    // all accept.
    let outcome = converter.convert(path, &part);
    let convert_ms = started.elapsed().as_secs_f64() * 1000.0;
    if let Err(detail) = outcome.and_then(|()| validate_output(&part)) {
        drop(part_guard);
        return Err(WaveformError::FsdbConversion {
            path: path.to_path_buf(),
            detail,
        });
    }
    if let Err(error) = std::fs::rename(&part, &entry) {
        drop(part_guard);
        return Err(WaveformError::FsdbConversion {
            path: path.to_path_buf(),
            detail: format!(
                "the converter wrote {} but it could not be moved into the cache: {error}",
                part.display()
            ),
        });
    }
    part_guard.armed = false;
    Ok(Prepared {
        vcd: entry,
        converter: converter_name.to_string(),
        convert_ms,
    })
}

/// A converter result that is worth opening: an existing, non-empty file.
///
/// Split out of the spawn so a tool that silently wrote nothing is rejected by
/// the same check the in-memory test converters go through.
pub(crate) fn validate_output(path: &Path) -> std::result::Result<(), String> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > 0 => Ok(()),
        Ok(_) => Err(format!(
            "the converter did not write a VCD ({} is empty)",
            path.display()
        )),
        Err(error) => Err(format!(
            "the converter did not write a VCD ({error}: {})",
            path.display()
        )),
    }
}

enum Lock {
    /// This process owns the conversion and must remove the lock on drop.
    Owned(LockGuard),
    /// Another process finished while this one waited: use its entry.
    Ready,
}

struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        // Only the owner removes the lock. A stale lock was replaced before this
        // guard existed, never while another process could own it.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A partial output that is removed unless the conversion published it.
struct PartGuard {
    path: PathBuf,
    armed: bool,
}

impl Drop for PartGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn acquire_lock(lock: &Path, entry: &Path, input: &Path) -> Result<Lock> {
    for _ in 0..4 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock)
        {
            Ok(mut file) => {
                use std::io::Write;
                let _ = writeln!(file, "{}", std::process::id());
                return Ok(Lock::Owned(LockGuard {
                    path: lock.to_path_buf(),
                }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if cache::is_stale(lock) {
                    // The owner is gone (killed conversion, reboot): recover.
                    let _ = std::fs::remove_file(lock);
                    continue;
                }
                // A live conversion owns the entry: wait for its result instead
                // of converting a second time.
                let deadline = Instant::now() + WAIT_FOR_CONVERSION;
                loop {
                    if cache::is_ready(entry) {
                        return Ok(Lock::Ready);
                    }
                    if Instant::now() >= deadline {
                        return Err(WaveformError::FsdbConversion {
                            path: input.to_path_buf(),
                            detail: format!(
                                "another conversion is still running (waited {} s for {})",
                                WAIT_FOR_CONVERSION.as_secs(),
                                entry.display()
                            ),
                        });
                    }
                    std::thread::sleep(WAIT_POLL);
                }
            }
            Err(error) => {
                return Err(WaveformError::FsdbConversion {
                    path: input.to_path_buf(),
                    detail: format!(
                        "cannot create the conversion lock {}: {error}",
                        lock.display()
                    ),
                })
            }
        }
    }
    Err(WaveformError::FsdbConversion {
        path: input.to_path_buf(),
        detail: format!("cannot acquire the conversion lock {}", lock.display()),
    })
}

#[cfg(test)]
pub(crate) mod testing {
    //! Injection point for the tests: the FSDB entry point must be exercised
    //! without a Verdi install. Production code never sees an installed
    //! converter.

    use std::cell::RefCell;
    use std::path::Path;
    use std::rc::Rc;

    use super::Converter;

    thread_local! {
        static INSTALLED: RefCell<Option<Rc<dyn Converter>>> = const { RefCell::new(None) };
    }

    struct Shared(Rc<dyn Converter>);

    impl Converter for Shared {
        fn name(&self) -> &'static str {
            self.0.name()
        }

        fn convert(&self, input: &Path, output: &Path) -> std::result::Result<(), String> {
            self.0.convert(input, output)
        }
    }

    pub(crate) fn install(converter: Rc<dyn Converter>) {
        INSTALLED.with(|slot| *slot.borrow_mut() = Some(converter));
    }

    pub(crate) fn clear() {
        INSTALLED.with(|slot| *slot.borrow_mut() = None);
    }

    pub(crate) fn converter() -> Option<Box<dyn Converter>> {
        INSTALLED.with(|slot| {
            slot.borrow()
                .as_ref()
                .map(|converter| Box::new(Shared(Rc::clone(converter))) as Box<dyn Converter>)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::{TimeRange, WaveFormat, Waveform, WaveformError};
    use std::cell::Cell;
    use std::io::Write;
    use std::rc::Rc;
    use std::time::{SystemTime, UNIX_EPOCH};

    const SMALL_VCD: &str = "$date fsdb conversion test $end\n$version fsdb2vcd $end\n$timescale 1ns $end\n$scope module top $end\n$var wire 1 ! clk $end\n$var wire 4 \" data [3:0] $end\n$upscope $end\n$enddefinitions $end\n#0\n0!\nb0000 \"\n#10\n1!\nb1010 \"\n#20\n0!\n";

    struct FakeConverter {
        name: &'static str,
        calls: Cell<usize>,
        vcd: &'static str,
        write: bool,
    }

    impl FakeConverter {
        fn writing(name: &'static str) -> Self {
            Self {
                name,
                calls: Cell::new(0),
                vcd: SMALL_VCD,
                write: true,
            }
        }

        fn silent(name: &'static str) -> Self {
            Self {
                name,
                calls: Cell::new(0),
                vcd: SMALL_VCD,
                write: false,
            }
        }
    }

    impl Converter for FakeConverter {
        fn name(&self) -> &'static str {
            self.name
        }

        fn convert(&self, _input: &Path, output: &Path) -> std::result::Result<(), String> {
            self.calls.set(self.calls.get() + 1);
            if !self.write {
                return Ok(());
            }
            std::fs::write(output, self.vcd).map_err(|error| error.to_string())
        }
    }

    struct Installed;

    impl Installed {
        fn new(converter: Rc<dyn Converter>) -> Self {
            testing::install(converter);
            Self
        }
    }

    impl Drop for Installed {
        fn drop(&mut self) {
            testing::clear();
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|age| age.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "wave-insight-fsdb-test-{}-{stamp}-{name}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create the test directory");
        dir
    }

    fn leftovers(entry: &Path, suffix: &str) -> Vec<PathBuf> {
        let Some(name) = entry
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            return Vec::new();
        };
        let Some(dir) = entry.parent() else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .map(|file| {
                        let file = file.to_string_lossy();
                        file.starts_with(&name) && file.ends_with(suffix)
                    })
                    .unwrap_or(false)
            })
            .collect()
    }

    fn touch(input: &Path, modified: SystemTime) {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(input)
            .expect("open the input for set_modified");
        file.set_modified(modified).expect("set the input mtime");
    }

    #[test]
    fn only_a_fsdb_extension_is_fsdb() {
        assert!(is_fsdb_path(Path::new("run.fsdb")));
        assert!(is_fsdb_path(Path::new("RUN.FSDB")));
        assert!(is_fsdb_path(Path::new("a.b.fsdb")));
        assert!(!is_fsdb_path(Path::new("run.fsdb.gz")));
        assert!(!is_fsdb_path(Path::new("run.fsdbx")));
        assert!(!is_fsdb_path(Path::new("run.vcd")));
        assert!(!is_fsdb_path(Path::new("fsdb")));
        assert!(!is_fsdb_path(Path::new("")));
    }

    #[test]
    fn a_cached_conversion_is_reused_until_the_input_changes() {
        let dir = temp_dir("reuse");
        let input = dir.join("trace.fsdb");
        std::fs::write(&input, b"input bytes").expect("write the input");
        let converter = FakeConverter::writing("fsdb2vcd");

        let first = prepare_with(&input, &converter).expect("first conversion");
        assert_eq!(converter.calls.get(), 1);
        assert!(first.convert_ms >= 0.0);
        assert!(first.vcd.is_file());
        assert!(leftovers(&first.vcd, ".part").is_empty());

        let second = prepare_with(&input, &converter).expect("cached conversion");
        assert_eq!(converter.calls.get(), 1, "the cache answer is reused");
        assert_eq!(second.vcd, first.vcd);
        assert_eq!(second.convert_ms, 0.0, "a cache hit reports no conversion");

        // Same path and length, new time stamp: a re-simulation converts again.
        touch(
            &input,
            SystemTime::now() + std::time::Duration::from_secs(60),
        );
        let third = prepare_with(&input, &converter).expect("re-conversion");
        assert_eq!(converter.calls.get(), 2);
        assert_ne!(third.vcd, first.vcd);
        assert!(leftovers(&third.vcd, ".part").is_empty());

        for entry in [first.vcd, third.vcd] {
            let _ = std::fs::remove_file(&entry);
            let _ = std::fs::remove_file(cache::lock_path(&entry));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_stale_lock_is_recovered() {
        let dir = temp_dir("stale-lock");
        let input = dir.join("run.fsdb");
        std::fs::write(&input, b"input bytes").expect("write the input");
        // the cache key uses the canonical path (a `\\?\` verbatim path on
        // Windows), so the test must use the same spelling
        let input = std::fs::canonicalize(&input).expect("canonicalize the input");
        let metadata = std::fs::metadata(&input).expect("metadata");
        let entry = cache::entry_path(&input, &metadata, "fsdb2vcd");
        assert!(!entry.exists());
        let lock = cache::lock_path(&entry);
        std::fs::write(&lock, b"dead process").expect("write the stale lock");
        touch(
            &lock,
            SystemTime::now() - cache::STALE_LOCK - std::time::Duration::from_secs(60),
        );

        let converter = FakeConverter::writing("fsdb2vcd");
        let prepared = prepare_with(&input, &converter).expect("the stale lock is recovered");
        assert_eq!(converter.calls.get(), 1);
        assert!(!lock.exists(), "the stale lock is removed");
        assert!(prepared.vcd.is_file());

        let _ = std::fs::remove_file(&prepared.vcd);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_converter_that_writes_nothing_is_an_error_and_leaves_no_partial_file() {
        let dir = temp_dir("no-output");
        let input = dir.join("trace.fsdb");
        std::fs::write(&input, b"input bytes").expect("write the input");
        let input = std::fs::canonicalize(&input).expect("canonicalize the input");
        let metadata = std::fs::metadata(&input).expect("metadata");
        let entry = cache::entry_path(&input, &metadata, "fsdb2vcd");
        let converter = FakeConverter::silent("fsdb2vcd");

        let error = prepare_with(&input, &converter).expect_err("no output is an error");
        assert!(
            matches!(error, WaveformError::FsdbConversion { .. }),
            "{error:?}"
        );
        let message = error.message();
        assert!(message.contains("trace.fsdb"), "{message}");
        assert!(message.contains("did not write a VCD"), "{message}");
        assert!(!entry.exists(), "nothing was published");
        assert!(leftovers(&entry, ".part").is_empty(), "no .part is left");
        assert!(!cache::lock_path(&entry).exists(), "the lock is released");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_converted_fsdb_opens_through_the_fsdb_entry_point() {
        let dir = temp_dir("pipeline");
        let input = dir.join("design.fsdb");
        std::fs::write(&input, b"fsdb input bytes").expect("write the input");
        let converter = Rc::new(FakeConverter::writing("fsdb2vcd"));
        let _installed = Installed::new(converter.clone());

        let waveform = Waveform::open_path(&input).expect("the converted FSDB opens");
        assert_eq!(converter.calls.get(), 1);
        assert_eq!(waveform.file_format(), WaveFormat::Fsdb);
        assert_eq!(waveform.container_format(), WaveFormat::Vcd);
        assert!(waveform.is_converted());
        assert_eq!(waveform.path(), Some(input.as_path()));

        let origin = waveform.origin().expect("an origin is recorded");
        assert_eq!(origin.format, WaveFormat::Fsdb);
        assert_eq!(origin.path, input);
        assert_eq!(origin.converter, "fsdb2vcd");
        assert!(origin.converted.is_file());
        assert_ne!(origin.converted, origin.path);
        assert!(origin.convert_ms >= 0.0);

        // the hierarchy and the values are the cached VCD's
        assert_eq!(waveform.signal_count(), 2);
        assert_eq!(
            waveform.time_range_hint().expect("a time range"),
            Some(TimeRange { start: 0, end: 20 })
        );
        let clk = waveform
            .signal_by_path("top.clk")
            .expect("the cached VCD's hierarchy")
            .index();
        assert_eq!(
            waveform
                .value_at(clk, 10)
                .expect("value at 10")
                .and_then(|value| value.to_u64()),
            Some(1)
        );
        assert_eq!(
            waveform
                .value_at(clk, 5)
                .expect("value at 5")
                .and_then(|value| value.to_u64()),
            Some(0)
        );
        let data = waveform.value_at(1, 15).expect("value at 15");
        assert_eq!(
            data.and_then(|value| value.to_bit_string()),
            Some(format!("{:04b}", 0b1010))
        );

        // the container is what wellen parsed; the report keeps both
        let debug = format!("{waveform:?}");
        assert!(debug.contains("Fsdb"), "{debug}");
        assert!(debug.contains("Vcd"), "{debug}");

        let converted = origin.converted.clone();
        drop(waveform);
        let _ = std::fs::remove_file(converted);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_plain_dump_keeps_its_own_provenance() {
        let dir = temp_dir("provenance");
        let path = dir.join("plain.vcd");
        std::fs::write(&path, SMALL_VCD).expect("write the VCD");
        let waveform = Waveform::open_path(&path).expect("a plain VCD opens");
        assert_eq!(waveform.file_format(), WaveFormat::Vcd);
        assert_eq!(waveform.container_format(), WaveFormat::Vcd);
        assert!(waveform.origin().is_none());
        assert!(!waveform.is_converted());
        assert_eq!(waveform.path(), Some(path.as_path()));

        let fst = Waveform::open_bytes(
            include_bytes!("../../../../qa/fixtures/io_mode_wave.fst").to_vec(),
        )
        .expect("the checked-in FST fixture opens");
        assert_eq!(fst.file_format(), WaveFormat::Fst);
        assert_eq!(fst.container_format(), WaveFormat::Fst);
        assert!(fst.origin().is_none());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prepare_without_a_converter_is_the_actionable_error() {
        testing::clear();
        let dir = temp_dir("no-converter");
        let input = dir.join("run.fsdb");
        std::fs::write(&input, b"input bytes").expect("write the input");
        let previous = std::env::var_os(tool::ENV_PROGRAM);
        std::env::set_var(tool::ENV_PROGRAM, dir.join("no-such-fsdb2vcd"));
        let result = prepare(&input);
        match previous {
            Some(value) => std::env::set_var(tool::ENV_PROGRAM, value),
            None => std::env::remove_var(tool::ENV_PROGRAM),
        }
        let error = result.expect_err("no converter is available");
        assert!(
            matches!(error, WaveformError::FsdbUnavailable { .. }),
            "{error:?}"
        );
        let message = error.message();
        assert!(message.contains("fsdb2vcd"), "{message}");
        assert!(message.contains("WAVE_INSIGHT_FSDB2VCD"), "{message}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn fsdb_bytes_without_a_path_get_the_actionable_error() {
        let error = Waveform::open_bytes(b"FSDB\x05\x00\x00\x00header".to_vec())
            .expect_err("bytes cannot be converted without a path");
        assert!(
            matches!(error, WaveformError::FsdbUnavailable { .. }),
            "{error:?}"
        );
        assert!(
            error.message().contains("open_path"),
            "the message names the path entry point: {error}"
        );
    }

    #[test]
    fn an_open_file_source_for_an_fsdb_path_is_refused() {
        let error = Waveform::open_source(crate::waveform::WaveformSource::from_path(
            PathBuf::from("run.fsdb"),
        ))
        .expect_err("open_source does not convert");
        assert!(
            matches!(error, WaveformError::FsdbUnavailable { .. }),
            "{error:?}"
        );
    }

    #[test]
    fn a_fresh_part_file_is_never_visible_as_an_entry() {
        // The publication uses a rename, so a failing converter cannot leave an
        // entry that the next open would trust.
        let dir = temp_dir("part-guard");
        let entry = dir.join("trace-0000.vcd");
        let part = cache::part_path(&entry);
        std::fs::write(&part, b"").expect("write an empty part");
        assert!(!cache::is_ready(&entry));
        assert!(!cache::is_ready(&part), "an empty file is not ready");

        let guard = PartGuard {
            path: part.clone(),
            armed: true,
        };
        drop(guard);
        assert!(!part.exists(), "an armed guard removes the part file");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_cache_key_covers_the_converter() {
        let dir = temp_dir("key");
        let input = dir.join("trace.fsdb");
        let mut file = std::fs::File::create(&input).expect("create the input");
        file.write_all(b"bytes").expect("write the input");
        drop(file);
        let metadata = std::fs::metadata(&input).expect("metadata");
        let tool = cache::entry_path(&input, &metadata, "fsdb2vcd");
        let sdk = cache::entry_path(&input, &metadata, "fsdb-reader-sdk");
        assert_ne!(tool, sdk);
        assert_eq!(tool.extension().and_then(|ext| ext.to_str()), Some("vcd"));
        assert_eq!(
            cache::part_path(&tool),
            tool.with_file_name(format!(
                "{}.{}.part",
                tool.file_name().unwrap().to_string_lossy(),
                std::process::id()
            ))
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
