//! Front end tests: module definitions, ports, vectors, parameters, spans,
//! multi-file input and diagnostics on malformed input.

use std::path::{Path, PathBuf};

use wave_insight_lib::design::{DesignDb, DiagnosticKind, Direction, Severity, SignalKind};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

fn source(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture should be readable")
}

fn line_of(source: &str, needle: &str) -> u32 {
    let index = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found in source"));
    source[..index].bytes().filter(|byte| *byte == b'\n').count() as u32 + 1
}

fn col_of(source: &str, needle: &str) -> u32 {
    let index = source
        .find(needle)
        .unwrap_or_else(|| panic!("`{needle}` not found in source"));
    let line_start = source[..index].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    source[line_start..index].chars().count() as u32 + 1
}

#[test]
fn loads_a_verilog_file_without_any_waveform() {
    // This is the case that used to panic in the prototype scanner: a design
    // file is loaded with no VCD/FST anywhere in sight.
    let db = DesignDb::load_files([fixture("legacy.v")]);
    assert!(
        db.diagnostics().is_empty(),
        "unexpected diagnostics: {:?}",
        db.diagnostics()
    );
    assert_eq!(db.files(), vec![fixture("legacy.v")]);

    let module = db.module("legacy").expect("legacy module");
    let names: Vec<&str> = module.ports.iter().map(|port| port.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "y"]);
    assert_eq!(module.ports[0].direction, Direction::Input);
    assert_eq!(module.ports[0].width, Some(1));
    assert_eq!(module.ports[1].direction, Direction::Input);
    assert_eq!(module.ports[2].direction, Direction::Output);
    assert_eq!(module.ports[2].width, Some(8));
    assert_eq!(module.ports[2].kind, SignalKind::Wire);
    assert!(db.module("does_not_exist").is_none());
}

#[test]
fn non_ansi_port_declarations_are_merged_in_header_order() {
    // `input wire [3:0] a, b;` - several names per declaration, explicit
    // net/variable types, classic non-ANSI style.
    let db = DesignDb::load_files([fixture("legacy_ports.sv")]);
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());
    let module = db.module("legacy_ports").expect("module");

    let ports: Vec<(&str, Direction, Option<u32>, SignalKind)> = module
        .ports
        .iter()
        .map(|port| (port.name.as_str(), port.direction, port.width, port.kind))
        .collect();
    assert_eq!(
        ports,
        vec![
            ("a", Direction::Input, Some(4), SignalKind::Wire),
            ("b", Direction::Input, Some(4), SignalKind::Wire),
            ("y", Direction::Output, Some(4), SignalKind::Reg),
            ("z", Direction::Output, Some(1), SignalKind::Wire),
        ]
    );
    assert_eq!(module.signal("bus").map(|signal| signal.width), Some(Some(8)));
    assert_eq!(module.signal("r").map(|signal| signal.kind), Some(SignalKind::Reg));
}

#[test]
fn packed_vectors_and_scalar_types() {
    let db = DesignDb::load_files([fixture("vectors.sv")]);
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());
    let module = db.module("vectors").expect("module");

    let data = module.port("data").expect("data port");
    assert_eq!(data.width, Some(8));
    assert_eq!(data.packed.len(), 1);
    assert_eq!(data.packed[0].msb, "W-1");
    assert_eq!(data.packed[0].lsb, "0");

    let packed4 = module.port("packed4").expect("packed4 port");
    assert_eq!(packed4.width, Some(32)); // D * W

    let matrix = module.port("matrix").expect("matrix port");
    assert_eq!(matrix.packed.len(), 2);
    assert_eq!(matrix.width, Some(32)); // [3:0][7:0]

    assert_eq!(module.signal("pair").map(|signal| signal.width), Some(Some(2)));
    assert_eq!(
        module.signal("count").map(|signal| signal.kind),
        Some(SignalKind::Integer)
    );
    assert_eq!(module.signal("flag").map(|signal| signal.width), Some(Some(1)));
}

#[test]
fn parameters_keep_defaults_and_localparams_are_marked() {
    let db = DesignDb::load_files([fixture("cpu.sv")]);
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());
    let module = db.module("cpu").expect("module");

    let parameters: Vec<(&str, Option<&str>, bool)> = module
        .parameters
        .iter()
        .map(|parameter| {
            (
                parameter.name.as_str(),
                parameter.default.as_deref(),
                parameter.is_local,
            )
        })
        .collect();
    assert_eq!(
        parameters,
        vec![
            ("WIDTH", Some("8"), false),
            ("DEPTH", Some("16"), false),
            ("STEP", Some("1"), true),
        ]
    );
}

