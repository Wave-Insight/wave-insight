//! Cache of the VCD that a converter produced for an FSDB file.
//!
//! FSDB has no public specification, so every open of an `.fsdb` trace has to go
//! through the vendor reader (`fsdb2vcd`, or the SDK in the next slice).  That
//! conversion costs seconds on a real dump, so its result is kept in a
//! **temporary artifact** directory:
//!
//! * default `%TEMP%\wave-insight-fsdb-cache` (`$TMPDIR` on unix), overridable
//!   with `WAVE_INSIGHT_FSDB_CACHE`;
//! * one file per `<canonical path, length, mtime, converter>`, so a
//!   re-simulation with a new size or time stamp converts again instead of
//!   serving stale values;
//! * nothing here is a second copy of the user's data that must be kept: delete
//!   the directory at any time, the next open converts again.
//!
//! Writes are `<entry>.<pid>.part` followed by a rename, and a `<entry>.lock`
//! created with `create_new` makes concurrent opens of the same file convert
//! once.  The lock is removed by its owner only; another process's live lock is
//! never deleted, a stale one (10 minutes) is.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Environment override for the cache directory.
pub(crate) const ENV_CACHE_DIR: &str = "WAVE_INSIGHT_FSDB_CACHE";

/// A lock older than this is stale (the converting process is gone).
pub(crate) const STALE_LOCK: Duration = Duration::from_secs(10 * 60);

/// The directory the converted VCDs live in; created on demand.
pub(crate) fn cache_dir() -> PathBuf {
    std::env::var_os(ENV_CACHE_DIR)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("wave-insight-fsdb-cache"))
}

/// The cache file for one input, or the place it will be written to.
///
/// The hash covers the canonical path, the file length, the modification time
/// in nanoseconds and the converter name: a re-simulated dump changes at least
/// one of the first two, and a different backend must not reuse another one's
/// output.
pub(crate) fn entry_path(input: &Path, metadata: &std::fs::Metadata, converter: &str) -> PathBuf {
    let name = format!(
        "{}-{:016x}.vcd",
        sanitized_stem(input),
        cache_key(input, metadata, converter)
    );
    cache_dir().join(name)
}

/// `<entry>.<pid>.part`: private until the rename makes it visible.
pub(crate) fn part_path(entry: &Path) -> PathBuf {
    entry.with_file_name(format!("{}.{}.part", file_name(entry), std::process::id()))
}

/// `<entry>.lock`: the conversion is owned by one process at a time.
pub(crate) fn lock_path(entry: &Path) -> PathBuf {
    entry.with_file_name(format!("{}.lock", file_name(entry)))
}

/// A cache entry is usable only when it exists and has bytes: an empty file is
/// what a killed converter could leave behind.
pub(crate) fn is_ready(entry: &Path) -> bool {
    std::fs::metadata(entry)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
}

/// `true` when the lock is old enough that its owner cannot still be working.
pub(crate) fn is_stale(lock: &Path) -> bool {
    let Ok(age) = std::fs::metadata(lock).and_then(|metadata| metadata.modified()) else {
        // A lock that vanished between the create and this check belongs to a
        // process that just finished: treat it as stale so the caller retries.
        return true;
    };
    match SystemTime::now().duration_since(age) {
        Ok(elapsed) => elapsed > STALE_LOCK,
        // A time stamp in the future is not evidence of a dead owner.
        Err(_) => false,
    }
}

fn file_name(entry: &Path) -> String {
    entry
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wave.vcd".to_string())
}

/// The cache key, exactly the tuple documented on [`entry_path`].
fn cache_key(input: &Path, metadata: &std::fs::Metadata, converter: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    input.to_string_lossy().hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    mtime_nanos(metadata).hash(&mut hasher);
    converter.hash(&mut hasher);
    hasher.finish()
}

fn mtime_nanos(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_nanos())
        .unwrap_or(0)
}

/// A file name component that keeps the trace's stem readable (`top-3f2a...vcd`)
/// without letting a path or a unicode name escape the cache directory.
fn sanitized_stem(input: &Path) -> String {
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("fsdb");
    let mut sanitized: String = stem
        .chars()
        .take(48)
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        sanitized.push_str("fsdb");
    }
    sanitized
}
