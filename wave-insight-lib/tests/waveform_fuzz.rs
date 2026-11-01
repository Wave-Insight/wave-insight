//! Deterministic seeded fuzz-style and differential tests for the waveform
//! backend.
//!
//! Three groups:
//! 1. a generator of valid random VCDs with a hand-built model: every query
//!    (all_changes, decoded_changes, value_at, changes_in_window, the memoized
//!    time range) must agree with the model;
//! 2. differential checks of the query tiers against each other on random
//!    windows and on the real FST fixture;
//! 3. corruption: deterministic truncations and byte flips of valid VCD/FST
//!    bytes must never panic - every entry point has to answer with a value or
//!    a WaveformError.

use wave_insight_lib::waveform::{TimeRange, ValueQuery, WaveValue, Waveform};

/// Small deterministic LCG so failures are reproducible from the seed.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1))
    }

    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next() % bound
        }
    }
}

/// One signal of the generated model: width and (time, bit string) changes.
struct ModelSignal {
    path: String,
    width: usize,
    changes: Vec<(u64, String)>,
}

fn random_bits(rng: &mut Lcg, width: usize) -> String {
    (0..width)
        .map(|_| match rng.below(10) {
            0 => 'x',
            1 => 'z',
            value if value < 5 => '0',
            _ => '1',
        })
        .collect()
}

/// Build a valid VCD plus the expected model, using only positive incrementing
/// time stamps and always changing a signal's value (wellen may elide a change
/// that repeats the previous value).
fn random_vcd(seed: u64) -> (String, Vec<ModelSignal>, u64) {
    let mut rng = Lcg::new(seed);
    let signal_count = 3 + rng.below(5) as usize;
    let step_count = 3 + rng.below(8) as usize;
    let identifiers: Vec<char> = "!#%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        .chars()
        .collect();

    let mut vcd =
        String::from("$date wave-insight fuzz $end\n$timescale 1ns $end\n$scope module top $end\n");
    let mut model = Vec::new();
    for (index, identifier) in identifiers.iter().enumerate().take(signal_count) {
        let width = 1 + rng.below(12) as usize;
        let path = format!("top.s{index}");
        vcd.push_str(&format!("$var wire {width} {} s{index} $end\n", identifier));
        model.push(ModelSignal {
            path,
            width,
            changes: Vec::new(),
        });
    }
    vcd.push_str("$upscope $end\n$enddefinitions $end\n");

    let mut time = 0u64;
    for step in 0..step_count {
        if step > 0 {
            time += 1 + rng.below(9);
        }
        vcd.push_str(&format!("#{time}\n"));
        for signal_index in 0..model.len() {
            let signal = &mut model[signal_index];
            let mut value = random_bits(&mut rng, signal.width);
            // wellen may elide a change whose value repeats the previous one
            if signal.changes.last().map(|(_, last)| last) == Some(&value) {
                let mut characters = value.chars();
                let first = characters.next();
                let rest = characters.as_str();
                value = format!("{}{rest}", if first == Some('0') { '1' } else { '0' });
            }
            vcd.push('b');
            vcd.push_str(&value);
            vcd.push(' ');
            vcd.push(identifiers[signal_index]);
            vcd.push('\n');
            signal.changes.push((time, value));
        }
    }
    (vcd, model, time)
}

fn bits_of(value: &WaveValue) -> Option<String> {
    value.to_bit_string()
}

