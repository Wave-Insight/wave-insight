//! Tests for the cached, per signal accessor (`Waveform::decoded_changes`).
//!
//! The renderer relies on three properties: the values are the same ones the
//! rest of the API reports, a redraw reuses the cache instead of re-reading the
//! file, and the cache can be given back (`unload_signal`) even for signals with
//! a long history.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use wave_insight_lib::waveform::{
    CacheStats, Change, DecodedSignal, ValueQuery, Waveform, WaveformError,
};

///`top.clk` is 1 bit, `top.data` is 8 bit with `x` values, `top.sub.flag` is 1 bit
const VCD: &str = "\
$date today $end
$timescale 1ns $end
$scope module top $end
$var wire 1 ! clk $end
$var wire 8 \" data [7:0] $end
$scope module sub $end
$var wire 1 & flag $end
$upscope $end
$upscope $end
$enddefinitions $end
#0
0!
b00000000 \"
0&
#10
1!
b10101010 \"
1&
#20
z!
bxxxxxxxx \"
0&
";

const FST_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/io_mode_wave.fst");

fn open_vcd() -> Waveform {
    Waveform::open_bytes(VCD).expect("the fixture is valid VCD")
}

///A VCD with `changes` alternating edges on a one bit clock, 10 ticks apart.
fn big_vcd(changes: usize) -> String {
    let mut vcd = String::with_capacity(changes * 12 + 128);
    vcd.push_str(
        "$timescale 1ns $end\n$scope module top $end\n$var wire 1 ! clk $end\n$upscope $end\n$enddefinitions $end\n",
    );
    for index in 0..changes {
        vcd.push_str(&format!("#{}\n", index * 10));
        vcd.push_str(if index % 2 == 0 { "0!\n" } else { "1!\n" });
    }
    vcd
}

fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after the epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("wave-insight-{nanos}-{name}"))
}

#[test]
fn decoded_changes_matches_all_changes() {
    let vcd = open_vcd();
    let fst = Waveform::open_path(FST_FIXTURE).expect("the FST fixture opens");
    for waveform in [&vcd, &fst] {
        assert!(!waveform.signals().is_empty());
        for signal in waveform.signals() {
            let decoded = waveform
                .decoded_changes(signal.index())
                .unwrap_or_else(|error| panic!("{}: {error}", signal.path()));
            let all = waveform.all_changes(signal.index()).unwrap();
            assert_eq!(decoded.len(), all.len(), "{}", signal.path());
            assert_eq!(&decoded[..], &all[..], "{}", signal.path());
        }
    }
}

#[test]
fn a_second_call_reuses_the_cached_decode() {
    let waveform = open_vcd();
    let signal = waveform.signal_by_path("top.data").unwrap().index();

    let first = waveform.decoded_changes(signal).unwrap();
    let after_first = waveform.cache_stats();
    let second = waveform.decoded_changes(signal).unwrap();
    let after_second = waveform.cache_stats();

    //same allocation, no second decode
    assert!(first.shares_storage_with(&second));
    assert_eq!(after_first, after_second);
    assert_eq!(after_second.decoded_signals, 1);
    assert_eq!(after_second.decoded_changes, first.len());
    assert_eq!(after_second.loaded_signals, 1);
    assert_eq!(after_second.decoded_bytes(), first.len() * std::mem::size_of::<Change>());

    //decoding another signal adds to the cache instead of replacing it
    let other = waveform.signal_by_path("top.clk").unwrap().index();
    let _ = waveform.decoded_changes(other).unwrap();
    assert_eq!(waveform.cache_stats().decoded_signals, 2);
}

#[test]
fn the_cached_decode_does_not_re_read_the_file() {
    let path = unique_temp_path("no-reread.vcd");
    fs::write(&path, VCD).expect("temp file is writable");
    let waveform = Waveform::open_path(&path).expect("the temp VCD opens");
    let signal = waveform.signal_by_path("top.data").unwrap().index();

    //the first query reads the body
    let first = waveform.decoded_changes(signal).unwrap();
    assert!(!first.is_empty());

    //from now on nothing may need the file: delete it
    fs::remove_file(&path).expect("temp file is removable");

    let second = waveform.decoded_changes(signal).unwrap();
    assert!(first.shares_storage_with(&second));
    assert_eq!(&first[..], &second[..]);

    //`all_changes` also answers from the cached body
    assert_eq!(&waveform.all_changes(signal).unwrap()[..], &second[..]);
    //...and `value_at` does too
    assert!(waveform.value_at(signal, 10).unwrap().is_some());

    //only the streaming path re-reads the file, so it has to fail now
    let error = waveform
        .changes_in_window(signal, 0, None)
        .expect_err("the streaming path needs the file");
    assert!(matches!(error, WaveformError::Io(_)), "unexpected error: {error:?}");

    let _ = fs::remove_file(&path);
}

