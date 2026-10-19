//! Tests for the new [`wave_insight_lib::waveform`] backend on a real FST file.
//!
//! # Fixture
//!
//! `tests/data/io_mode_wave.fst` is a genuine FST dump written by `fstWriter`
//! (SpinalHDL simulation of a SPI master, 1625 bytes, sha256
//! `814bca2fd2b0850acecfabaaae64d4b9e6e71d0b2d809d632736d2c3eb2376b4`). It was
//! found on the development machine - there is no pure Rust FST writer on
//! crates.io, so a *real* writer was used instead of producing a file through
//! some other format.
//!
//! What it contains: two scope levels (`TOP`, `TOP.SpiMasterCtrlContinuous`),
//! 1/8/9/32 bit wires, logic and parameter variables, declared index ranges,
//! 245 time steps up to 13800000 with a 1 ps timescale, and 2-state values
//! only. `x`/`z` coverage for the FST path therefore comes from the VCD tests
//! (the reader path is the same wellen code for both formats, but the fixture
//! has no unknown bits).

use wave_insight_lib::data_struct::{BoolData, ShowType};
use wave_insight_lib::waveform::{
    SignalKind, TimeUnit, ValueQuery, WaveFormat, WaveValue, Waveform,
};

const FIXTURE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/io_mode_wave.fst");
const FIXTURE_BYTES: &[u8] = include_bytes!("data/io_mode_wave.fst");

fn open() -> Waveform {
    Waveform::open_path(FIXTURE_PATH).expect("the FST fixture should open")
}

fn bits_of(value: &WaveValue) -> String {
    value
        .to_bit_string()
        .expect("the fixture only contains bit vectors")
}

#[test]
fn detects_fst_and_its_metadata() {
    let waveform = open();
    assert_eq!(waveform.file_format(), WaveFormat::Fst);
    assert_eq!(waveform.date(), "Fri Dec 27 10:44:19 2024");
    assert_eq!(waveform.version(), "fstWriter");
    assert_eq!(waveform.path().map(|path| path.ends_with("io_mode_wave.fst")), Some(true));

    let timescale = waveform.timescale().expect("FST stores a timescale");
    assert_eq!(timescale.factor, 1);
    assert_eq!(timescale.unit, TimeUnit::PicoSeconds);
    assert_eq!(timescale.to_string(), "1 ps");

    assert_eq!(waveform.start_time().unwrap(), Some(0));
    assert_eq!(waveform.end_time().unwrap(), Some(13_800_000));
}

#[test]
fn hierarchy_is_resolved_from_the_fst_header() {
    let waveform = open();
    let hierarchy = waveform.hierarchy();
    let root = hierarchy.root();

    let top_level: Vec<_> = root
        .children()
        .iter()
        .map(|scope| (scope.name(), scope.signals().len()))
        .collect();
    assert_eq!(top_level, vec![("TOP", 19)]);

    let top = root.child("TOP").expect("`TOP` scope");
    let sub = top.child("SpiMasterCtrlContinuous").expect("sub scope");
    assert_eq!(sub.path(), "TOP.SpiMasterCtrlContinuous");
    assert_eq!(sub.signals().len(), 41);
    assert_eq!(waveform.signal_count(), 60);

    //variables of the sub scope are addressable by their full path
    assert!(waveform.signal_by_path("TOP.SpiMasterCtrlContinuous.clk").is_some());
    assert!(waveform.signal_by_path("TOP.clk").is_some());
    //the same local name can exist in different scopes
    let a = waveform.signal_by_path("TOP.clk").unwrap();
    let b = waveform.signal_by_path("TOP.SpiMasterCtrlContinuous.clk").unwrap();
    assert_ne!(a.index(), b.index());
}

#[test]
fn signal_widths_ranges_and_variable_types() {
    let waveform = open();

    let clk = waveform.signal_by_path("TOP.clk").unwrap();
    assert_eq!(clk.width(), 1);
    assert_eq!(clk.kind(), SignalKind::BitVector);
    assert_eq!(clk.index_range(), None);
    assert_eq!(clk.var_type(), "Wire");

    let sclk_toggle = waveform.signal_by_path("TOP.io_config_sclkToggle").unwrap();
    assert_eq!(sclk_toggle.width(), 8);
    assert_eq!(sclk_toggle.index_range(), Some((7, 0)));
    assert_eq!(sclk_toggle.range_string().as_deref(), Some("[7:0]"));

    let args = waveform.signal_by_path("TOP.io_cmd_payload_args").unwrap();
    assert_eq!(args.width(), 9);
    assert_eq!(args.index_range(), Some((8, 0)));

    let mode = waveform.signal_by_path("TOP.io_cmd_payload_mode").unwrap();
    assert_eq!(mode.index_range(), Some((0, 0)));

    let timer = waveform.signal_by_path("TOP.SpiMasterCtrlContinuous.timer_counter").unwrap();
    assert_eq!(timer.width(), 8);
    assert_eq!(timer.index_range(), Some((7, 0)));
    assert_eq!(timer.var_type(), "Logic");

    let mode_string = waveform
        .signal_by_path("TOP.SpiMasterCtrlContinuous.io_cmd_payload_mode_string")
        .unwrap();
    assert_eq!(mode_string.width(), 32);
    assert_eq!(mode_string.index_range(), Some((31, 0)));

    let parameter = waveform
        .signal_by_path("TOP.SpiMasterCtrlContinuous.SpiMasterCtrlCmdMode_DATA")
        .unwrap();
    assert_eq!(parameter.var_type(), "Parameter");
    assert_eq!(parameter.index_range(), Some((0, 0)));
}

