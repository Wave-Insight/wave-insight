//!Parsing tests for the VCD front end of `wave-insight-lib`.
//!
//!They only use the public api, so they also double as a check that the crate is
//!usable from the outside.

use wave_insight_lib::data_struct::{BoolData, Module, ShowType};
use wave_insight_lib::parser::vcd_parser::vcd_parser;

///`top.clk` is 1 bit, `top.data` is 8 bit, `top.sub.flag` is 1 bit
const VCD: &str = "\
$date today $end
$version wave-insight test $end
$timescale 1ns $end
$scope module top $end
$var wire 1 ! clk $end
$var wire 8 \" data [7:0] $end
$scope module sub $end
$var wire 1 # flag $end
$upscope $end
$upscope $end
$enddefinitions $end
#0
0!
b00000000 \"
#10
1!
b10101010 \"
0#
#20
z!
bxxxxxxxx \"
1#
";

#[test]
fn hierarchy() {
    let (module, _value) = vcd_parser(VCD.to_string(), &mut Module::new());

    let top = module.sub_module.get("top").expect("top module should be there");
    assert!(top.signal.contains_key("clk"));
    assert!(top.signal.contains_key("data"));
    assert!(top.sub_module.contains_key("sub"));
    assert!(top.sub_module["sub"].signal.contains_key("flag"));
}

#[test]
fn signal_declaration() {
    let (module, _value) = vcd_parser(VCD.to_string(), &mut Module::new());
    let top = &module.sub_module["top"];

    assert_eq!(top.signal["clk"].size, 1);
    assert_eq!(top.signal["clk"].value_key, "!");
    assert_eq!(top.signal["data"].size, 8);
    assert_eq!(top.signal["data"].value_key, "\"");
    assert_eq!(top.sub_module["sub"].signal["flag"].value_key, "#");
}

#[test]
fn bool_values() {
    let (_module, value) = vcd_parser(VCD.to_string(), &mut Module::new());

    assert_eq!(
        value.get_bool("!"),
        vec![(0, BoolData::Zero), (10, BoolData::One), (20, BoolData::Z)],
    );
    assert_eq!(
        value.get_bool("#"),
        vec![(10, BoolData::Zero), (20, BoolData::One)],
    );
}

#[test]
fn bits_values() {
    let (_module, value) = vcd_parser(VCD.to_string(), &mut Module::new());
    let data = value.get_bits("\"");

    assert_eq!(data.len(), 3);
    assert_eq!(data[0].1.to_string(8, &ShowType::Bin), "00000000");
    assert_eq!(data[1].1.to_string(8, &ShowType::Bin), "10101010");
    assert_eq!(data[1].1.to_string(8, &ShowType::Hex), "aa");
    assert_eq!(data[1].1.to_string(8, &ShowType::UInt), "170");
    assert_eq!(data[2].1.to_string(8, &ShowType::Bin), "xxxxxxxx");
    assert_eq!(data[2].1.to_string(8, &ShowType::Hex), "xx");
}

#[test]
fn end_clock() {
    let (module, _value) = vcd_parser(VCD.to_string(), &mut Module::new());
    assert_eq!(module.end_clock, 20);
}

///values which are legal VCD but were not handled before: upper case `X`/`Z` and
///the `h`/`l`/`u`/`w`/`-` weaken states
#[test]
fn extended_value_states() {
    let vcd = "\
$scope module top $end
$var wire 4 ! value $end
$upscope $end
$enddefinitions $end
#0
X!
#10
Z!
#20
H!
#30
L!
#40
-!
";
    let (_module, value) = vcd_parser(vcd.to_string(), &mut Module::new());

    assert_eq!(
        value.get_bool("!"),
        vec![
            (0, BoolData::X),
            (10, BoolData::Z),
            (20, BoolData::One),
            (30, BoolData::Zero),
            (40, BoolData::X),
        ],
    );
}

///every one of these used to panic the parser
#[test]
fn malformed_input_does_not_panic() {
    let vcd = "\
$scope module top $end
$var wire
$var wire x ! not_a_size $end
$upscope $end
$upscope $end
$upscope $end
$enddefinitions $end
#0
b !
1!
";
    let (module, value) = vcd_parser(vcd.to_string(), &mut Module::new());

    let top = &module.sub_module["top"];
    assert!(!top.signal.contains_key("not_a_size"));
    //the empty `b` value must not create an entry, otherwise the chunk size in
    //`get_bits` becomes 0 and it panics
    assert_eq!(value.get_bool("!"), vec![(0, BoolData::One)]);
    assert!(value.get_bits("!").len() == 1);
}
