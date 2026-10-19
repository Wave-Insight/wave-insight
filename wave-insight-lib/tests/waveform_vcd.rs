//! End to end tests for the new [`wave_insight_lib::waveform`] backend on VCD.
//!
//! The fixtures are hand written so that every assertion can be traced back to
//! a line of the dump: three scope levels, widths of 1/4/8/16 bits, `x`/`z`
//! values, non uniform time stamps and a value that does *not* change in the
//! last time step.

use wave_insight_lib::data_struct::{BoolData, Module, ShowType};
use wave_insight_lib::parser::vcd_parser::vcd_parser;
use wave_insight_lib::waveform::{
    BitValue, SignalKind, TimeUnit, ValueQuery, WaveFormat, WaveValue, Waveform,
};

///`top` contains `clk`, `data` and `nibble`, `top.sub` contains `flag` and
///`top.sub.u_cpu` contains `pc`.
const VCD: &str = "\
$date today $end
$version wave-insight waveform test $end
$timescale 1ns $end
$scope module top $end
$var wire 1 ! clk $end
$var wire 8 \" data [7:0] $end
$var reg 4 # nibble [3:0] $end
$scope module sub $end
$var wire 1 & flag $end
$scope module u_cpu $end
$var wire 16 ' pc [15:0] $end
$upscope $end
$upscope $end
$upscope $end
$enddefinitions $end
#0
0!
b00000000 \"
b0000 #
0&
b0000000000000000 '
#10
1!
b10101010 \"
b10xz #
1&
b0000000000000101 '
#20
z!
bxxxxxxxx \"
bzx01 #
0&
";

fn open() -> Waveform {
    Waveform::open_bytes(VCD).expect("the fixture is valid VCD")
}

///First change of `signal` as a bit string.
fn first_bits(waveform: &Waveform, path: &str) -> String {
    let signal = waveform
        .signal_by_path(path)
        .unwrap_or_else(|| panic!("signal `{path}` should be in the hierarchy"));
    let changes = waveform
        .all_changes(signal.index())
        .expect("values should decode");
    changes
        .first()
        .and_then(|(_, value)| value.to_bit_string())
        .unwrap_or_else(|| panic!("signal `{path}` should have at least one change"))
}

#[test]
fn header_metadata_is_available() {
    let waveform = open();
    assert_eq!(waveform.file_format(), WaveFormat::Vcd);
    assert_eq!(waveform.date(), "today");
    assert_eq!(waveform.version(), "wave-insight waveform test");

    let timescale = waveform.timescale().expect("`$timescale 1ns` is declared");
    assert_eq!(timescale.factor, 1);
    assert_eq!(timescale.unit, TimeUnit::NanoSeconds);
    assert_eq!(timescale.to_string(), "1 ns");
    assert_eq!(waveform.start_time().unwrap(), Some(0));
    assert_eq!(waveform.end_time().unwrap(), Some(20));
}

#[test]
fn hierarchy_has_three_scope_levels() {
    let waveform = open();
    let hierarchy = waveform.hierarchy();
    let root = hierarchy.root();

    //the virtual root holds the top level scopes
    let top_names: Vec<_> = root.children().iter().map(|scope| scope.name()).collect();
    assert_eq!(top_names, vec!["top"]);

    let top = root.child("top").expect("`top` scope");
    let mut top_signals: Vec<_> = top
        .signals()
        .iter()
        .map(|&index| hierarchy.signal(index).unwrap().name())
        .collect();
    top_signals.sort_unstable();
    assert_eq!(top_signals, vec!["clk", "data", "nibble"]);

    let sub = top.child("sub").expect("`top.sub` scope");
    assert_eq!(sub.path(), "top.sub");
    let flag = hierarchy.signal(sub.signals()[0]).unwrap();
    assert_eq!(flag.name(), "flag");
    assert_eq!(flag.path(), "top.sub.flag");

    let u_cpu = sub.child("u_cpu").expect("`top.sub.u_cpu` scope");
    assert_eq!(u_cpu.path(), "top.sub.u_cpu");
    let pc = hierarchy.signal(u_cpu.signals()[0]).unwrap();
    assert_eq!(pc.path(), "top.sub.u_cpu.pc");
    assert_eq!(pc.scope(), ["top", "sub", "u_cpu"]);

    //the tree and the flat list agree
    assert_eq!(hierarchy.len(), 5);
    assert_eq!(root.total_signals(), 5);
    assert!(root.find("top.sub.u_cpu").is_some());
    assert!(root.find("top.nope").is_none());
}

