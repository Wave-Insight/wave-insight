//! Native memory-mapped byte source: answers like the file/memory sources, still
//! validates corrupt input, and releases the mapping when the waveform drops.
//! The whole file is compiled for native targets only (wasm has no mmap).
#![cfg(not(target_arch = "wasm32"))]

use std::path::{Path, PathBuf};

use wave_insight_lib::waveform::{Waveform, WaveformError};

const FST_FIXTURE: &[u8] = include_bytes!("data/io_mode_wave.fst");

const VCD_FIXTURE: &str = "\
$date mmap test $end
$version 1 $end
$timescale 1ns $end
$scope module top $end
$var wire 1 ! clk $end
$var wire 8 % data [7:0] $end
$upscope $end
$enddefinitions $end
#0
0!
b00000000 %
#10
1!
b10101010 %
#20
0!
b11110000 %
#30
1!
b00001111 %
";

fn unique_path(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "wave-insight-mmap-{}-{nanos}-{name}",
        std::process::id()
    ))
}

fn write_temp(name: &str, bytes: &[u8]) -> PathBuf {
    let path = unique_path(name);
    std::fs::write(&path, bytes).expect("temp fixture is writable");
    path
}

fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Compare every public answer of two waveforms loaded from different sources.
fn assert_same_answers(left: &Waveform, right: &Waveform) {
    assert_eq!(left.file_format(), right.file_format());
    assert_eq!(left.signal_count(), right.signal_count());
    assert_eq!(left.date(), right.date());
    assert_eq!(left.version(), right.version());
    assert_eq!(left.start_time().unwrap(), right.start_time().unwrap());
    assert_eq!(left.end_time().unwrap(), right.end_time().unwrap());
    assert_eq!(
        left.time_range_hint().unwrap(),
        right.time_range_hint().unwrap()
    );
    for (left_signal, right_signal) in left.signals().iter().zip(right.signals()) {
        assert_eq!(left_signal.path(), right_signal.path());
        assert_eq!(left_signal.name(), right_signal.name());
        assert_eq!(left_signal.width(), right_signal.width());
        assert_eq!(left_signal.scope(), right_signal.scope());
    }
    let take = left.signal_count().min(3);
    for index in 0..take {
        let left_decoded = left.decoded_changes(index).expect("decoded left");
        let right_decoded = right.decoded_changes(index).expect("decoded right");
        assert_eq!(&left_decoded[..], &right_decoded[..], "signal {index}");
        let end = left.end_time().expect("end").unwrap_or(0);
        let mut left_window = Vec::new();
        left.stream_changes(
            &wave_insight_lib::waveform::ValueQuery::window(index, 0, Some(end)),
            |_, time, value| {
                left_window.push((time, value));
                true
            },
        )
        .expect("stream left");
        let mut right_window = Vec::new();
        right
            .stream_changes(
                &wave_insight_lib::waveform::ValueQuery::window(index, 0, Some(end)),
                |_, time, value| {
                    right_window.push((time, value));
                    true
                },
            )
            .expect("stream right");
        assert_eq!(left_window, right_window, "streamed signal {index}");
        for time in [0u64, 5, 10, 25, 40] {
            assert_eq!(
                left.value_at(index, time).expect("value left"),
                right.value_at(index, time).expect("value right"),
                "value_at signal {index} time {time}"
            );
        }
    }
}

#[test]
fn mmap_source_answers_like_file_and_memory_sources() {
    for (name, bytes) in [
        ("vcd", VCD_FIXTURE.as_bytes().to_vec()),
        ("fst", FST_FIXTURE.to_vec()),
    ] {
        let path = write_temp(name, &bytes);
        let from_file = Waveform::open_path(&path).expect("file source opens");
        let from_mmap = Waveform::open_path_mmap(&path).expect("mmap source opens");
        let from_memory = Waveform::open_bytes(bytes.clone()).expect("memory source opens");
        assert_same_answers(&from_file, &from_mmap);
        assert_same_answers(&from_file, &from_memory);

        assert_eq!(from_file.path(), Some(path.as_path()));
        assert_eq!(from_mmap.path(), Some(path.as_path()));
        assert_eq!(from_memory.path(), None);
        assert_eq!(from_file.source().retained_bytes(), 0);
        assert_eq!(from_mmap.source().retained_bytes(), 0);
        assert_eq!(from_memory.source().retained_bytes(), bytes.len());
        assert_eq!(from_mmap.source().memory_len(), Some(bytes.len()));
        assert_eq!(from_memory.source().memory_len(), Some(bytes.len()));
        assert_eq!(from_file.source().memory_len(), None);
        assert!(from_mmap.source().is_mmap());
        assert!(!from_file.source().is_mmap());
        assert_eq!(from_mmap.mapped_bytes(), bytes.len());

        let file_stats = from_file.memory_stats();
        let mmap_stats = from_mmap.memory_stats();
        let memory_stats = from_memory.memory_stats();
        assert_eq!(file_stats.source_retained_bytes, 0);
        assert_eq!(file_stats.source_mapped_bytes, 0);
        assert_eq!(mmap_stats.source_retained_bytes, 0);
        assert_eq!(mmap_stats.source_mapped_bytes, bytes.len());
        assert_eq!(memory_stats.source_retained_bytes, bytes.len());
        assert_eq!(memory_stats.source_mapped_bytes, 0);
        assert_eq!(file_stats.hierarchy_bytes, mmap_stats.hierarchy_bytes);
        assert_eq!(file_stats.hierarchy, mmap_stats.hierarchy);
        remove(&path);
    }
}

