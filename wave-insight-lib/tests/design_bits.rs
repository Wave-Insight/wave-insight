//! Driver/load bit ranges: two slices of one signal stay distinct, single-bit
//! selects and generate-loop index text are kept, the ranges cross the serde
//! boundary additively and the LSP hover shows them.

use std::path::{Path, PathBuf};

use wave_insight_lib::design::{DesignDb, ElaboratedDesign, SignalReference, UsageKind};
use wave_insight_lib::lsp::{path_to_uri, LspServer};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

fn text(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture should be readable")
}

fn bits_of(references: &[SignalReference]) -> Vec<Option<&str>> {
    references
        .iter()
        .map(|reference| reference.bits.as_deref())
        .collect()
}

#[test]
fn two_slices_of_one_signal_keep_distinct_ranges() {
    let db = DesignDb::load_files([fixture("bit_ranges.sv")]);
    let design = db.elaborate("bit_ranges");

    let q = design.signal("bit_ranges.q").expect("q");
    assert_eq!(q.drivers.len(), 2, "{:?}", q.drivers);
    let mut driver_bits: Vec<&str> = q
        .drivers
        .iter()
        .map(|driver| driver.bits.as_deref().expect("driver has a range"))
        .collect();
    driver_bits.sort_unstable();
    assert_eq!(driver_bits, vec!["[3:0]", "[7:4]"]);
    assert!(
        q.drivers
            .iter()
            .all(|driver| driver.kind == UsageKind::ContinuousAssign),
        "{:?}",
        q.drivers
    );

    let a = design.signal("bit_ranges.a").expect("a");
    let mut load_bits: Vec<&str> = a
        .loads
        .iter()
        .filter_map(|load| load.bits.as_deref())
        .collect();
    load_bits.sort_unstable();
    assert_eq!(load_bits, vec!["[0]", "[7:4]"], "{:?}", a.loads);

    let b = design.signal("bit_ranges.b").expect("b");
    assert_eq!(bits_of(&b.loads), vec![Some("[3:0]")]);

    // A plain whole-signal assign keeps the reference without a range.
    let y = design.signal("bit_ranges.y").expect("y");
    assert_eq!(y.drivers.len(), 1);
    assert_eq!(y.drivers[0].bits, None);
}

#[test]
fn single_bit_and_whole_signal_ranges() {
    let db = DesignDb::load_files([fixture("bit_ranges.sv")]);
    let design = db.elaborate("bit_ranges");
    let a = design.signal("bit_ranges.a").expect("a");
    let single: Vec<&str> = a
        .loads
        .iter()
        .filter_map(|load| load.bits.as_deref())
        .filter(|bits| *bits == "[0]")
        .collect();
    assert_eq!(single.len(), 1, "{:?}", a.loads);

    // The leaf module's own whole-signal assign has no range.
    let leaf = DesignDb::load_files([fixture("bit_ranges.sv")]).elaborate("slice_leaf");
    let leaf_q = leaf.signal("slice_leaf.q").expect("leaf q");
    assert_eq!(leaf_q.drivers.len(), 1);
    assert_eq!(leaf_q.drivers[0].bits, None);
}

#[test]
fn generate_loop_select_keeps_its_textual_range() {
    let db = DesignDb::load_files([fixture("bit_ranges_generate.sv")]);
    let design = db.elaborate("bit_ranges_generate");
    let q = design.signal("bit_ranges_generate.q").expect("q");
    assert_eq!(q.drivers.len(), 4, "{:?}", q.drivers);
    assert!(
        q.drivers
            .iter()
            .all(|driver| driver.bits.as_deref() == Some("[i]")),
        "the genvar select stays textual: {:?}",
        q.drivers
    );
    // All unrolled iterations share the one source span of the generate body
    // (the loop is compiled into four drivers from the same line).
    let spans: Vec<(u32, u32)> = q
        .drivers
        .iter()
        .map(|driver| (driver.span.start_line, driver.span.start_col))
        .collect();
    assert!(spans.windows(2).all(|pair| pair[0] == pair[1]), "{spans:?}");

    let d = design.signal("bit_ranges_generate.d").expect("d");
    assert_eq!(d.loads.len(), 4, "{:?}", d.loads);
    assert!(d
        .loads
        .iter()
        .all(|load| load.bits.as_deref() == Some("[i]")));
}