#[test]
fn waveform_handle_is_send_and_sync() {
    //the UI wants to keep this in a `tauri::State`
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Waveform>();
}

#[test]
fn signals_expose_width_kind_and_index_range() {
    let waveform = open();

    let clk = waveform.signal_by_path("top.clk").expect("`top.clk`");
    assert_eq!(clk.width(), 1);
    assert_eq!(clk.kind(), SignalKind::BitVector);
    assert!(clk.is_1bit());
    assert_eq!(clk.index_range(), None);

    let data = waveform.signal_by_path("top.data").expect("`top.data`");
    assert_eq!(data.width(), 8);
    assert_eq!(data.index_range(), Some((7, 0)));
    assert_eq!(data.range_string().as_deref(), Some("[7:0]"));

    let nibble = waveform.signal_by_path("top.nibble").expect("`top.nibble`");
    assert_eq!(nibble.width(), 4);
    assert_eq!(nibble.index_range(), Some((3, 0)));

    let pc = waveform.signal_by_path("top.sub.u_cpu.pc").expect("`pc`");
    assert_eq!(pc.width(), 16);
    assert_eq!(pc.index_range(), Some((15, 0)));

    //`$var reg` keeps its variable type
    assert_eq!(nibble.var_type(), "Reg");
    assert_eq!(data.var_type(), "Wire");
}

#[test]
fn values_keep_x_and_z_apart() {
    let waveform = open();

    let nibble = waveform.signal_by_path("top.nibble").unwrap();
    let changes = waveform.all_changes(nibble.index()).unwrap();
    assert_eq!(changes.len(), 3, "three time steps, one change each");
    assert_eq!(changes[1].1.to_bit_string().as_deref(), Some("10xz"));
    assert_eq!(changes[2].1.to_bit_string().as_deref(), Some("zx01"));

    //individual bits, lsb first
    let bits = changes[1].1.as_bits().expect("bit vector");
    assert_eq!(bits.width(), 4);
    assert_eq!(bits.get(0), Some(BitValue::Z));
    assert_eq!(bits.get(1), Some(BitValue::X));
    assert_eq!(bits.get(2), Some(BitValue::Zero));
    assert_eq!(bits.get(3), Some(BitValue::One));
    assert_eq!(bits.get(4), None);
    assert!(bits.has_unknown());
    assert_eq!(bits.as_u64(), None);

    //a fully unknown bus
    let data = waveform.signal_by_path("top.data").unwrap();
    let data_changes = waveform.all_changes(data.index()).unwrap();
    assert_eq!(data_changes[2].1.to_bit_string().as_deref(), Some("xxxxxxxx"));

    //fully defined values convert to numbers
    assert_eq!(data_changes[1].1.to_u64(), Some(0xaa));

    //a single `z` bit is still a one bit vector, not an event
    let clk = waveform.signal_by_path("top.clk").unwrap();
    let clk_changes = waveform.all_changes(clk.index()).unwrap();
    assert_eq!(clk_changes[2].1.kind(), SignalKind::BitVector);
    assert_eq!(clk_changes[2].1.as_bits().unwrap().bit_string(), "z");
}

