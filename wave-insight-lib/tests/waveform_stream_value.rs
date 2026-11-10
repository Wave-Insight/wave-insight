//! Memory-flat value lookup: equal to the decoded value_at, but it never
//! materialises the random-access body.

use wave_insight_lib::waveform::{Waveform, WaveformError};

const FST_FIXTURE: &[u8] = include_bytes!("data/io_mode_wave.fst");

/// A VCD with a first change at #10, a clock, an 8 bit bus and an idle bit.
fn fixture_vcd() -> String {
    let mut vcd = String::new();
    vcd.push_str("$date streaming value test $end\n$version 1 $end\n");
    vcd.push_str("$timescale 1ns $end\n$scope module top $end\n");
    vcd.push_str("$var wire 1 ! clk $end\n");
    vcd.push_str("$var wire 8 % data [7:0] $end\n");
    vcd.push_str("$var wire 1 & idle $end\n");
    vcd.push_str("$upscope $end\n$enddefinitions $end\n");
    for step in 0..200u64 {
        vcd.push_str(&format!("#{step}\n"));
        if step >= 5 {
            vcd.push(if step % 2 == 0 { '0' } else { '1' });
            vcd.push_str("!\n");
        }
        if step % 10 == 0 && step >= 10 {
            vcd.push_str(&format!("b{:08b} %\n", (step / 10) as u8));
        }
        if step == 0 {
            vcd.push_str("0&\n");
        }
    }
    vcd
}

const FIRST_CHANGE: u64 = 5;

fn decoded_value(
    waveform: &Waveform,
    signal: usize,
    time: u64,
) -> Option<(u64, wave_insight_lib::waveform::WaveValue)> {
    let decoded = waveform.decoded_changes(signal).expect("decoded");
    decoded
        .change_at(time)
        .map(|(change_time, value)| (*change_time, value.clone()))
}

#[test]
fn vcd_streaming_lookup_matches_the_decoded_history() {
    let waveform = Waveform::open_bytes(fixture_vcd().into_bytes()).expect("VCD opens");
    let signals: Vec<usize> = waveform
        .signals()
        .iter()
        .map(|signal| signal.index())
        .collect();
    let mut checked = 0usize;
    for signal in signals {
        for time in [
            0u64,
            1,
            4,
            FIRST_CHANGE - 1,
            FIRST_CHANGE,
            6,
            10,
            11,
            55,
            199,
            200,
            500,
        ] {
            let expected = decoded_value(&waveform, signal, time);
            let actual = waveform
                .stream_value_before(signal, time)
                .expect("streaming lookup");
            assert_eq!(actual, expected, "signal {signal} time {time}");
            checked += 1;
        }
    }
    assert!(checked > 30);
}

#[test]
fn fst_streaming_lookup_matches_the_decoded_history() {
    let waveform = Waveform::open_bytes(FST_FIXTURE.to_vec()).expect("FST fixture opens");
    let signals: Vec<usize> = waveform
        .signals()
        .iter()
        .map(|signal| signal.index())
        .collect();
    let (start, end) = waveform
        .time_range_hint()
        .expect("hint")
        .map(|range| (range.start, range.end))
        .unwrap_or((0, 0));
    let mid = start + (end.saturating_sub(start)) / 2;
    for signal in signals {
        for time in [
            start.saturating_sub(1),
            start,
            mid,
            end.saturating_sub(1),
            end,
            end + 10,
        ] {
            let expected = decoded_value(&waveform, signal, time);
            let actual = waveform
                .stream_value_before(signal, time)
                .expect("streaming lookup");
            assert_eq!(actual, expected, "signal {signal} time {time}");
        }
    }
}

#[test]
fn streaming_lookup_does_not_materialise_the_body() {
    let waveform = Waveform::open_bytes(fixture_vcd().into_bytes()).expect("VCD opens");
    assert_eq!(waveform.cache_stats(), Default::default());
    let signal = waveform.signal_by_path("top.clk").expect("clk").index();
    for time in [0u64, 10, 50, 150, 500] {
        let _ = waveform
            .stream_value_before(signal, time)
            .expect("streaming lookup");
        let cache = waveform.cache_stats();
        assert_eq!(cache.time_steps, 0, "no body time table at time {time}");
        assert_eq!(cache.time_table_bytes, 0);
        assert_eq!(cache.packed_bytes, 0);
        assert_eq!(cache.decoded_signals, 0);
        assert_eq!(cache.decoded_struct_bytes, 0);
        let memory = waveform.memory_stats();
        assert_eq!(
            memory.cache_bytes, 0,
            "no random access cache at time {time}"
        );
        assert_eq!(memory.time_table_bytes, 0);
        assert_eq!(memory.packed_bytes, 0);
        assert!(memory.lookup_cache_entries >= 1);
        assert!(memory.lookup_cache_entries <= 64);
        assert!(memory.lookup_cache_bytes > 0);
        assert_eq!(
            memory.total_bytes,
            memory.hierarchy_bytes
                + memory.source_retained_bytes
                + memory.cache_bytes
                + memory.lookup_cache_bytes
        );
    }
}