#[test]
fn a_large_history_can_be_sliced_and_unloaded() {
    const CHANGES: usize = 50_000;
    let waveform = Waveform::open_bytes(big_vcd(CHANGES)).expect("synthetic VCD opens");
    let signal = waveform.signal_by_path("top.clk").unwrap().index();

    let decoded = waveform.decoded_changes(signal).unwrap();
    assert_eq!(decoded.len(), CHANGES);
    assert_eq!(decoded.span(), Some((0, (CHANGES as u64 - 1) * 10)));

    //slicing a small window out of a long history is a binary search, the rest
    //of the history is not touched
    let window = decoded.window(10_000, Some(10_050));
    assert_eq!(window.len(), 6);
    assert_eq!(window[0].0, 10_000);
    assert_eq!(window[5].0, 10_050);
    assert!(decoded.value_at(10_005).is_some());
    assert_eq!(decoded.value_at(10_005).unwrap(), decoded.value_at(10_000).unwrap());

    let stats = waveform.cache_stats();
    assert_eq!(stats.decoded_signals, 1);
    assert_eq!(stats.decoded_changes, CHANGES);
    assert_eq!(stats.decoded_bytes(), CHANGES * std::mem::size_of::<Change>());
    assert!(stats.time_steps > 0);
    //this is the documented cost of the cache: ~40 bytes per change
    assert_eq!(std::mem::size_of::<Change>(), 40);

    //unloading frees the history but keeps the body and the time table
    waveform.unload_signal(signal).unwrap();
    let after = waveform.cache_stats();
    assert_eq!(after.decoded_signals, 0);
    assert_eq!(after.decoded_changes, 0);
    assert_eq!(after.loaded_signals, 0);
    assert_eq!(after.time_steps, stats.time_steps);

    //the handle we still hold is a snapshot, not a dangling reference
    assert_eq!(decoded.len(), CHANGES);
    assert_eq!(decoded.window(10_000, Some(10_050)).len(), 6);

    //and decoding again is correct, though it is a fresh allocation
    let redecoded = waveform.decoded_changes(signal).unwrap();
    assert!(!decoded.shares_storage_with(&redecoded));
    assert_eq!(&decoded[..], &redecoded[..]);
    assert_eq!(waveform.cache_stats().decoded_changes, CHANGES);
}

#[test]
fn value_at_agrees_before_and_after_decoding() {
    let waveform = open_vcd();
    let times = 0..=30u64;

    for signal in waveform.signals() {
        let before: Vec<_> = times
            .clone()
            .map(|time| waveform.value_at(signal.index(), time).unwrap())
            .collect();

        let decoded = waveform.decoded_changes(signal.index()).unwrap();

        let after: Vec<_> = times
            .clone()
            .map(|time| waveform.value_at(signal.index(), time).unwrap())
            .collect();
        assert_eq!(before, after, "{}: value_at changed once decoded", signal.path());

        //the cached slice and the accessor agree with each other
        for time in times.clone() {
            assert_eq!(
                waveform.value_at(signal.index(), time).unwrap(),
                decoded.value_at(time).cloned(),
                "{} at {time}",
                signal.path()
            );
        }
    }
}

#[test]
fn window_slicing_agrees_with_the_streaming_api() {
    let waveform = open_vcd();
    let signal = waveform.signal_by_path("top.data").unwrap().index();
    let decoded = waveform.decoded_changes(signal).unwrap();

    let windows: [(u64, Option<u64>); 8] = [
        (0, None),
        (0, Some(0)),
        (0, Some(10)),
        (5, Some(15)),
        (10, Some(20)),
        (11, Some(19)),
        (20, Some(20)),
        (21, Some(99)),
    ];
    for (start, end) in windows {
        let streamed = waveform.changes_in_window(signal, start, end).unwrap();
        let sliced: Vec<Change> = decoded.window(start, end).to_vec();
        assert_eq!(sliced, streamed, "window {start}..={end:?}");
    }

    //a renderer seeds the left edge with the last change at or before `start`
    let seed = decoded.value_at(11).expect("a value is in effect at 11");
    assert_eq!(seed.to_bit_string().as_deref(), Some("10101010"));
    //which is the change from before the window
    assert!(decoded.window(11, Some(19)).is_empty());
}