#[test]
fn random_valid_vcds_match_the_model_on_every_query() {
    for seed in 0..12u64 {
        let (vcd, model, end_time) = random_vcd(seed);
        let waveform = Waveform::open_bytes(vcd.into_bytes())
            .unwrap_or_else(|error| panic!("seed {seed}: generated VCD must open: {error}"));
        assert_eq!(waveform.signal_count(), model.len(), "seed {seed}");
        assert_eq!(
            waveform.time_range_hint().expect("hint"),
            Some(TimeRange {
                start: 0,
                end: end_time
            }),
            "seed {seed}"
        );
        assert_eq!(waveform.start_time().expect("start"), Some(0));
        assert_eq!(waveform.end_time().expect("end"), Some(end_time));

        let mut rng = Lcg::new(seed.wrapping_add(0x5eed));
        for expected in &model {
            let signal = waveform
                .signal_by_path(&expected.path)
                .unwrap_or_else(|| panic!("seed {seed}: missing {}", expected.path));
            assert_eq!(signal.width() as usize, expected.width, "seed {seed}");

            let changes = waveform.all_changes(signal.index()).expect("all_changes");
            let seen: Vec<(u64, String)> = changes
                .iter()
                .map(|(time, value)| (*time, bits_of(value).expect("bit vector")))
                .collect();
            assert_eq!(seen, expected.changes, "seed {seed} {}", expected.path);

            let decoded = waveform
                .decoded_changes(signal.index())
                .expect("decoded_changes");
            assert_eq!(&decoded[..], &changes[..], "seed {seed} {}", expected.path);

            // value_at against the model's binary search
            for _ in 0..8 {
                let time = rng.below(end_time + 10);
                let model_value = expected
                    .changes
                    .iter()
                    .rev()
                    .find(|(change_time, _)| *change_time <= time)
                    .map(|(_, value)| value.clone());
                let actual = waveform
                    .value_at(signal.index(), time)
                    .expect("value_at")
                    .map(|value| bits_of(&value).expect("bit vector"));
                assert_eq!(
                    actual, model_value,
                    "seed {seed} {} at {time}",
                    expected.path
                );
            }

            // changes_in_window against the model
            for _ in 0..5 {
                let start = rng.below(end_time + 1);
                let end = start + rng.below(end_time - start + 1);
                let window = waveform
                    .changes_in_window(signal.index(), start, Some(end))
                    .expect("window");
                let expected_window: Vec<(u64, String)> = expected
                    .changes
                    .iter()
                    .filter(|(time, _)| *time >= start && *time <= end)
                    .cloned()
                    .collect();
                let seen: Vec<(u64, String)> = window
                    .iter()
                    .map(|(time, value)| (*time, bits_of(value).expect("bit vector")))
                    .collect();
                assert_eq!(
                    seen, expected_window,
                    "seed {seed} {} {start}..={end}",
                    expected.path
                );
            }
        }
    }
}

#[test]
fn query_tiers_agree_on_random_windows_and_the_fst_fixture() {
    for seed in 0..8u64 {
        let (vcd, model, end_time) = random_vcd(seed);
        let waveform = Waveform::open_bytes(vcd.into_bytes()).expect("generated VCD opens");
        let mut rng = Lcg::new(seed ^ 0xdead_beef);
        let indices: Vec<usize> = waveform
            .signals()
            .iter()
            .map(|signal| signal.index())
            .collect();

        // batched decode == per-signal decode, in argument order (including
        // duplicates)
        let batched = waveform.decoded_changes_many(&indices).expect("batched");
        for (position, index) in indices.iter().enumerate() {
            assert_eq!(
                &batched[position][..],
                &waveform.decoded_changes(*index).expect("single")[..]
            );
        }
        let duplicated = waveform
            .decoded_changes_many(&[indices[0], indices[0]])
            .expect("duplicates");
        assert!(duplicated[0].shares_storage_with(&duplicated[1]));

        for expected in &model {
            let signal = waveform.signal_by_path(&expected.path).expect("signal");
            let decoded = waveform.decoded_changes(signal.index()).expect("decoded");
            for _ in 0..8 {
                let start = rng.below(end_time + 2);
                let remaining = (end_time + 2).saturating_sub(start).max(1);
                let end = start + rng.below(remaining);
                let streamed = waveform
                    .changes_in_window(signal.index(), start, Some(end))
                    .expect("streamed");
                assert_eq!(streamed, decoded.window(start, Some(end)).to_vec());
                // value_at on both tiers agrees
                assert_eq!(
                    waveform.value_at(signal.index(), start).expect("value_at"),
                    decoded.value_at(start).cloned()
                );
            }
        }
    }

    // The real FST fixture: same differential checks.
    let fst = Waveform::open_path(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/io_mode_wave.fst"
    ))
    .expect("FST fixture opens");
    let mut rng = Lcg::new(0xf57);
    let end = fst.end_time().expect("end").expect("non-empty");
    for signal in fst.signals() {
        let decoded = fst.decoded_changes(signal.index()).expect("decoded");
        for _ in 0..4 {
            let start = rng.below(end + 1);
            let finish = start + rng.below(end - start + 1);
            let streamed = fst
                .changes_in_window(signal.index(), start, Some(finish))
                .expect("streamed");
            assert_eq!(streamed, decoded.window(start, Some(finish)).to_vec());
        }
    }
}

/// Exercise every public query on a possibly corrupt file: the calls may return
/// errors, but none of them may panic.
fn exercise(waveform: &Waveform) {
    let _ = waveform.time_range_hint();
    let _ = waveform.start_time();
    let _ = waveform.end_time();
    let _ = waveform.cache_stats();
    let indices: Vec<usize> = waveform
        .signals()
        .iter()
        .map(|signal| signal.index())
        .collect();
    let _ = waveform.decoded_changes_many(&indices);
    for &index in indices.iter().take(8) {
        let _ = waveform.all_changes(index);
        let _ = waveform.decoded_changes(index);
        let _ = waveform.value_at(index, 0);
        let _ = waveform.changes_in_window(index, 0, Some(100));
        let _ = waveform.unload_signal(index);
    }
    let _ = waveform.stream_changes(&ValueQuery::all().with_max_changes(64), |_, _, _| true);
}