#[test]
fn timestamps_are_reported_in_order() {
    let waveform = open();

    let data = waveform.signal_by_path("top.data").unwrap();
    let times: Vec<_> = waveform
        .all_changes(data.index())
        .unwrap()
        .into_iter()
        .map(|(time, _)| time)
        .collect();
    assert_eq!(times, vec![0, 10, 20]);

    //a signal that does not change in the last time step keeps its last value
    let pc = waveform.signal_by_path("top.sub.u_cpu.pc").unwrap();
    let pc_times: Vec<_> = waveform
        .all_changes(pc.index())
        .unwrap()
        .into_iter()
        .map(|(time, _)| time)
        .collect();
    assert_eq!(pc_times, vec![0, 10]);
}

#[test]
fn value_at_returns_the_value_in_effect() {
    let waveform = open();
    let data = waveform.signal_by_path("top.data").unwrap().index();
    let pc = waveform.signal_by_path("top.sub.u_cpu.pc").unwrap().index();
    let flag = waveform.signal_by_path("top.sub.flag").unwrap().index();

    let bits_at = |signal: usize, time| {
        waveform
            .value_at(signal, time)
            .unwrap()
            .and_then(|value| value.to_bit_string())
    };

    assert_eq!(bits_at(data, 0).as_deref(), Some("00000000"));
    assert_eq!(bits_at(data, 9).as_deref(), Some("00000000"));
    assert_eq!(bits_at(data, 10).as_deref(), Some("10101010"));
    assert_eq!(bits_at(data, 15).as_deref(), Some("10101010"));
    assert_eq!(bits_at(data, 20).as_deref(), Some("xxxxxxxx"));
    assert_eq!(bits_at(data, 1_000).as_deref(), Some("xxxxxxxx"));

    //`pc` changed at 0 and 10 but not at 20
    assert_eq!(bits_at(pc, 0).as_deref(), Some("0000000000000000"));
    assert_eq!(bits_at(pc, 10).as_deref(), Some("0000000000000101"));
    assert_eq!(bits_at(pc, 20).as_deref(), Some("0000000000000101"));
    assert_eq!(bits_at(pc, 2).as_deref(), Some("0000000000000000"));

    //one bit values work the same way
    assert_eq!(bits_at(flag, 9).as_deref(), Some("0"));
    assert_eq!(bits_at(flag, 10).as_deref(), Some("1"));
    assert_eq!(bits_at(flag, 100).as_deref(), Some("0"));

    //a signal that never changed has no value at all
    let silent = wave_insight_lib::waveform::Waveform::open_bytes(SILENT_VCD).unwrap();
    let silent_signal = silent.signal_by_path("top.never").unwrap().index();
    assert_eq!(silent.value_at(silent_signal, 10).unwrap(), None);
}

#[test]
fn streaming_only_reports_the_requested_window() {
    let waveform = open();
    let data = waveform.signal_by_path("top.data").unwrap().index();

    let all = waveform.changes_in_window(data, 0, None).unwrap();
    assert_eq!(all.len(), 3);

    let window = waveform.changes_in_window(data, 10, Some(20)).unwrap();
    assert_eq!(window.len(), 2);
    assert_eq!(window[0].0, 10);
    assert_eq!(window[1].0, 20);

    let inside = waveform.changes_in_window(data, 5, Some(15)).unwrap();
    assert_eq!(inside.len(), 1);
    assert_eq!(inside[0].0, 10);

    let empty = waveform.changes_in_window(data, 11, Some(19)).unwrap();
    assert!(empty.is_empty(), "nothing changes between 10 and 20");
}