#[test]
fn streaming_queries_do_not_populate_the_decode_cache() {
    let waveform = open_vcd();
    let signal = waveform.signal_by_path("top.data").unwrap().index();

    let mut seen = 0usize;
    waveform
        .stream_changes(&ValueQuery::for_signal(signal), |_, _, _| {
            seen += 1;
            true
        })
        .unwrap();
    assert_eq!(seen, 3);

    //streaming runs off its own reader and does not touch the random access
    //cache at all: no body, no decoded signals
    let stats = waveform.cache_stats();
    assert_eq!(stats.decoded_signals, 0);
    assert_eq!(stats.loaded_signals, 0);
    assert_eq!(stats.time_steps, 0, "the eager body was not needed");
}

#[test]
fn decoding_many_signals_at_once_matches_one_by_one() {
    let indices: Vec<usize> = open_vcd()
        .signals()
        .iter()
        .map(|signal| signal.index())
        .collect();

    //one pass for the whole visible row set
    let waveform = open_vcd();
    let batched = waveform.decoded_changes_many(&indices).unwrap();
    assert_eq!(batched.len(), indices.len());
    let stats = waveform.cache_stats();
    assert_eq!(stats.loaded_signals, indices.len());
    assert_eq!(stats.decoded_signals, indices.len());

    //same values as one signal at a time (on a fresh instance, so nothing is
    //shared through the cache)
    let sequential = open_vcd();
    for (position, index) in indices.iter().enumerate() {
        let one = sequential.decoded_changes(*index).unwrap();
        assert_eq!(&one[..], &batched[position][..], "signal {index}");
        assert_eq!(
            sequential.all_changes(*index).unwrap(),
            batched[position].to_vec(),
            "signal {index}"
        );
    }

    //the batch hands out the cached handles: asking again shares storage
    let again = waveform.decoded_changes_many(&indices).unwrap();
    for (position, index) in indices.iter().enumerate() {
        assert!(batched[position].shares_storage_with(&again[position]));
        assert!(batched[position].shares_storage_with(&waveform.decoded_changes(*index).unwrap()));
    }

    //duplicates are decoded once, an empty request is fine, unknown indices fail
    let duplicated = waveform
        .decoded_changes_many(&[indices[0], indices[0]])
        .unwrap();
    assert_eq!(duplicated.len(), 2);
    assert!(duplicated[0].shares_storage_with(&duplicated[1]));
    assert!(waveform.decoded_changes_many(&[]).unwrap().is_empty());
    let error = waveform.decoded_changes_many(&[usize::MAX]).unwrap_err();
    assert!(matches!(error, WaveformError::UnknownSignal(_)));
}

#[test]
fn the_cache_can_be_emptied_whole_or_partially() {
    let waveform = open_vcd();
    let signal = waveform.signal_by_path("top.data").unwrap().index();
    let first = waveform.decoded_changes(signal).unwrap();
    assert!(waveform.cache_stats().time_steps > 0);

    //dropping the decoded signals keeps the parsed body
    waveform.unload_all_signals();
    let after_unload = waveform.cache_stats();
    assert_eq!(after_unload.loaded_signals, 0);
    assert_eq!(after_unload.decoded_signals, 0);
    assert!(after_unload.time_steps > 0, "the body stays parsed");

    //dropping everything returns to the state right after opening
    let _ = waveform.decoded_changes(signal).unwrap();
    waveform.release_cache();
    assert_eq!(waveform.cache_stats(), CacheStats::default());

    //a query rebuilds it, with the same values in a fresh allocation
    let rebuilt = waveform.decoded_changes(signal).unwrap();
    assert!(!first.shares_storage_with(&rebuilt));
    assert_eq!(&first[..], &rebuilt[..]);
}

#[test]
fn cache_stats_are_free_before_the_first_value_query() {
    let waveform = open_vcd();
    //opening only reads the header, so nothing is cached yet
    assert_eq!(waveform.cache_stats(), CacheStats::default());
    assert!(waveform.signal_count() > 0);
    //and asking for stats did not force the body to be read
    assert_eq!(waveform.cache_stats().time_steps, 0);
}

#[test]
fn decoded_handles_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DecodedSignal>();
    assert_send_sync::<Waveform>();
}
