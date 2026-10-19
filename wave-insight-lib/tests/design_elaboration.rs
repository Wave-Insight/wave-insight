//! Elaboration tests: instance tree, parameter overrides, port binding by
//! name/position, flat signal table, driver/load extraction and source
//! position queries.

use std::path::{Path, PathBuf};

use wave_insight_lib::design::{
    DesignDb, DiagnosticKind, Direction, SourceItem, UsageKind,
};

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

fn hierarchy_db() -> DesignDb {
    DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv")])
}

#[test]
fn builds_the_instance_tree() {
    let db = hierarchy_db();
    let design = db.elaborate("top");
    assert!(
        design.diagnostics().is_empty(),
        "unexpected elaboration diagnostics: {:?}",
        design.diagnostics()
    );
    assert_eq!(design.top(), "top");
    assert_eq!(design.root(), 0);
    assert_eq!(design.instances().len(), 3);

    let top = design.instance("top").expect("top instance");
    assert!(top.parent.is_none());
    assert_eq!(top.module, "top");
    assert_eq!(top.children.len(), 2);
    assert_eq!(
        top.children
            .iter()
            .map(|index| design.instances()[*index].path.as_str())
            .collect::<Vec<_>>(),
        vec!["top.u_cpu", "top.u_cpu16"]
    );

    let cpu = design.instance("top.u_cpu").expect("u_cpu");
    assert_eq!(cpu.module, "cpu");
    assert_eq!(cpu.parent, Some(0));
    assert_eq!(cpu.span.file, fixture("top.sv"));
    assert_eq!(cpu.span.start_line, line_of(&source("top.sv"), "u_cpu ("));
    assert!(design.instance("top.u_cpu.missing").is_none());
}

#[test]
fn binds_ports_by_name_and_by_position() {
    let db = hierarchy_db();
    let design = db.elaborate("top");

    // Named connections: .pc(pc)
    let named = design.instance("top.u_cpu").expect("u_cpu");
    let pc = named.port("pc").expect("pc binding");
    assert_eq!(pc.direction, Direction::Output);
    assert_eq!(pc.width, Some(8));
    let connection = pc.connection.as_ref().expect("pc connected");
    assert_eq!(connection.text, "pc");
    assert_eq!(connection.signals, vec!["top.pc".to_string()]);

    // Positional connections: (clk, reset, {8'b0, instr}, pc16, result)
    let positional = design.instance("top.u_cpu16").expect("u_cpu16");
    assert_eq!(
        positional
            .ports
            .iter()
            .map(|binding| binding.port.as_str())
            .collect::<Vec<_>>(),
        vec!["clk", "reset", "instr", "pc", "data_out"]
    );
    assert_eq!(
        positional
            .port("pc")
            .and_then(|binding| binding.connection.as_ref())
            .map(|connection| connection.signals.clone()),
        Some(vec!["top.pc16".to_string()])
    );
    assert_eq!(
        positional
            .port("data_out")
            .and_then(|binding| binding.connection.as_ref())
            .map(|connection| connection.text.clone()),
        Some("result".to_string())
    );
    assert_eq!(
        positional
            .port("instr")
            .and_then(|binding| binding.connection.as_ref())
            .map(|connection| connection.signals.clone()),
        Some(vec!["top.instr".to_string()])
    );

    // The top instance has no connections.
    assert!(design
        .instance("top")
        .expect("top")
        .ports
        .iter()
        .all(|binding| binding.connection.is_none()));
}

#[test]
fn applies_parameter_overrides_per_instance() {
    let db = hierarchy_db();
    let design = db.elaborate("top");

    let small = design.instance("top.u_cpu").expect("u_cpu");
    assert_eq!(
        small.parameter("WIDTH").map(|parameter| parameter.value),
        Some(Some(8))
    );
    assert_eq!(
        small.parameter("DEPTH").map(|parameter| parameter.value),
        Some(Some(16))
    );
    assert_eq!(
        small.parameter("STEP").map(|parameter| parameter.is_local),
        Some(true)
    );

    let wide = design.instance("top.u_cpu16").expect("u_cpu16");
    assert_eq!(
        wide.parameter("WIDTH").map(|parameter| parameter.value),
        Some(Some(16))
    );
    assert_eq!(
        wide.parameter("DEPTH").map(|parameter| parameter.value),
        Some(Some(32))
    );

    // Widths are recomputed with the instance's parameter values.
    assert_eq!(
        design.signal("top.u_cpu.pc").map(|signal| signal.width),
        Some(Some(8))
    );
    assert_eq!(
        design.signal("top.u_cpu16.pc").map(|signal| signal.width),
        Some(Some(16))
    );
    assert_eq!(
        design
            .signal("top.u_cpu16.instr")
            .map(|signal| signal.width),
        Some(Some(16))
    );
}

