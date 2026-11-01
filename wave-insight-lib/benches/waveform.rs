//! Criterion benchmarks for the waveform backend (`wave_insight_lib::waveform`).
//!
//! These are the paths a user feels: opening a dump, the time axis on the first
//! frame, decoding one row / a whole screen of rows, a cursor readout, a
//! streamed window, and formatting values for the view.
//!
//! # Running
//!
//! ```text
//! cargo bench -p wave-insight-lib                       # everything, criterion defaults
//! cargo bench -p wave-insight-lib -- --quick            # fast smoke run
//! cargo bench -p wave-insight-lib -- decode             # one group
//! ```
//!
//! The HTML report lands in `target/criterion/report/index.html` (the
//! `html_reports` feature) and is what `.github/workflows/bench.yml` uploads.
//!
//! # Fixtures
//!
//! * `open/vcd_inline` and `open/fst_bytes` use checked-in fixtures: a small hand
//!   written VCD (the same shape as `tests/waveform_vcd.rs`) and the real FST
//!   dump `tests/data/io_mode_wave.fst`.
//! * Everything that needs a *large* history builds a synthetic VCD in memory
//!   (`synthetic_vcd`), so no multi megabyte dump is checked in. Two sizes are
//!   generated once per group:
//!   * `SMALL_STEPS`: ~1.7 MB and ~100 000 changes - used where the bench runs
//!     the measured call once per sample (decode, stream, value queries).
//!   * `LARGE_STEPS`: ~8.3 MB and ~500 000 changes - the scale the module docs
//!     talk about (an 8 MB dump); used by `open`, `time_range_hint` and one
//!     streaming bench, each of which is cheap per call.
//!
//! # What is measured
//!
//! The cached accessors (`decoded_changes`) are deliberately measured twice:
//! `decode/single` and `decode/batched_8` open a fresh `Waveform` per iteration
//! and pay the first decode (the slow path when the visible row set changes),
//! while `decode/cached` measures the per frame path a renderer hits on every
//! redraw. `decode/sequential_8` exists to keep the documented "one pass per
//! screen instead of one pass per row" claim honest: it decodes the same eight
//! signals one call at a time, and `decode/batched_8` does it in one
//! `decoded_changes_many` call.
//!
//! Note what that pair actually shows on a VCD: the body is parsed **eagerly**
//! on the first value query, so the extra `load_signals` calls of the sequential
//! loop only copy out of the already parsed body and the gap is small. The large
//! gap the module docs quote (30 signals: 1.04 s sequentially vs 73 ms batched)
//! is an FST property - its signal blocks are decoded lazily, one seek per
//! request - and the 1.6 KB FST fixture is far too small to reproduce it. The
//! pair is kept so the difference is measured instead of assumed.
//!
//! `criterion`'s `black_box` is deprecated, so the standard one is used.

use std::fmt::Write as _;
use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion, Throughput};
use wave_insight_lib::data_struct::ShowType;
use wave_insight_lib::waveform::{ValueQuery, Waveform};

/// The checked-in FST fixture (a real `fstWriter` dump, 1625 bytes).
const FST_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/io_mode_wave.fst");

/// The same FST fixture as bytes, so `open_bytes` can be measured too.
const FST_BYTES: &[u8] = include_bytes!("../tests/data/io_mode_wave.fst");

/// A small hand written VCD: two scopes, a 1 bit clock, an 8 bit bus with
/// `x`/`z` values and three time steps (mirrors `tests/waveform_vcd.rs`).
const VCD_FIXTURE: &str = "\
$date today $end
$version wave-insight bench fixture $end
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

/// One time step every `TICK` wave units in the synthetic dump.
const TICK: u64 = 10;

/// Time steps of the small synthetic dump (~1.7 MB, ~100 000 changes).
const SMALL_STEPS: usize = 20_000;

/// Time steps of the large synthetic dump (~8.3 MB, ~500 000 changes).
const LARGE_STEPS: usize = 100_000;

/// Number of 8 bit buses in the synthetic dump.
const BUSES: usize = 24;

/// A synthetic bus changes every `BUS_DIVISOR` time steps.
const BUS_DIVISOR: usize = 8;

/// Path of the 1 bit clock in the synthetic dump.
const SYNTH_CLOCK: &str = "top.clk";

