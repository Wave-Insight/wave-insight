//! Hostile and edge case input for the waveform backend.
//!
//! Opening a corrupt file must produce a [`WaveformError`], not a panic, and
//! the error must be usable from the UI (a message plus a proper
//! `std::error::Error` implementation).

use std::error::Error;
use std::path::Path;

use wave_insight_lib::waveform::{Waveform, WaveformError};

const FIXTURE_BYTES: &[u8] = include_bytes!("data/io_mode_wave.fst");

#[test]
fn empty_input_is_unknown() {
    let error = Waveform::open_bytes(Vec::new()).unwrap_err();
    assert!(
        matches!(error, WaveformError::UnknownFormat),
        "expected UnknownFormat, got {error:?}"
    );
    assert!(error.to_string().contains("unknown waveform format"));
}

#[test]
fn garbage_is_reported_as_an_error() {
    let garbage: Vec<u8> = (0u8..=255).collect();
    let error = Waveform::open_bytes(garbage).unwrap_err();
    assert!(
        matches!(error, WaveformError::UnknownFormat),
        "expected UnknownFormat, got {error:?}"
    );
}

#[test]
fn missing_file_is_an_io_error() {
    let error = Waveform::open_path(Path::new("this/does/not/exist.vcd")).unwrap_err();
    assert!(
        matches!(error, WaveformError::Io(_)),
        "expected Io, got {error:?}"
    );
    //the source of the error is the io error, so `?`/`anyhow` chains stay useful
    let source: &dyn Error = error.source().expect("io error as source");
    assert!(source.to_string().len() > 0);
}

#[test]
fn vcd_header_without_enddefinitions_is_rejected() {
    let truncated = "\
$scope module top $end
$var wire 8 ! data [7:0] $end
$upscope $end
";
    let error = Waveform::open_bytes(truncated.as_bytes()).unwrap_err();
    assert!(
        matches!(error, WaveformError::Malformed(_)),
        "expected Malformed, got {error:?}"
    );
    //the message names the format that was being parsed
    let message = error.to_string().to_lowercase();
    assert!(message.contains("vcd"), "{error}");
}

#[test]
fn truncated_fst_is_reported_and_does_not_panic() {
    //only the magic and part of the header
    for len in [3usize, 12, 64, 400] {
        let truncated = &FIXTURE_BYTES[..len.min(FIXTURE_BYTES.len())];
        match Waveform::open_bytes(truncated) {
            Err(error) => {
                assert!(!error.to_string().is_empty());
            }
            Ok(waveform) => {
                //if wellen accepted the header, reading values must not panic
                for signal in waveform.signals().iter().take(3) {
                    let _ = waveform.all_changes(signal.index());
                }
                let _ = waveform.end_time();
            }
        }
    }
}

#[test]
fn unknown_signal_indices_are_errors_not_panics() {
    let waveform = Waveform::open_bytes(FIXTURE_BYTES).expect("fixture opens");
    let error = waveform.value_at(usize::MAX, 0).unwrap_err();
    assert!(matches!(error, WaveformError::UnknownSignal(_)));
    assert!(error.to_string().contains("no such signal"));
    assert!(waveform.changes_in_window(usize::MAX, 0, None).is_err());
    assert!(waveform.unload_signal(usize::MAX).is_err());
}