#[test]
fn before_the_first_change_is_none() {
    let waveform = Waveform::open_bytes(fixture_vcd().into_bytes()).expect("VCD opens");
    let clock = waveform.signal_by_path("top.clk").expect("clk").index();
    assert_eq!(
        waveform
            .stream_value_before(clock, FIRST_CHANGE - 1)
            .expect("lookup"),
        None
    );
    let (change_time, _) = waveform
        .stream_value_before(clock, FIRST_CHANGE)
        .expect("lookup")
        .expect("a change exactly at FIRST_CHANGE");
    assert_eq!(change_time, FIRST_CHANGE);
}

#[test]
fn cache_is_bounded_and_repeats_are_free() {
    let waveform = Waveform::open_bytes(fixture_vcd().into_bytes()).expect("VCD opens");
    let signal = waveform.signal_by_path("top.data").expect("data").index();
    let first = waveform.stream_value_before(signal, 55).expect("lookup");
    let second = waveform.stream_value_before(signal, 55).expect("lookup");
    assert_eq!(first, second);
    assert_eq!(waveform.memory_stats().lookup_cache_entries, 1);
    // More distinct (signal, time) pairs than the LRU capacity: the cache stays
    // bounded, answers stay correct.
    for time in 0..200u64 {
        let _ = waveform
            .stream_value_before(signal, time)
            .expect("streaming lookup");
        assert!(waveform.memory_stats().lookup_cache_entries <= 64);
    }
    assert_eq!(waveform.memory_stats().lookup_cache_entries, 64);
}

#[test]
fn unknown_signal_is_an_error() {
    let waveform = Waveform::open_bytes(fixture_vcd().into_bytes()).expect("VCD opens");
    let error = waveform
        .stream_value_before(usize::MAX, 10)
        .expect_err("unknown signal");
    assert!(
        matches!(error, WaveformError::UnknownSignal(_)),
        "{error:?}"
    );
}

#[test]
fn batch_results_match_the_single_lookups_and_keep_request_order() {
    let waveform = Waveform::open_bytes(fixture_vcd().into_bytes()).expect("VCD opens");
    let clock = waveform.signal_by_path("top.clk").expect("clk").index();
    let data = waveform.signal_by_path("top.data").expect("data").index();
    let idle = waveform.signal_by_path("top.idle").expect("idle").index();
    for time in [0u64, 4, 5, 55, 199, 500] {
        let request = [data, clock, data, idle];
        let batch = waveform.values_before(&request, time).expect("batch");
        // Duplicates collapse, request order is kept.
        assert_eq!(batch.len(), 3, "time {time}");
        for (position, signal) in [data, clock, idle].iter().enumerate() {
            let single = waveform
                .stream_value_before(*signal, time)
                .expect("single lookup");
            assert_eq!(batch[position], single, "signal {signal} time {time}");
        }
    }
    assert!(waveform.values_before(&[], 10).expect("empty").is_empty());
    let error = waveform
        .values_before(&[clock, usize::MAX], 10)
        .expect_err("unknown signal");
    assert!(
        matches!(error, WaveformError::UnknownSignal(_)),
        "{error:?}"
    );
}

#[test]
fn batch_lookup_keeps_the_body_unmaterialised_and_warms_the_lru() {
    let waveform = Waveform::open_bytes(fixture_vcd().into_bytes()).expect("VCD opens");
    let indices: Vec<usize> = waveform
        .signals()
        .iter()
        .map(|signal| signal.index())
        .collect();
    let request: Vec<usize> = indices.iter().copied().chain([indices[0]]).collect();
    let first = waveform.values_before(&request, 55).expect("batch");
    assert_eq!(first.len(), indices.len());
    let cache = waveform.cache_stats();
    assert_eq!(cache.time_steps, 0);
    assert_eq!(cache.time_table_bytes, 0);
    assert_eq!(cache.packed_bytes, 0);
    assert_eq!(waveform.memory_stats().cache_bytes, 0);
    let memory = waveform.memory_stats();
    assert_eq!(memory.lookup_cache_entries, indices.len());
    assert!(memory.lookup_cache_bytes > 0);
    // Same batch again: equal answers from the LRU.
    let second = waveform.values_before(&request, 55).expect("batch again");
    assert_eq!(first, second);
    // A single lookup for one of the rows also hits the LRU and agrees.
    assert_eq!(
        waveform
            .stream_value_before(indices[0], 55)
            .expect("single"),
        first[0]
    );
}