#[test]
fn flat_signal_table_has_declarations_drivers_and_loads() {
    let db = hierarchy_db();
    let design = db.elaborate("top");

    let pc_reg = design.signal("top.u_cpu.pc_reg").expect("pc_reg signal");
    assert_eq!(pc_reg.module, "cpu");
    assert_eq!(pc_reg.local_name, "pc_reg");
    assert_eq!(pc_reg.instance, "top.u_cpu");
    assert_eq!(pc_reg.direction, None);
    assert_eq!(pc_reg.width, Some(8));
    assert_eq!(
        pc_reg.drivers.len(),
        2,
        "pc_reg is assigned in both branches of the always block: {:?}",
        pc_reg.drivers
    );
    assert!(pc_reg
        .drivers
        .iter()
        .all(|driver| driver.kind == UsageKind::ProceduralAssign));
    assert!(pc_reg
        .loads
        .iter()
        .any(|load| load.kind == UsageKind::ContinuousAssign));

    // An input port is driven through its parent connection and read by the
    // event control.
    let clk = design.signal("top.u_cpu.clk").expect("clk");
    assert_eq!(clk.direction, Some(Direction::Input));
    assert!(clk
        .drivers
        .iter()
        .any(|driver| driver.kind == UsageKind::PortConnection));
    assert!(clk
        .loads
        .iter()
        .any(|load| load.kind == UsageKind::EventControl));

    // An output port is driven inside the child and read by the parent.
    let pc = design.signal("top.u_cpu.pc").expect("pc");
    assert!(pc
        .drivers
        .iter()
        .any(|driver| driver.kind == UsageKind::ContinuousAssign));
    assert!(pc
        .loads
        .iter()
        .any(|load| load.kind == UsageKind::PortConnection));

    // The top output is driven by a continuous assign and by the child port.
    let result = design.signal("top.result").expect("result");
    assert!(result
        .drivers
        .iter()
        .any(|driver| driver.kind == UsageKind::ContinuousAssign));
    assert!(result
        .drivers
        .iter()
        .any(|driver| driver.kind == UsageKind::PortConnection));

    // Accessors mirror the table.
    assert_eq!(design.drivers("top.u_cpu.pc_reg").len(), pc_reg.drivers.len());
    assert_eq!(design.loads("top.u_cpu.pc_reg").len(), pc_reg.loads.len());
    assert!(design.signals().len() >= 20);
    assert!(design.signal("top.not_a_signal").is_none());
}

#[test]
fn declaration_and_source_item_queries() {
    let db = hierarchy_db();
    let design = db.elaborate("top");
    let cpu_source = source("cpu.sv");
    let top_source = source("top.sv");

    // Where is this signal declared?
    let declaration = design
        .declaration("top.u_cpu.pc_reg")
        .expect("declaration span");
    assert_eq!(declaration.file, fixture("cpu.sv"));
    assert_eq!(
        declaration.start_line,
        line_of(&cpu_source, "reg   [WIDTH-1:0] pc_reg;")
    );
    assert_eq!(db.declaration_of(&design, "top.u_cpu.pc_reg"), Some(declaration));

    // Module name.
    match db.item_at(
        &design,
        fixture("top.sv"),
        line_of(&top_source, "module top"),
        col_of(&top_source, "top ("),
    ) {
        Some(SourceItem::Module { name, .. }) => assert_eq!(name, "top"),
        other => panic!("expected module item, got {other:?}"),
    }

    // Instance name.
    match db.item_at(
        &design,
        fixture("top.sv"),
        line_of(&top_source, "u_cpu ("),
        col_of(&top_source, "u_cpu ("),
    ) {
        Some(SourceItem::Instance { path, module, .. }) => {
            assert_eq!(path, "top.u_cpu");
            assert_eq!(module, "cpu");
        }
        other => panic!("expected instance item, got {other:?}"),
    }

    // Port declaration (direction declaration line of the port).
    match db.item_at(
        &design,
        fixture("cpu.sv"),
        line_of(&cpu_source, "input  logic [WIDTH-1:0] instr,"),
        col_of(&cpu_source, "instr,"),
    ) {
        Some(SourceItem::Port { module, name, .. }) => {
            assert_eq!(module, "cpu");
            assert_eq!(name, "instr");
        }
        other => panic!("expected port item, got {other:?}"),
    }

    // Signal declaration.
    match db.item_at(
        &design,
        fixture("cpu.sv"),
        line_of(&cpu_source, "reg   [WIDTH-1:0] pc_reg;"),
        col_of(&cpu_source, "pc_reg;"),
    ) {
        Some(SourceItem::Signal { name, .. }) => assert_eq!(name, "pc_reg"),
        other => panic!("expected signal item, got {other:?}"),
    }

    // Driver: left hand side of the non blocking assignment.
    match db.item_at(
        &design,
        fixture("cpu.sv"),
        line_of(&cpu_source, "pc_reg <= next_pc;"),
        col_of(&cpu_source, "pc_reg <= next_pc;"),
    ) {
        Some(SourceItem::Driver { signal, kind, .. }) => {
            assert_eq!(signal, "top.u_cpu.pc_reg");
            assert_eq!(kind, UsageKind::ProceduralAssign);
        }
        other => panic!("expected driver item, got {other:?}"),
    }

    // Load: right hand side of the same assignment.
    match db.item_at(
        &design,
        fixture("cpu.sv"),
        line_of(&cpu_source, "pc_reg <= next_pc;"),
        col_of(&cpu_source, "next_pc;"),
    ) {
        Some(SourceItem::Load { signal, kind, .. }) => {
            assert_eq!(signal, "top.u_cpu.next_pc");
            assert_eq!(kind, UsageKind::ProceduralAssign);
        }
        other => panic!("expected load item, got {other:?}"),
    }

    // Nothing under an unrelated position.
    assert!(db
        .item_at(&design, fixture("cpu.sv"), 1, 1)
        .is_none());
}