/// Path of the 32 bit counter bus in the synthetic dump.
const SYNTH_DATA: &str = "top.data";

/// Printable VCD identifier codes. `$` (command) and whitespace are excluded so
/// a value line can never be mistaken for a command.
const IDENTIFIERS: &[u8] = b"!#%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_`abcdefghijklmnopqrstuvwxyz{|}~";

/// The `index`th identifier code.
fn identifier(index: usize) -> char {
    char::from(IDENTIFIERS[index])
}

/// Build a synthetic VCD of `steps` time steps in memory.
///
/// Every step toggles the 1 bit `top.clk` and changes the 32 bit `top.data`
/// counter; every `BUS_DIVISOR` steps the `BUSES` 8 bit `top.busNN` buses change
/// as well. That is four changes per step on average, so the dump has
/// `steps * 4` changes.
fn synthetic_vcd(steps: usize) -> String {
    let mut vcd = String::with_capacity(steps * 90 + 2048);
    vcd.push_str("$date generated by wave-insight-lib/benches/waveform.rs $end\n");
    vcd.push_str("$version synthetic criterion dump $end\n");
    vcd.push_str("$timescale 1ns $end\n");
    vcd.push_str("$scope module top $end\n");
    writeln!(&mut vcd, "$var wire 1 {} clk $end", identifier(0))
        .expect("writing to a String cannot fail");
    writeln!(&mut vcd, "$var wire 32 {} data [31:0] $end", identifier(1))
        .expect("writing to a String cannot fail");
    for bus in 0..BUSES {
        writeln!(
            &mut vcd,
            "$var wire 8 {} bus{bus:02} [7:0] $end",
            identifier(bus + 2)
        )
        .expect("writing to a String cannot fail");
    }
    vcd.push_str("$upscope $end\n$enddefinitions $end\n");

    for step in 0..steps {
        writeln!(&mut vcd, "#{}", step as u64 * TICK).expect("writing to a String cannot fail");

        //the clock toggles on every step
        vcd.push(if step % 2 == 0 { '0' } else { '1' });
        vcd.push(identifier(0));
        vcd.push('\n');

        //the 32 bit counter changes on every step
        vcd.push('b');
        for bit in (0..32).rev() {
            vcd.push(if (step >> bit) & 1 == 1 { '1' } else { '0' });
        }
        vcd.push(' ');
        vcd.push(identifier(1));
        vcd.push('\n');

        //the buses change every BUS_DIVISOR steps, one round at a time
        if step % BUS_DIVISOR == 0 {
            let round = step / BUS_DIVISOR;
            for bus in 0..BUSES {
                let value = (round + bus) as u8;
                vcd.push('b');
                for bit in (0..8).rev() {
                    vcd.push(if (value >> bit) & 1 == 1 { '1' } else { '0' });
                }
                vcd.push(' ');
                vcd.push(identifier(bus + 2));
                vcd.push('\n');
            }
        }
    }
    vcd
}

/// A synthetic dump plus the signal indexes the benches use.
struct Synthetic {
    /// The dump itself, so `iter_batched` can hand a fresh copy to the setup.
    bytes: Vec<u8>,
    /// Index of the 1 bit clock signal.
    clock: usize,
    /// Index of the 32 bit `top.data` bus.
    data: usize,
    /// Indexes of the 8 bit `top.busNN` buses.
    buses: Vec<usize>,
    /// Last time stamp of the dump, used to bound the random reads.
    end_time: u64,
}

impl Synthetic {
    /// Generate the dump and resolve the indexes the benches need.
    fn new(steps: usize) -> Self {
        let bytes = synthetic_vcd(steps).into_bytes();
        let waveform =
            Waveform::open_bytes(bytes.clone()).expect("the synthetic dump is valid VCD");
        let index_of = |path: &str| {
            waveform
                .signal_by_path(path)
                .unwrap_or_else(|| panic!("`{path}` should be in the synthetic dump"))
                .index()
        };
        let clock = index_of(SYNTH_CLOCK);
        let data = index_of(SYNTH_DATA);
        let buses: Vec<usize> = (0..BUSES)
            .map(|bus| index_of(&format!("top.bus{bus:02}")))
            .collect();
        let end_time = waveform
            .end_time()
            .expect("the synthetic body reads")
            .unwrap_or(0);
        Self {
            bytes,
            clock,
            data,
            buses,
            end_time,
        }
    }