#[test]
fn values_are_decoded_from_the_fst_body() {
    let waveform = open();

    let cmd_valid = waveform
        .signal_by_path("TOP.io_cmd_valid")
        .expect("`TOP.io_cmd_valid`");
    let changes = waveform.all_changes(cmd_valid.index()).unwrap();
    assert_eq!(changes.len(), 3);
    assert_eq!(changes[0].0, 0);
    assert_eq!(bits_of(&changes[0].1), "0");
    assert_eq!(bits_of(&changes[1].1), "1");
    assert_eq!(changes[1].0, 2_600_000);
    assert_eq!(changes[2].0, 12_800_000);
    assert_eq!(bits_of(&changes[2].1), "0");

    //an eight bit signal, seen as bits and as bits plus numbers
    let spi_ss = waveform.signal_by_path("TOP.io_spi_ss").unwrap();
    let spi_ss_changes = waveform.all_changes(spi_ss.index()).unwrap();
    let times: Vec<_> = spi_ss_changes.iter().map(|(time, _)| *time).collect();
    assert_eq!(times, vec![0, 2_700_000, 12_700_000]);
    assert_eq!(bits_of(&spi_ss_changes[0].1), "11111111");
    assert_eq!(spi_ss_changes[1].1.to_u64(), Some(0xfe));
    assert_eq!(bits_of(&spi_ss_changes[2].1), "11111111");

    //a 32 bit value spelled like ASCII "DATA"
    let mode_string = waveform
        .signal_by_path("TOP.SpiMasterCtrlContinuous.io_cmd_payload_mode_string")
        .unwrap();
    let mode_changes = waveform.all_changes(mode_string.index()).unwrap();
    assert_eq!(mode_changes.len(), 4);
    assert_eq!(mode_changes[0].1.to_u64(), Some(0x4441_5441));
    assert_eq!(mode_changes[0].0, 0);
    assert_eq!(mode_changes[1].0, 2_600_000);

    //an eight bit counter that changes with every timer tick
    let timer = waveform
        .signal_by_path("TOP.SpiMasterCtrlContinuous.timer_counter")
        .unwrap();
    let timer_changes = waveform.all_changes(timer.index()).unwrap();
    assert_eq!(timer_changes.len(), 103);
    assert_eq!(timer_changes[0].1.to_u64(), Some(0));
    assert_eq!(timer_changes[1].1.to_u64(), Some(1));
    assert_eq!(timer_changes[1].0, 2_700_000);

    //a signal that only changes once
    let sclk_toggle = waveform.signal_by_path("TOP.io_config_sclkToggle").unwrap();
    let sclk_changes = waveform.all_changes(sclk_toggle.index()).unwrap();
    assert_eq!(sclk_changes.len(), 1);
    assert_eq!(bits_of(&sclk_changes[0].1), "00000001");
}

#[test]
fn value_at_and_window_queries() {
    let waveform = open();
    let clk = waveform.signal_by_path("TOP.clk").unwrap().index();

    //the clock toggles every 50000 ticks once the reset is released
    let bits_at = |time| waveform.value_at(clk, time).unwrap().map(|value| bits_of(&value));
    assert_eq!(bits_at(0).as_deref(), Some("0"));
    assert_eq!(bits_at(1_699_999).as_deref(), Some("0"));
    assert_eq!(bits_at(1_700_000).as_deref(), Some("1"));
    assert_eq!(bits_at(1_750_000).as_deref(), Some("0"));
    assert!(bits_at(13_800_000).is_some(), "the clock keeps a value to the end");

    let window = waveform
        .changes_in_window(clk, 1_700_000, Some(1_800_000))
        .unwrap();
    let seen: Vec<_> = window
        .iter()
        .map(|(time, value)| (*time, bits_of(value)))
        .collect();
    assert_eq!(
        seen,
        vec![
            (1_700_000, "1".to_string()),
            (1_750_000, "0".to_string()),
            (1_800_000, "1".to_string())
        ]
    );

    //nothing happens in a quiet window
    let quiet = waveform
        .changes_in_window(clk, 100, Some(1000))
        .unwrap();
    assert!(quiet.is_empty());
}