#[test]
fn wildcard_and_implicit_port_connections() {
    let db = DesignDb::load_files([fixture("connect_forms.sv")]);
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());

    for top in ["implicit_conn", "wildcard_conn"] {
        let design = db.elaborate(top);
        assert!(design.diagnostics().is_empty(), "{top}: {:?}", design.diagnostics());
        let leaf = design
            .instance(&format!("{top}.u_leaf"))
            .unwrap_or_else(|| panic!("{top}.u_leaf"));
        for port in ["a", "b"] {
            let binding = leaf.port(port).expect(port);
            assert_eq!(
                binding
                    .connection
                    .as_ref()
                    .map(|connection| connection.signals.clone()),
                Some(vec![format!("{top}.{port}")]),
                "{top}.u_leaf.{port}"
            );
        }
        let y = leaf.port("y").expect("y binding");
        assert_eq!(
            y.connection
                .as_ref()
                .map(|connection| connection.signals.clone()),
            Some(vec![format!("{top}.y")])
        );
        // The parent signal reads/writes are linked.
        assert!(design
            .signal(&format!("{top}.y"))
            .expect("top y")
            .drivers
            .iter()
            .any(|driver| driver.kind == UsageKind::PortConnection));
    }
}

#[test]
fn unresolved_top_module_is_a_diagnostic() {
    let db = hierarchy_db();
    let design = db.elaborate("nope");
    assert!(design.instances().is_empty());
    assert!(design
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedModule));
}

#[test]
fn unresolved_instance_module_and_unknown_port_are_diagnostics() {
    let mut db = DesignDb::new();
    db.add_source(
        "elab_errors.sv",
        r#"
module leaf(input logic a, output logic y);
    assign y = a;
endmodule

module top_errors(input logic a, output logic y);
    leaf u_ok(.a(a), .y(y));
    missing u_missing(.a(a));
    leaf u_bad(.a(a), .nope(a), .y(y));
endmodule
"#,
    );
    let design = db.elaborate("top_errors");
    assert!(design
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedModule));
    assert!(design
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnknownPort));
    // The resolvable part of the tree is still available.
    assert!(design.instance("top_errors.u_ok").is_some());
    assert!(design.instance("top_errors.u_bad").is_some());
    assert!(design.instance("top_errors.u_missing").is_none());
}

#[test]
fn non_ansi_header_ports_are_queryable() {
    let db = DesignDb::load_files([fixture("legacy.v")]);
    let design = db.elaborate("legacy");
    let text = source("legacy.v");
    match db.item_at(
        &design,
        fixture("legacy.v"),
        line_of(&text, "module legacy(a, b, y)"),
        col_of(&text, "a, b, y)"),
    ) {
        Some(SourceItem::Port { module, name, .. }) => {
            assert_eq!(module, "legacy");
            assert_eq!(name, "a");
        }
        other => panic!("expected header port item, got {other:?}"),
    }
}

#[test]
fn non_ansi_always_block_drivers_and_loads() {
    let db = DesignDb::load_files([fixture("legacy_ports.sv")]);
    let design = db.elaborate("legacy_ports");
    assert!(design.diagnostics().is_empty(), "{:?}", design.diagnostics());

    let r = design.signal("legacy_ports.r").expect("r");
    assert!(r
        .drivers
        .iter()
        .any(|driver| driver.kind == UsageKind::ProceduralAssign));
    assert!(r
        .loads
        .iter()
        .any(|load| load.kind == UsageKind::ContinuousAssign));

    let z = design.signal("legacy_ports.z").expect("z");
    assert!(z
        .drivers
        .iter()
        .any(|driver| driver.kind == UsageKind::ProceduralAssign));

    let a = design.signal("legacy_ports.a").expect("a");
    assert!(a
        .loads
        .iter()
        .any(|load| load.kind == UsageKind::ProceduralAssign));
}