#[test]
fn streaming_can_stop_early_and_filter_signals() {
    let waveform = open();
    let data = waveform.signal_by_path("top.data").unwrap().index();

    let mut seen = Vec::new();
    waveform
        .stream_changes(&ValueQuery::for_signal(data), |signal, time, value| {
            seen.push((signal, time, value.to_bit_string()));
            true
        })
        .unwrap();
    assert_eq!(seen.len(), 3);
    assert!(seen.iter().all(|(signal, _, _)| *signal == data));

    //`max_changes` stops the stream without decoding the rest
    let mut count = 0;
    waveform
        .stream_changes(
            &ValueQuery::all().with_max_changes(2),
            |_, _, _| {
                count += 1;
                true
            },
        )
        .unwrap();
    assert_eq!(count, 2);

    //returning `false` from the visitor also stops
    let mut visited = 0;
    waveform
        .stream_changes(&ValueQuery::all(), |_, _, _| {
            visited += 1;
            visited < 4
        })
        .unwrap();
    assert_eq!(visited, 4);
}

#[test]
fn values_can_be_rendered_with_the_old_renderers() {
    let waveform = open();

    //the `ShowType` renderers live on `BitsData`, so the new value hands over a
    //`BitsData` that renders exactly like the old model did
    let nibble = waveform.signal_by_path("top.nibble").unwrap().index();
    let value = waveform.value_at(nibble, 10).unwrap().expect("value at 10");
    let bits_data = value.to_bits_data().expect("bit vector");
    assert_eq!(bits_data.to_string(4, &ShowType::Bin), "10xz");
    assert_eq!(bits_data.to_string(4, &ShowType::Hex), "x");

    //one bit values map onto `BoolData`, including `z`
    let clk = waveform.signal_by_path("top.clk").unwrap().index();
    let bool_at = |time| waveform.value_at(clk, time).unwrap().unwrap().to_bool_data();
    assert_eq!(bool_at(0), Some(BoolData::Zero));
    assert_eq!(bool_at(10), Some(BoolData::One));
    assert_eq!(bool_at(20), Some(BoolData::Z));

    //a bus is not a single bit, so there is no bool form
    let data = waveform.signal_by_path("top.data").unwrap().index();
    assert_eq!(waveform.value_at(data, 0).unwrap().unwrap().to_bool_data(), None);
}

#[test]
fn unknown_signals_are_reported() {
    let waveform = open();
    assert!(waveform.signal_by_path("top.nope").is_none());
    assert!(waveform.signal(99).is_none());

    let error = waveform.all_changes(99).unwrap_err();
    assert!(
        matches!(error, wave_insight_lib::waveform::WaveformError::UnknownSignal(_)),
        "expected UnknownSignal, got {error:?}"
    );
}

#[test]
fn reader_input_works_like_bytes_input() {
    let from_reader =
        Waveform::open_reader(VCD.as_bytes()).expect("VCD from a reader should open");
    assert_eq!(from_reader.file_format(), WaveFormat::Vcd);
    assert_eq!(from_reader.signal_count(), 5);
    assert_eq!(first_bits(&from_reader, "top.data"), "00000000");
}

/* ------------------------------------------------------------------------- */
/* real, string and event signals, plus short binary literals                 */
/* ------------------------------------------------------------------------- */