#[test]
fn source_spans_point_at_the_declaration() {
    let db = DesignDb::load_files([fixture("cpu.sv")]);
    let module = db.module("cpu").expect("module");
    let text = source("cpu.sv");

    let pc_reg = module.signal("pc_reg").expect("pc_reg declaration");
    assert_eq!(pc_reg.span.file, fixture("cpu.sv"));
    assert_eq!(
        pc_reg.span.start_line,
        line_of(&text, "reg   [WIDTH-1:0] pc_reg;")
    );
    assert_eq!(pc_reg.span.start_col, col_of(&text, "pc_reg;"));

    let instr = module.port("instr").expect("instr port");
    assert_eq!(instr.span.start_line, line_of(&text, "input  logic [WIDTH-1:0] instr,"));
    assert_eq!(instr.span.start_col, col_of(&text, "instr,"));

    let module_span = module.span.clone();
    assert_eq!(module_span.start_line, line_of(&text, "module cpu"));
    assert_eq!(module_span.start_col, col_of(&text, "cpu #"));
}

#[test]
fn multiple_files_are_parsed_together() {
    let db = DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv")]);
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());
    assert_eq!(db.modules().len(), 2);
    let top = db.module("top").expect("top");
    assert_eq!(top.ports.len(), 4);
    assert_eq!(top.instances.len(), 2);
    assert_eq!(top.instances[0].module, "cpu");
    assert_eq!(top.instances[1].module, "cpu");
    assert!(db.module("cpu").is_some());
}

#[test]
fn malformed_file_produces_diagnostics_instead_of_panicking() {
    let db = DesignDb::load_files([fixture("malformed.sv")]);
    assert!(
        db.diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.kind == DiagnosticKind::Parse),
        "expected a parse diagnostic, got {:?}",
        db.diagnostics()
    );
    assert!(db.modules().is_empty());

    // Elaborating a design that failed to parse is also diagnostic-only.
    let design = db.elaborate("broken");
    assert!(design.instances().is_empty());
    assert!(design
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedModule));
}

#[test]
fn unsupported_constructs_degrade_to_warnings() {
    let db = DesignDb::load_files([fixture("with_generate.sv")]);
    let module = db.module("with_generate").expect("module still parsed");
    assert_eq!(module.ports.len(), 2);

    let unsupported: Vec<_> = db
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.kind == DiagnosticKind::UnsupportedConstruct)
        .collect();
    assert!(
        unsupported.len() >= 2,
        "expected typedef/genvar/generate diagnostics, got {:?}",
        db.diagnostics()
    );
    assert!(unsupported
        .iter()
        .all(|diagnostic| diagnostic.severity == Severity::Warning));
}

#[test]
fn duplicate_modules_are_reported_once_and_first_definition_wins() {
    let mut db = DesignDb::new();
    assert!(db.add_source("first.sv", "module dup(input logic a); endmodule"));
    assert!(db.add_source("second.sv", "module dup(input logic b); endmodule"));

    assert!(db
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::DuplicateDeclaration));
    assert_eq!(db.modules().len(), 1);
    assert_eq!(db.module("dup").expect("dup").ports[0].name, "a");
}

#[test]
fn duplicate_signals_inside_a_module_are_reported() {
    let mut db = DesignDb::new();
    db.add_source(
        "dup_signal.sv",
        "module dup_signal; logic y; wire y; endmodule",
    );
    assert!(db
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::DuplicateDeclaration));
}

#[test]
fn ansi_ports_inherit_direction_and_width() {
    let mut db = DesignDb::new();
    db.add_source(
        "ansi_inherit.sv",
        "module ansi_inherit(input a, b, input logic [7:0] c, d, output logic [3:0] y);\nendmodule",
    );
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());
    let module = db.module("ansi_inherit").expect("module");
    assert_eq!(module.ports.len(), 5);
    assert_eq!(module.ports[0].name, "a");
    assert_eq!(module.ports[0].direction, Direction::Input);
    assert_eq!(module.ports[0].width, Some(1));
    // `b` has no header of its own: it inherits `input`, scalar.
    assert_eq!(module.ports[1].name, "b");
    assert_eq!(module.ports[1].direction, Direction::Input);
    assert_eq!(module.ports[1].width, Some(1));
    assert_eq!(module.ports[2].width, Some(8));
    // `d` inherits the `logic [7:0]` header of `c`.
    assert_eq!(module.ports[3].name, "d");
    assert_eq!(module.ports[3].direction, Direction::Input);
    assert_eq!(module.ports[3].width, Some(8));
    assert_eq!(module.ports[3].kind, SignalKind::Logic);
    assert_eq!(module.ports[4].width, Some(4));
}

#[test]
fn inline_sources_and_module_lookup() {
    let mut db = DesignDb::new();
    db.add_source(
        "inline.sv",
        "module inline(input a, b, output [3:0] y);\n  assign y = {2'b0, a, b};\nendmodule",
    );
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());
    let module = db.module("inline").expect("inline module");
    assert_eq!(module.span.file, PathBuf::from("inline.sv"));
    assert_eq!(module.ports.len(), 3);
    assert_eq!(module.ports[0].direction, Direction::Input);
    assert_eq!(module.ports[2].width, Some(4));
}