#[test]
fn corrupt_input_through_the_mmap_still_answers_corrupt() {
    // The Wave 13 VCD width conflict: open validates through the same ByteSource.
    let bad_vcd = "$timescale 1ns $end\n$scope module top $end\n\
                   $var wire 64 ! big [63:0] $end\n\
                   $var wire 8 ! small [7:0] $end\n\
                   $upscope $end\n$enddefinitions $end\n#0\nb0 !\n";
    let path = write_temp("bad-vcd", bad_vcd.as_bytes());
    let error = Waveform::open_path_mmap(&path).expect_err("conflicting widths are corrupt");
    assert!(matches!(error, WaveformError::Corrupt { .. }), "{error:?}");
    remove(&path);

    // The Wave 13 FST release abort seed through the mapping.
    let mut bad_fst = FST_FIXTURE.to_vec();
    bad_fst[817] ^= 0xB0;
    let path = write_temp("bad-fst", &bad_fst);
    let error = Waveform::open_path_mmap(&path).expect_err("corrupt FST is rejected");
    assert!(
        matches!(
            error,
            WaveformError::Corrupt {
                format: wave_insight_lib::waveform::WaveFormat::Fst,
                ..
            }
        ),
        "{error:?}"
    );
    remove(&path);
}

#[test]
fn dropping_the_waveform_releases_the_map() {
    let bytes = VCD_FIXTURE.as_bytes().to_vec();
    let path = write_temp("drop", &bytes);
    let waveform = Waveform::open_path_mmap(&path).expect("mmap opens");
    assert_eq!(waveform.mapped_bytes(), bytes.len());
    assert_eq!(waveform.memory_stats().source_mapped_bytes, bytes.len());
    drop(waveform);
    // The mapping (and its file handle) is gone: the dump can be removed and
    // opened again.
    std::fs::remove_file(&path).expect("the file can be removed after drop");
    let again = write_temp("drop-again", &bytes);
    let waveform = Waveform::open_path_mmap(&again).expect("mmap opens again");
    drop(waveform);
    remove(&again);
}

#[test]
fn mmap_io_failures_are_errors_not_panics() {
    let missing = unique_path("missing");
    let error = Waveform::open_path_mmap(&missing).expect_err("missing file");
    assert!(matches!(error, WaveformError::Io(_)), "{error:?}");

    let empty = write_temp("empty", b"");
    let error = Waveform::open_path_mmap(&empty).expect_err("empty file cannot be mapped");
    assert!(matches!(error, WaveformError::Io(_)), "{error:?}");
    remove(&empty);

    // A directory is not a waveform and not mappable.
    let directory = std::env::temp_dir();
    let _ = Waveform::open_path_mmap(&directory);
}

#[test]
fn real_large_fst_if_present_matches_the_file_source() {
    let path = std::env::temp_dir().join("wi-fst").join("large.fst");
    if !path.exists() {
        eprintln!("skipping: {} is not present", path.display());
        return;
    }
    let bytes = std::fs::metadata(&path).expect("metadata").len() as usize;
    let from_file = Waveform::open_path(&path).expect("file source opens the large FST");
    let from_mmap = Waveform::open_path_mmap(&path).expect("mmap source opens the large FST");
    let file_stats = from_file.memory_stats();
    let mmap_stats = from_mmap.memory_stats();
    eprintln!(
        "large FST {} bytes: open_path retained={} mapped={} | open_path_mmap retained={} mapped={}",
        bytes,
        file_stats.source_retained_bytes,
        file_stats.source_mapped_bytes,
        mmap_stats.source_retained_bytes,
        mmap_stats.source_mapped_bytes
    );
    assert_eq!(file_stats.source_retained_bytes, 0);
    assert_eq!(file_stats.source_mapped_bytes, 0);
    assert_eq!(mmap_stats.source_retained_bytes, 0);
    assert_eq!(mmap_stats.source_mapped_bytes, bytes);
    assert_eq!(from_file.file_format(), from_mmap.file_format());
    assert_eq!(from_file.signal_count(), from_mmap.signal_count());
    assert_eq!(
        from_file.time_range_hint().expect("hint file"),
        from_mmap.time_range_hint().expect("hint mmap")
    );
    for (left, right) in from_file.signals().iter().zip(from_mmap.signals()) {
        assert_eq!(left.path(), right.path());
        assert_eq!(left.name(), right.name());
        assert_eq!(left.width(), right.width());
    }
    // Decode a bounded window of the first few rows through both sources: the
    // actual compare is the same values from file reads vs mapped pages.
    let indices: Vec<usize> = from_mmap
        .signals()
        .iter()
        .take(4)
        .map(|signal| signal.index())
        .collect();
    let hint = from_mmap.time_range_hint().unwrap().expect("non-empty");
    let start = hint.start + (hint.end.saturating_sub(hint.start)) / 4;
    let end = hint.start + (hint.end.saturating_sub(hint.start)) / 2;
    let file_rows = from_file
        .window_changes_many(&indices, start, Some(end), 256)
        .expect("file window");
    let mmap_rows = from_mmap
        .window_changes_many(&indices, start, Some(end), 256)
        .expect("mmap window");
    assert_eq!(file_rows, mmap_rows);
}