    /// A fresh `Waveform` over the dump, for the first-call benches.
    fn open(&self) -> Waveform {
        Waveform::open_bytes(self.bytes.clone()).expect("the synthetic dump is valid VCD")
    }
}

/// Deterministic pseudo random time stamps: a 64 bit LCG, so runs are
/// comparable between machines and CI.
fn random_times(seed: u64, end_time: u64) -> impl FnMut() -> u64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        state % end_time
    }
}

/// `Waveform::open_bytes` / `Waveform::open_path`: header only, no body read.
fn bench_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("open");

    group.throughput(Throughput::Bytes(VCD_FIXTURE.len() as u64));
    group.bench_function("vcd_inline", |b| {
        b.iter(|| Waveform::open_bytes(VCD_FIXTURE))
    });

    let small = Synthetic::new(SMALL_STEPS);
    group.throughput(Throughput::Bytes(small.bytes.len() as u64));
    group.bench_function("vcd_synthetic_small", |b| {
        b.iter_batched(
            || small.bytes.clone(),
            Waveform::open_bytes,
            BatchSize::LargeInput,
        )
    });

    let large = Synthetic::new(LARGE_STEPS);
    group.throughput(Throughput::Bytes(large.bytes.len() as u64));
    group.bench_function("vcd_synthetic_large", |b| {
        b.iter_batched(
            || large.bytes.clone(),
            Waveform::open_bytes,
            BatchSize::LargeInput,
        )
    });

    group.throughput(Throughput::Bytes(FST_BYTES.len() as u64));
    group.bench_function("fst_bytes", |b| b.iter(|| Waveform::open_bytes(FST_BYTES)));
    group.bench_function("fst_path", |b| b.iter(|| Waveform::open_path(FST_FIXTURE)));

    group.finish();
}