fn fst_fixture_bytes() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/io_mode_wave.fst"
    ))
    .expect("FST fixture readable")
}

#[test]
fn truncated_and_byte_flipped_vcds_never_panic() {
    let (vcd, _, _) = random_vcd(99);
    let bytes = vcd.into_bytes();
    let mut rng = Lcg::new(0xc0ffee);

    // every truncation length of a valid dump
    for length in 0..bytes.len() {
        let corrupted = bytes[..length].to_vec();
        if let Ok(waveform) = Waveform::open_bytes(corrupted) {
            exercise(&waveform);
        }
    }

    // deterministic single byte flips and splices
    for _ in 0..256 {
        let mut corrupted = bytes.clone();
        let position = rng.below(corrupted.len() as u64) as usize;
        corrupted[position] ^= (1 + rng.below(255)) as u8;
        if let Ok(waveform) = Waveform::open_bytes(corrupted) {
            exercise(&waveform);
        }
    }
    for _ in 0..64 {
        let mut corrupted = bytes.clone();
        let position = rng.below(corrupted.len() as u64) as usize;
        corrupted.remove(position);
        if let Ok(waveform) = Waveform::open_bytes(corrupted) {
            exercise(&waveform);
        }
    }
}

#[test]
fn truncated_and_byte_flipped_fsts_never_panic() {
    let bytes = fst_fixture_bytes();
    let mut rng = Lcg::new(0xf57c0de);
    for length in 0..bytes.len() {
        if let Ok(waveform) = Waveform::open_bytes(bytes[..length].to_vec()) {
            exercise(&waveform);
        }
    }
    for _ in 0..256 {
        let mut corrupted = bytes.clone();
        let position = rng.below(corrupted.len() as u64) as usize;
        corrupted[position] ^= (1 + rng.below(255)) as u8;
        if let Ok(waveform) = Waveform::open_bytes(corrupted) {
            exercise(&waveform);
        }
    }
}
/// Two VCD variables can share one identifier code (an alias of the same
/// signal).  The value stream must report a change against the variable that
/// was requested, not always against the first one.
const ALIASED_VCD: &str = "\
$timescale 1ns $end
$scope module top $end
$var wire 8 ! data [7:0] $end
$var wire 8 ! alias [7:0] $end
$upscope $end
$enddefinitions $end
#0
b00000000 !
#10
b10101010 !
#20
b00001111 !
";

#[test]
fn aliased_stream_reports_the_requested_variable_index() {
    let waveform = Waveform::open_bytes(ALIASED_VCD.as_bytes().to_vec()).expect("alias VCD opens");
    assert_eq!(waveform.signal_count(), 2);
    let data = waveform.signal_by_path("top.data").expect("data").index();
    let alias = waveform.signal_by_path("top.alias").expect("alias").index();
    assert_ne!(data, alias);

    // Both variables see the same history.
    let data_changes = waveform.all_changes(data).expect("data history");
    let alias_changes = waveform.all_changes(alias).expect("alias history");
    assert_eq!(data_changes, alias_changes);

    for requested in [data, alias] {
        let mut seen = Vec::new();
        waveform
            .stream_changes(&ValueQuery::for_signal(requested), |signal, time, value| {
                seen.push((signal, time, value.to_bit_string()));
                true
            })
            .expect("stream");
        assert_eq!(seen.len(), 3, "one change per time step");
        assert!(
            seen.iter().all(|(signal, _, _)| *signal == requested),
            "a query for variable {requested} reported {:?}",
            seen.iter()
                .map(|(signal, _, _)| *signal)
                .collect::<Vec<_>>()
        );
    }

    // Querying every variable reports each alias under its own index.
    let mut indices = Vec::new();
    waveform
        .stream_changes(&ValueQuery::all(), |signal, _, _| {
            indices.push(signal);
            true
        })
        .expect("stream all");
    indices.sort_unstable();
    indices.dedup();
    assert_eq!(indices, vec![data.min(alias), data.max(alias)]);

    // The cached decode path still shares the underlying signal.
    let batched = waveform
        .decoded_changes_many(&[data, alias])
        .expect("batched decode");
    assert_eq!(&batched[0][..], &batched[1][..]);
}
