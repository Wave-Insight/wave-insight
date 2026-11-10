//! Bench: batch values_before vs sequential stream_value_before (task-78 follow-up).
//!
//! cargo run -p app --example values_before_bench -- --mode batch|sequential --time <t> [--count 16] [--path <dump>]
//!
//! Opens the dump with the native mmap source directly (same library call the
//! backend uses), resolves the first N signals of the hierarchy, and times the
//! cold and warm readout of all N rows at one time stamp. The caller samples the
//! process working set externally.

#![allow(dead_code)]

use std::time::Instant;

use wave_insight_lib::waveform::{ShowType, Waveform};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut mode = "batch".to_string();
    let mut time = 0u64;
    let mut count = 16usize;
    let mut path = String::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--mode" => {
                index += 1;
                mode = args.get(index).cloned().unwrap_or(mode);
            }
            "--time" => {
                index += 1;
                time = args
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
            }
            "--count" => {
                index += 1;
                count = args
                    .get(index)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(16);
            }
            "--path" => {
                index += 1;
                path = args.get(index).cloned().unwrap_or_default();
            }
            other => {
                if path.is_empty() {
                    path = other.to_string();
                }
            }
        }
        index += 1;
    }
    assert!(
        !path.is_empty(),
        "usage: values_before_bench --mode <m> --time <t> [--count N] [--path <dump>]"
    );
    let waveform = Waveform::open_path_mmap(&path).expect("open with mmap");

    // first N signals in declaration order = the visible rows of a fresh view
    let signals: Vec<usize> = waveform
        .signals()
        .iter()
        .take(count)
        .map(|signal| signal.index())
        .collect();
    println!(
        "OPEN signals={} picked={} time={} mode={mode}",
        waveform.signal_count(),
        signals.len(),
        time
    );

    let show = ShowType::Hex;
    let width_of = |signal: usize| {
        waveform
            .signal(signal)
            .map(|info| info.width())
            .unwrap_or(0)
    };

    // cold pass: nothing decoded yet for these rows in this process
    let started = Instant::now();
    match mode.as_str() {
        "sequential" => {
            for &signal in &signals {
                let _ = waveform
                    .stream_value_before(signal, time)
                    .expect("stream_value_before");
            }
        }
        "batch" => {
            let answers = waveform
                .values_before(&signals, time)
                .expect("values_before");
            assert_eq!(answers.len(), signals.len());
        }
        other => panic!("unknown mode {other} (batch|sequential)"),
    }
    let cold = started.elapsed().as_secs_f64() * 1000.0;

    // warm pass: the per-signal LRU must answer everything
    let started = Instant::now();
    match mode.as_str() {
        "sequential" => {
            for &signal in &signals {
                let _ = waveform
                    .stream_value_before(signal, time)
                    .expect("stream_value_before warm");
            }
        }
        "batch" => {
            let answers = waveform
                .values_before(&signals, time)
                .expect("values_before warm");
            assert_eq!(answers.len(), signals.len());
        }
        _ => {}
    }
    let warm = started.elapsed().as_secs_f64() * 1000.0;

    let stats = waveform.memory_stats();
    let cache_stats = waveform.cache_stats();
    let rendered = waveform
        .values_before(&signals, time)
        .expect("values_before for the readout")
        .into_iter()
        .zip(&signals)
        .filter_map(|(answer, &signal)| {
            answer.map(|(_, value)| value.render(width_of(signal) as usize, &show))
        })
        .count();
    println!(
        "TIMING mode={mode} rows={} cold_ms={cold:.3} warm_ms={warm:.3} rendered={rendered}",
        signals.len()
    );
    println!(
        "STATS time_steps={} time_table_bytes={} packed_bytes={} decoded_struct_bytes={} cache_bytes={} lookup_entries={} lookup_bytes={} total_bytes={}",
        cache_stats.time_steps,
        cache_stats.time_table_bytes,
        cache_stats.packed_bytes,
        cache_stats.decoded_struct_bytes,
        stats.cache_bytes,
        stats.lookup_cache_entries,
        stats.lookup_cache_bytes,
        stats.total_bytes
    );
    println!(
        "PROCESS_MEMORY_JSON {}",
        serde_json::to_string(&process_memory::process_memory()).expect("process memory json")
    );
}

#[path = "../src/process_memory.rs"]
mod process_memory;