const MISC_VCD: &str = "\
$timescale 10ns $end
$scope module misc $end
$var real 64 ! temp $end
$var event 1 \" ev $end
$var string 0 ( label $end
$var wire 16 % short [15:0] $end
$upscope $end
$enddefinitions $end
#0
r1.5 !
1\"
shello (
b101 %
#10
r-2.25 !
1\"
sworld (
b1 %
";

#[test]
fn real_string_and_event_values_are_distinguished() {
    let waveform = Waveform::open_bytes(MISC_VCD).expect("misc fixture should parse");
    let temp = waveform.signal_by_path("misc.temp").expect("real signal");
    let ev = waveform.signal_by_path("misc.ev").expect("event signal");
    let label = waveform.signal_by_path("misc.label").expect("string signal");

    assert_eq!(temp.kind(), SignalKind::Real);
    assert_eq!(temp.width(), 0);
    assert_eq!(ev.kind(), SignalKind::Event);
    assert_eq!(label.kind(), SignalKind::String);

    assert_eq!(
        waveform.all_changes(temp.index()).unwrap(),
        vec![(0, WaveValue::Real(1.5)), (10, WaveValue::Real(-2.25))]
    );
    assert_eq!(
        waveform.all_changes(ev.index()).unwrap(),
        vec![(0, WaveValue::Event), (10, WaveValue::Event)]
    );
    assert_eq!(
        waveform.all_changes(label.index()).unwrap(),
        vec![
            (0, WaveValue::String("hello".to_string())),
            (10, WaveValue::String("world".to_string()))
        ]
    );
    assert_eq!(
        waveform.value_at(temp.index(), 5).unwrap(),
        Some(WaveValue::Real(1.5))
    );
}

#[test]
fn short_binary_literals_are_padded_to_the_declared_width() {
    let waveform = Waveform::open_bytes(MISC_VCD).expect("misc fixture should parse");
    let short = waveform.signal_by_path("misc.short").expect("16 bit signal");
    assert_eq!(short.width(), 16);

    let changes = waveform.all_changes(short.index()).unwrap();
    assert_eq!(changes[0].1.to_bit_string().as_deref(), Some("0000000000000101"));
    assert_eq!(changes[0].1.to_u64(), Some(5));
    assert_eq!(changes[1].1.to_bit_string().as_deref(), Some("0000000000000001"));
}

/* ------------------------------------------------------------------------- */
/* legacy adapter                                                            */
/* ------------------------------------------------------------------------- */

#[test]
fn legacy_module_has_the_old_shape() {
    let waveform = open();
    let module = waveform.legacy_module().expect("module should build");

    assert_eq!(module.end_clock, 20);
    let top = module.sub_module.get("top").expect("`top` sub module");
    assert!(top.signal.contains_key("clk"));
    assert!(top.signal.contains_key("data"));
    assert!(top.signal.contains_key("nibble"));

    let sub = top.sub_module.get("sub").expect("`top.sub` sub module");
    assert!(sub.signal.contains_key("flag"));
    let u_cpu = sub.sub_module.get("u_cpu").expect("`top.sub.u_cpu`");
    assert!(u_cpu.signal.contains_key("pc"));

    //the old model has no notion of hierarchy in the key, so `value_key` is
    //the full path now
    assert_eq!(top.signal["clk"].value_key, "top.clk");
    assert_eq!(top.signal["data"].size, 8);
    assert_eq!(u_cpu.signal["pc"].size, 16);
    assert_eq!(top.signal["data"].value_key, "top.data");

    //real/string/event variables are skipped by the adapter
    assert_eq!(top.signal.len() + sub.signal.len() + u_cpu.signal.len(), 5);
}

#[test]
fn legacy_adapter_still_serves_get_bool_and_get_bits() {
    let waveform = open();
    let (_, values) = waveform.to_legacy().expect("legacy conversion");

    assert_eq!(
        values.get_bool("top.clk"),
        vec![(0, BoolData::Zero), (10, BoolData::One), (20, BoolData::Z)],
    );
    assert_eq!(
        values.get_bool("top.sub.flag"),
        vec![(0, BoolData::Zero), (10, BoolData::One), (20, BoolData::Zero)],
    );

    let data = values.get_bits("top.data");
    assert_eq!(data.len(), 3);
    assert_eq!(data[0].1.to_string(8, &ShowType::Bin), "00000000");
    assert_eq!(data[1].1.to_string(8, &ShowType::Bin), "10101010");
    assert_eq!(data[1].1.to_string(8, &ShowType::UInt), "170");
    assert_eq!(data[2].1.to_string(8, &ShowType::Bin), "xxxxxxxx");

    let nibble = values.get_bits("top.nibble");
    assert_eq!(nibble[1].1.to_string(4, &ShowType::Bin), "10xz");
    assert_eq!(nibble[2].1.to_string(4, &ShowType::Bin), "zx01");

    let pc = values.get_bits("top.sub.u_cpu.pc");
    assert_eq!(pc[1].1.to_string(16, &ShowType::Bin), "0000000000000101");
    assert_eq!(pc[1].1.to_string(16, &ShowType::UInt), "5");
}

///The old and the new implementation must agree on the values, signal for
///signal. The old parser keys values by VCD identifier code, the adapter keys
///them by hierarchical path, so the two maps are compared through a key map.
#[test]
fn legacy_adapter_matches_the_old_vcd_parser() {
    let old = {
        let (module, values) = vcd_parser(VCD.to_string(), &mut Module::new());
        (module, values)
    };
    let waveform = open();
    let (module, values) = waveform.to_legacy().unwrap();

    //same module tree
    assert_eq!(old.0.end_clock, module.end_clock, "end clock of the root module");
    let old_top = &old.0.sub_module["top"];
    let new_top = &module.sub_module["top"];
    assert_eq!(old_top.end_clock, new_top.end_clock);
    let mut old_names: Vec<_> = old_top.signal.keys().cloned().collect();
    let mut new_names: Vec<_> = new_top.signal.keys().cloned().collect();
    old_names.sort_unstable();
    new_names.sort_unstable();
    assert_eq!(old_names, new_names);
    assert!(old_top.sub_module.contains_key("sub"));
    assert!(new_top.sub_module["sub"].sub_module.contains_key("u_cpu"));

    //same values, modulo the key naming scheme
    let key_pairs = [
        ("!", "top.clk"),
        ("#", "top.nibble"),
        ("&", "top.sub.flag"),
        ("'", "top.sub.u_cpu.pc"),
    ];
    for (old_key, new_key) in key_pairs {
        assert_eq!(
            old.1.get_bool(old_key),
            values.get_bool(new_key),
            "bool values of `{new_key}` differ from the old parser"
        );
        let old_bits = old.1.get_bits(old_key);
        let new_bits = values.get_bits(new_key);
        assert_eq!(old_bits.len(), new_bits.len(), "change count of `{new_key}`");
        for ((old_time, old_value), (new_time, new_value)) in
            old_bits.iter().zip(new_bits.iter())
        {
            assert_eq!(old_time, new_time, "time stamp of `{new_key}`");
            //the bit width comes from the declaration, so print all of it
            let width = waveform.signal_by_path(new_key).unwrap().width() as usize;
            assert_eq!(
                old_value.to_string(width, &ShowType::Bin),
                new_value.to_string(width, &ShowType::Bin),
                "value of `{new_key}` at {old_time}"
            );
        }
    }
}

/* ------------------------------------------------------------------------- */
/* hostile input                                                             */
/* ------------------------------------------------------------------------- */

const SILENT_VCD: &str = "\
$scope module top $end
$var wire 12 ! never [11:0] $end
$upscope $end
$enddefinitions $end
#0
";

#[test]
fn malformed_vcd_is_reported_and_never_panics() {
    //truncated header
    let truncated = "$scope module top $end\n$var wire 8 ! data $end\n$enddefinitions $end\n#0\nb1010";
    let result = Waveform::open_bytes(truncated.as_bytes());
    if let Ok(waveform) = result {
        //if wellen accepted it, querying must not panic either
        for signal in waveform.signals() {
            let _ = waveform.all_changes(signal.index());
            let _ = waveform.value_at(signal.index(), 0);
        }
    }

    //garbage
    let error = Waveform::open_bytes(b"\x00\x01\x02not a waveform".to_vec()).unwrap_err();
    assert!(
        matches!(
            error,
            wave_insight_lib::waveform::WaveformError::UnknownFormat
                | wave_insight_lib::waveform::WaveformError::Malformed(_)
        ),
        "unexpected error for garbage input: {error:?}"
    );

    //empty
    let error = Waveform::open_bytes(Vec::new()).unwrap_err();
    assert!(matches!(
        error,
        wave_insight_lib::waveform::WaveformError::UnknownFormat
    ));
}