#[test]
fn port_connections_carry_their_slice() {
    let db = DesignDb::load_files([fixture("bit_ranges.sv")]);
    let design = db.elaborate("bit_ranges_top");

    let bus = design.signal("bit_ranges_top.bus").expect("bus");
    let connection_load = bus
        .loads
        .iter()
        .find(|load| load.kind == UsageKind::PortConnection)
        .expect("the instance connection loads bus");
    assert_eq!(connection_load.bits.as_deref(), Some("[3:0]"));

    let out = design.signal("bit_ranges_top.out").expect("out");
    let connection_driver = out
        .drivers
        .iter()
        .find(|driver| driver.kind == UsageKind::PortConnection)
        .expect("the instance connection drives out");
    assert_eq!(connection_driver.bits.as_deref(), Some("[3:0]"));

    // The child port itself is a whole-port reference.
    let child_port = design
        .signal("bit_ranges_top.u.d")
        .expect("child input port");
    assert!(child_port
        .drivers
        .iter()
        .all(|driver| driver.bits.is_none()));
}

#[test]
fn old_json_without_the_bits_field_still_decodes() {
    // Field level: an old payload has span and kind only.
    let reference: SignalReference = serde_json::from_str(
        r#"{"span":{"file":"x.sv","start_line":1,"start_col":1,"end_line":1,"end_col":1},"kind":"ContinuousAssign"}"#,
    )
    .expect("old SignalReference decodes");
    assert_eq!(reference.bits, None);

    // Whole design: strip every bits member (as an old serialization would
    // look) and decode again.
    let db = DesignDb::load_files([fixture("bit_ranges.sv")]);
    let design = db.elaborate("bit_ranges");
    let mut value = serde_json::to_value(&design).expect("serialize design");
    strip_bits(&mut value);
    let restored: ElaboratedDesign = serde_json::from_value(value).expect("old design decodes");
    let q = restored.signal("bit_ranges.q").expect("q after decode");
    assert!(q.drivers.iter().all(|driver| driver.bits.is_none()));
    assert_eq!(restored.signals().len(), design.signals().len());

    // A current serialization keeps the ranges and round-trips through the db.
    let restored_db: DesignDb =
        serde_json::from_str(&serde_json::to_string(&db).expect("serialize db"))
            .expect("decode db");
    let restored_design = restored_db.elaborate("bit_ranges");
    let q = restored_design
        .signal("bit_ranges.q")
        .expect("q after db decode");
    let mut driver_bits: Vec<&str> = q
        .drivers
        .iter()
        .filter_map(|driver| driver.bits.as_deref())
        .collect();
    driver_bits.sort_unstable();
    assert_eq!(driver_bits, vec!["[3:0]", "[7:4]"]);
}

fn strip_bits(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            object.remove("bits");
            for child in object.values_mut() {
                strip_bits(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                strip_bits(child);
            }
        }
        _ => {}
    }
}

fn position_of(source: &str, needle: &str, offset: usize) -> (u64, u64) {
    let index = source
        .find(needle)
        .unwrap_or_else(|| panic!("needle not found: {needle}"))
        + offset;
    let line_start = source[..index].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    let line = source[..index].matches('\n').count() as u64;
    let column = source[line_start..index].encode_utf16().count() as u64;
    (line, column)
}

fn hover_at(server: &mut LspServer, uri: &str, position: (u64, u64)) -> String {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": position.0, "character": position.1 },
        },
    })
    .to_string();
    server
        .handle_message(&message)
        .into_iter()
        .next()
        .expect("hover reply")
}

#[test]
fn hover_mentions_driver_bit_ranges() {
    let db = DesignDb::load_files([fixture("bit_ranges.sv")]);
    let mut server = LspServer::with_db(db);
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": { "capabilities": {} },
    })
    .to_string();
    assert!(server.handle_message(&initialize)[0].contains("\"result\""));
    server.set_top(Some("bit_ranges".to_owned()));
    let top = fixture("bit_ranges.sv");
    let source = text("bit_ranges.sv");
    let uri = path_to_uri(&top);
    let open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": { "textDocument": {
            "uri": uri, "languageId": "systemverilog", "version": 1, "text": source,
        }},
    })
    .to_string();
    assert!(server.handle_message(&open).is_empty());
    let _ = server.take_notifications();

    let q_position = position_of(&source, "q,", 0);
    let hover = hover_at(&mut server, &uri, q_position);
    assert!(hover.contains("\"result\""), "{hover}");
    assert!(hover.contains("driven here:"), "{hover}");
    assert!(
        hover.contains("[3:0]") && hover.contains("[7:4]"),
        "{hover}"
    );

    // y is driven by a whole-signal assign: no driven-here line.
    let y_position = position_of(&source, "y\n", 0);
    let hover = hover_at(&mut server, &uri, y_position);
    assert!(!hover.contains("driven here:"), "{hover}");
}