#[test]
fn batch_on_the_fst_fixture_matches_the_single_lookups() {
    let waveform = Waveform::open_bytes(FST_FIXTURE.to_vec()).expect("FST fixture opens");
    let indices: Vec<usize> = waveform
        .signals()
        .iter()
        .take(8)
        .map(|signal| signal.index())
        .collect();
    let (start, end) = waveform
        .time_range_hint()
        .expect("hint")
        .map(|range| (range.start, range.end))
        .unwrap_or((0, 0));
    for time in [start, start + (end - start) / 2, end] {
        let batch = waveform.values_before(&indices, time).expect("batch");
        for (position, signal) in indices.iter().enumerate() {
            assert_eq!(
                batch[position],
                waveform.stream_value_before(*signal, time).expect("single"),
                "signal {signal} time {time}"
            );
        }
    }
}

#[test]
fn fst_time_range_hint_does_not_materialise_the_body() {
    let waveform = Waveform::open_bytes(FST_FIXTURE.to_vec()).expect("FST fixture opens");
    let hint = waveform
        .time_range_hint()
        .expect("hint")
        .expect("fixture has a header time range");
    let cache = waveform.cache_stats();
    let memory = waveform.memory_stats();
    assert_eq!(
        cache.time_steps, 0,
        "the time table must not be materialised"
    );
    assert_eq!(cache.time_table_bytes, 0);
    assert_eq!(cache.packed_bytes, 0);
    assert_eq!(memory.cache_bytes, 0);

    // The same answer as the exact body-backed start/end on another instance.
    let reference = Waveform::open_bytes(FST_FIXTURE.to_vec()).expect("FST fixture opens");
    let start = reference.start_time().expect("start").expect("non-empty");
    let end = reference.end_time().expect("end").expect("non-empty");
    assert_eq!(hint.start, start);
    assert_eq!(hint.end, end);
}

#[test]
fn real_large_fst_lookup_stays_memory_flat() {
    let path = std::env::temp_dir().join("wi-fst").join("large.fst");
    if !path.exists() {
        eprintln!("skipping: {} is not present", path.display());
        return;
    }
    let waveform = Waveform::open_path_mmap(&path).expect("large FST opens");
    let hint = waveform
        .time_range_hint()
        .expect("hint")
        .expect("non-empty");
    let mid = hint.start + (hint.end.saturating_sub(hint.start)) / 2;
    // The open path (dump tab summary) must not build the time table either.
    let after_hint = waveform.cache_stats();
    assert_eq!(after_hint.time_steps, 0);
    assert_eq!(after_hint.time_table_bytes, 0);
    let signals: Vec<usize> = waveform
        .signals()
        .iter()
        .take(4)
        .map(|signal| signal.index())
        .collect();
    let begin = std::time::Instant::now();
    for signal in &signals {
        let _ = waveform
            .stream_value_before(*signal, mid)
            .expect("streaming lookup");
    }
    let elapsed = begin.elapsed();
    let cache = waveform.cache_stats();
    let memory = waveform.memory_stats();
    eprintln!(
        "large FST stream_value_before 4 signals at mid: {:?}, cache time_steps={} time_table_bytes={} packed_bytes={}, lookup entries={} bytes={}",
        elapsed,
        cache.time_steps,
        cache.time_table_bytes,
        cache.packed_bytes,
        memory.lookup_cache_entries,
        memory.lookup_cache_bytes
    );
    assert_eq!(cache.time_steps, 0);
    assert_eq!(cache.time_table_bytes, 0);
    assert_eq!(cache.packed_bytes, 0);
    assert_eq!(memory.cache_bytes, 0);

    // Batch lookup for a screen full of rows: one pass instead of one pass per
    // signal.  Timed here so a local run reports the win; correctness is the
    // equality with the single-signal lookups above.
    let batch_indices: Vec<usize> = waveform
        .signals()
        .iter()
        .take(16)
        .map(|signal| signal.index())
        .collect();
    // Two fresh waveforms so the timings do not share a warmed LRU.
    let sequential = Waveform::open_path_mmap(&path).expect("large FST opens");
    let sequential_begin = std::time::Instant::now();
    for signal in &batch_indices {
        let _ = sequential
            .stream_value_before(*signal, mid)
            .expect("single lookup");
    }
    let sequential_elapsed = sequential_begin.elapsed();
    let batch_probe = Waveform::open_path_mmap(&path).expect("large FST opens");
    let begin = std::time::Instant::now();
    let batch = batch_probe
        .values_before(&batch_indices, mid)
        .expect("batch lookup on the large FST");
    let batch_elapsed = begin.elapsed();
    eprintln!(
        "large FST batch values_before 16 signals at mid: {:?} (sequential single lookups would have been {:?}, now mostly LRU hits)",
        batch_elapsed, sequential_elapsed
    );
    assert_eq!(batch.len(), batch_indices.len());
    let after_batch = waveform.cache_stats();
    assert_eq!(after_batch.time_steps, 0);
    assert_eq!(after_batch.time_table_bytes, 0);
    assert_eq!(after_batch.packed_bytes, 0);
}