/// `Waveform::time_range_hint`: the ruler on the first frame, before and after
/// the memoized answer is filled in.
fn bench_time_range_hint(c: &mut Criterion) {
    let mut group = c.benchmark_group("time_range_hint");

    //the first call scans the whole VCD body for its first and last time stamp
    let large = Synthetic::new(LARGE_STEPS);
    group.bench_function("vcd_first_call", |b| {
        b.iter_batched(
            || large.open(),
            |waveform| waveform.time_range_hint(),
            BatchSize::LargeInput,
        )
    });

    //the memoized answer: this is what a redraw pays
    let cached = large.open();
    cached
        .time_range_hint()
        .expect("the synthetic VCD has a time range");
    group.bench_function("vcd_cached", |b| b.iter(|| cached.time_range_hint()));

    //FST carries the time table in its header, so the first call opens the body
    group.bench_function("fst_first_call", |b| {
        b.iter_batched(
            || Waveform::open_path(FST_FIXTURE).expect("the FST fixture opens"),
            |waveform| waveform.time_range_hint(),
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

/// `Waveform::decoded_changes` / `decoded_changes_many`: the per signal cache
/// that a renderer draws from.
fn bench_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("decode");
    let small = Synthetic::new(SMALL_STEPS);

    //first decode of one row: reads the body and converts the history
    group.bench_function("single", |b| {
        b.iter_batched(
            || small.open(),
            |waveform| waveform.decoded_changes(small.clock),
            BatchSize::LargeInput,
        )
    });

    //first decode of eight rows in one pass: what a screen of rows should do
    let eight: Vec<usize> = small.buses[..8].to_vec();
    group.bench_function("batched_8", |b| {
        b.iter_batched(
            || small.open(),
            |waveform| waveform.decoded_changes_many(&eight),
            BatchSize::LargeInput,
        )
    });

    //the same eight rows decoded one call at a time (the pattern the docs warn
    //about); keep it honest next to `batched_8`
    group.bench_function("sequential_8", |b| {
        b.iter_batched(
            || small.open(),
            |waveform| {
                for &signal in &eight {
                    black_box(
                        waveform
                            .decoded_changes(signal)
                            .expect("the synthetic signal decodes"),
                    );
                }
            },
            BatchSize::LargeInput,
        )
    });

    //the per frame path: the signal is already decoded, this only clones a handle
    let warm = small.open();
    warm.decoded_changes(small.clock)
        .expect("the synthetic clock decodes");
    group.bench_function("cached", |b| b.iter(|| warm.decoded_changes(small.clock)));

    group.finish();
}

/// `Waveform::value_at`: the cursor readout, from the packed decode and from an
/// already decoded history.
fn bench_value_at(c: &mut Criterion) {
    let mut group = c.benchmark_group("value_at");
    let small = Synthetic::new(SMALL_STEPS);

    //packed path: the body is loaded, the signal is not decoded into a history
    let packed = small.open();
    packed
        .value_at(small.data, 0)
        .expect("the synthetic bus has values");
    let mut packed_times = random_times(0x1234_5678_9abc_def0, small.end_time);
    group.bench_function("packed", |b| {
        b.iter(|| {
            let time = packed_times();
            black_box(packed.value_at(small.data, time))
        })
    });

    //decoded path: a renderer already asked for the history
    let decoded = small.open();
    decoded
        .decoded_changes(small.data)
        .expect("the synthetic bus decodes");
    let mut decoded_times = random_times(0x1234_5678_9abc_def0, small.end_time);
    group.bench_function("decoded", |b| {
        b.iter(|| {
            let time = decoded_times();
            black_box(decoded.value_at(small.data, time))
        })
    });

    group.finish();
}

/// `Waveform::changes_in_window` / `Waveform::stream_changes`: the streaming
/// path that re-reads the body on every call.
fn bench_window(c: &mut Criterion) {
    let mut group = c.benchmark_group("window");
    //every call re-parses the dump; ten samples is enough to see the cost and
    //keeps the whole suite in the seconds range
    group.sample_size(10);

    let small = Synthetic::new(SMALL_STEPS);
    let waveform = small.open();
    let start = small.end_time / 4;
    let end = small.end_time / 2;

    group.bench_function("changes_in_window", |b| {
        b.iter(|| waveform.changes_in_window(small.clock, start, Some(end)))
    });

    //a screen of rows streamed in one pass, counting instead of collecting
    let query = ValueQuery::for_signals(small.buses.iter().copied()).between(start, Some(end));
    group.bench_function("stream_changes_group", |b| {
        b.iter(|| {
            let mut changes = 0usize;
            waveform
                .stream_changes(&query, |_, _, _| {
                    changes += 1;
                    true
                })
                .expect("the synthetic dump streams");
            black_box(changes)
        })
    });

    //the same window on the 8 MB dump: this is the path that must not be called
    //once per frame
    let large = Synthetic::new(LARGE_STEPS);
    let large_waveform = large.open();
    let large_start = large.end_time / 4;
    let large_end = large.end_time / 2;
    group.bench_function("changes_in_window_large", |b| {
        b.iter(|| large_waveform.changes_in_window(large.clock, large_start, Some(large_end)))
    });

    group.finish();
}

/// Formatting values for the view: `WaveValue::render` (the public seam onto
/// the `waveform::render` helpers, which are crate private).
fn bench_render(c: &mut Criterion) {
    let mut group = c.benchmark_group("render");
    let small = Synthetic::new(SMALL_STEPS);
    let waveform = small.open();

    //one 32 bit value: hex is the default view, bin the widest text
    let decoded = waveform
        .decoded_changes(small.data)
        .expect("the synthetic bus decodes");
    let value = decoded
        .first_change()
        .map(|(_, value)| value)
        .expect("the synthetic bus changes at least once");
    group.bench_function("hex_32bit", |b| b.iter(|| value.render(32, &ShowType::Hex)));
    group.bench_function("bin_32bit", |b| b.iter(|| value.render(32, &ShowType::Bin)));

    //a whole row: every change of an 8 bit bus formatted, like a redraw does
    let bus = waveform
        .decoded_changes(small.buses[0])
        .expect("the synthetic bus decodes");
    group.bench_function("row_hex_8bit", |b| {
        b.iter(|| {
            let mut width = 0usize;
            for (_, value) in bus.iter() {
                width += value.render(8, &ShowType::Hex).map_or(0, |text| text.len());
            }
            black_box(width)
        })
    });

    group.finish();
}

criterion_group!(
    waveform_benches,
    bench_open,
    bench_time_range_hint,
    bench_decode,
    bench_value_at,
    bench_window,
    bench_render
);
criterion_main!(waveform_benches);