#[test]
fn streaming_reports_only_the_requested_signals() {
    let waveform = open();
    let clk = waveform.signal_by_path("TOP.clk").unwrap().index();
    let ss = waveform.signal_by_path("TOP.io_spi_ss").unwrap().index();

    let mut changes = Vec::new();
    waveform
        .stream_changes(&ValueQuery::for_signals([clk, ss]), |signal, time, value| {
            changes.push((signal, time, bits_of(&value)));
            true
        })
        .unwrap();

    assert!(changes.iter().all(|(signal, _, _)| *signal == clk || *signal == ss));
    //the clock changes in between the two `io_spi_ss` changes
    assert!(changes.iter().all(|(_, time, _)| *time <= 13_800_000));
    let ss_times: Vec<_> = changes
        .iter()
        .filter(|(signal, _, _)| *signal == ss)
        .map(|(_, time, _)| *time)
        .collect();
    assert_eq!(ss_times, vec![0, 2_700_000, 12_700_000]);

    //`max_changes` bounds the amount of work
    let mut count = 0usize;
    waveform
        .stream_changes(&ValueQuery::all().with_max_changes(7), |_, _, _| {
            count += 1;
            true
        })
        .unwrap();
    assert_eq!(count, 7);
}

#[test]
fn bytes_and_path_sources_agree() {
    let from_path = open();
    let from_bytes = Waveform::open_bytes(FIXTURE_BYTES).expect("FST from bytes should open");

    assert_eq!(from_path.signal_count(), from_bytes.signal_count());
    assert_eq!(from_path.end_time().unwrap(), from_bytes.end_time().unwrap());

    let clk = from_path.signal_by_path("TOP.clk").unwrap().index();
    assert_eq!(
        from_path.all_changes(clk).unwrap(),
        from_bytes
            .all_changes(from_bytes.signal_by_path("TOP.clk").unwrap().index())
            .unwrap()
    );

    //the in-memory copy does not remember a path
    assert!(from_bytes.path().is_none());
    assert_eq!(from_bytes.source().memory_len(), Some(FIXTURE_BYTES.len()));
}

#[test]
fn legacy_adapter_works_on_fst() {
    let waveform = open();
    let (module, values) = waveform.to_legacy().expect("legacy conversion");

    assert_eq!(module.end_clock, 13_800_000);
    let top = module.sub_module.get("TOP").expect("`TOP` module");
    assert!(top.sub_module.contains_key("SpiMasterCtrlContinuous"));

    let clk = &top.signal["clk"];
    assert_eq!(clk.size, 1);
    assert_eq!(clk.value_key, "TOP.clk");

    let clk_values = values.get_bool("TOP.clk");
    assert_eq!(clk_values.len(), 243);
    assert_eq!(clk_values[0], (0, BoolData::Zero));
    assert_eq!(clk_values[1], (1_700_000, BoolData::One));

    let ss = values.get_bits("TOP.io_spi_ss");
    assert_eq!(ss.len(), 3);
    assert_eq!(ss[0].1.to_string(8, &ShowType::Hex), "ff");
    assert_eq!(ss[1].1.to_string(8, &ShowType::Hex), "fe");
    assert_eq!(ss[2].1.to_string(8, &ShowType::Hex), "ff");

    //a width that is not a multiple of eight: the old layout has to keep the
    //padding bits in the first byte and the display has to drop them
    let args = values.get_bits("TOP.io_cmd_payload_args");
    assert_eq!(args[0].1.to_string(9, &ShowType::Bin), "110111001");
    assert_eq!(args[0].1.to_string(9, &ShowType::Hex), "1b9");
    assert_eq!(args[0].1.to_string(9, &ShowType::UInt), "441");
    assert_eq!(args[1].1.to_string(9, &ShowType::UInt), "1");

    //an unknown key behaves like the old model: empty, no panic
    assert!(values.get_bool("TOP.does_not_exist").is_empty());
    assert!(values.get_bits("TOP.does_not_exist").is_empty());
}

#[test]
fn unloading_a_signal_frees_the_cache_but_keeps_the_values() {
    let waveform = open();
    let clk = waveform.signal_by_path("TOP.clk").unwrap().index();

    let first = waveform.all_changes(clk).unwrap();
    waveform.unload_signal(clk).unwrap();
    //decoding again gives the same result, so the cache was rebuilt
    let second = waveform.all_changes(clk).unwrap();
    assert_eq!(first, second);
    assert_eq!(waveform.value_at(clk, 1_700_000).unwrap().map(|v| bits_of(&v)), Some("1".to_string()));
}
