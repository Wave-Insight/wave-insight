#![allow(dead_code)] // the probe only exercises part of the backend
//! Native value_at measurement probe (task-78).
//!
//! cargo run -p app --example value_at_probe -- <dump> <signal-path> <time>[,<time>...]
//!
//! Opens the dump through the native registry (the same code the Tauri commands
//! use), prints the library memory accounting after open / hierarchy / the
//! lookups, and the per-call timings. The *process* working set is sampled
//! externally by the caller (Start-Process + WorkingSet64), so the before/after
//! numbers include everything the native side maps or allocates.

#[path = "../src/dumps.rs"]
mod dumps;

use std::time::Instant;

#[path = "../src/process_memory.rs"]
mod process_memory;

use dumps::DumpRegistry;

fn print_stats(registry: &DumpRegistry, id: u64, label: &str) {
    let stats = registry.memory_stats(id).expect("memory stats");
    println!(
        "{label} {}",
        serde_json::to_string(&stats).expect("stats json")
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: value_at_probe <dump> <signal-path> <time>[,<time>...]");
        std::process::exit(2);
    }
    let path = &args[0];
    let signal_path = &args[1];
    let times: Vec<u64> = args[2]
        .split(',')
        .filter_map(|time| time.trim().parse().ok())
        .collect();
    let registry = DumpRegistry::new();
    let started = Instant::now();
    let summary = registry.open(path).expect("open the dump");
    let open_ms = started.elapsed().as_secs_f64() * 1000.0;
    println!("OPEN_MS {open_ms:.3}");
    print_stats(&registry, summary.id, "STATS_AFTER_OPEN");
    println!(
        "SUMMARY {}",
        serde_json::to_string(&summary).expect("summary json")
    );

    // resolve the signal path through the hierarchy pages (same as the FE)
    let mut offset = 0usize;
    let mut signal = None;
    loop {
        let page = registry
            .hierarchy(summary.id, offset, 4096)
            .expect("hierarchy page");
        if let Some(row) = page
            .rows
            .iter()
            .find(|row| row.kind == "signal" && row.path == *signal_path)
        {
            signal = row.signal;
            break;
        }
        if page.rows.is_empty() {
            break;
        }
        offset += page.rows.len();
    }
    let signal = signal.unwrap_or_else(|| panic!("signal {signal_path} not found"));
    println!("SIGNAL_INDEX {signal}");

    // hierarchy page and window query timings (the rest of the native API)
    let started = Instant::now();
    let page = registry
        .hierarchy(summary.id, 0, 4096)
        .expect("hierarchy page");
    println!(
        "HIERARCHY_PAGE_MS {:.3} rows={}",
        started.elapsed().as_secs_f64() * 1000.0,
        page.rows.len()
    );
    let (window_start, window_end) = match summary.time_range {
        Some(range) => {
            let start = range.start + (range.end - range.start) / 2;
            (start, start + 1_000)
        }
        None => (0, 1_000),
    };
    let started = Instant::now();
    let window = registry
        .window(
            summary.id,
            &[signal],
            window_start,
            Some(window_end),
            Some(256),
            None,
        )
        .expect("window query");
    let changes: usize = window.iter().map(|row| row.changes.len()).sum();
    println!(
        "WINDOW_MS {:.3} window={window_start}..{window_end} changes={changes} capped={}",
        started.elapsed().as_secs_f64() * 1000.0,
        window.iter().any(|row| row.capped)
    );
    print_stats(&registry, summary.id, "STATS_AFTER_HIERARCHY");

    for (index, time) in times.iter().enumerate() {
        let started = Instant::now();
        let value = registry
            .value_at(summary.id, signal, *time, None)
            .expect("value_at");
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        println!(
            "VALUE_AT[{index}] time={time} ms={ms:.3} value={}",
            value.as_deref().unwrap_or("<none>")
        );
    }
    print_stats(&registry, summary.id, "STATS_AFTER_VALUE_AT");

    // same-time repeat: the per-dump cache should make this free
    if let Some(time) = times.last() {
        let started = Instant::now();
        let value = registry
            .value_at(summary.id, signal, *time, None)
            .expect("repeat value_at");
        println!(
            "REPEAT_VALUE_AT time={time} ms={:.3} value={}",
            started.elapsed().as_secs_f64() * 1000.0,
            value.as_deref().unwrap_or("<none>")
        );
    }
    println!(
        "PROCESS_MEMORY_JSON {}",
        serde_json::to_string(&process_memory::process_memory()).expect("process memory json")
    );
}
