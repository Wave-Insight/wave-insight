//! Generate elaboration, arrayed instances and driver/load diagnostics.
//!
//! Fixtures under tests/data come with provenance comments; the paths below are
//! the Verdi-style flat paths the design database uses.

use std::path::{Path, PathBuf};

use wave_insight_lib::design::{DesignDb, DiagnosticKind, SourceItem};

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
        .unwrap_or_else(|| panic!("needle not found in the fixture: {needle}"));
    source[..index]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u32
        + 1
}

fn col_of(source: &str, needle: &str) -> u32 {
    let index = source
        .find(needle)
        .unwrap_or_else(|| panic!("needle not found in the fixture: {needle}"));
    let line_start = source[..index].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    source[line_start..index].chars().count() as u32 + 1
}

// -- generate -----------------------------------------------------------------

#[test]
fn a_generate_for_unrolls_every_iteration_into_its_own_instance() {
    let db = DesignDb::load_files([fixture("generate.sv")]);
    let design = db.elaborate("gen_top");

    // top plus one gen_leaf per loop iteration (default N = 4).
    assert_eq!(design.instances().len(), 5, "{:?}", design.instances());
    for index in 0..4 {
        let path = format!("gen_top.g[{index}].u_leaf");
        let instance = design
            .instance(&path)
            .unwrap_or_else(|| panic!("missing instance {path}"));
        assert_eq!(instance.module, "gen_leaf");
        assert!(
            design
                .signal(&format!("{path}.a"))
                .is_some_and(|signal| signal.width == Some(1)),
            "child port of {path} is missing or has the wrong width"
        );
    }
    // The generated instances are children of the top instance.
    let top = design.instance("gen_top").expect("top instance");
    assert_eq!(top.children.len(), 4);

    // Generated signals are in the flat table with their declared widths.
    let a = design.signal("gen_top.a").expect("gen_top.a");
    assert_eq!(a.width, Some(4));
    assert_eq!(
        a.drivers.len(),
        0,
        "an input port is not driven inside its module"
    );
    // Four port connections plus the assignments in the wide/big/many blocks.
    assert_eq!(a.loads.len(), 7, "each generated leaf reads a bit of a");
}

#[test]
fn generate_if_and_case_select_the_constant_branch() {
    let db = DesignDb::load_files([fixture("generate.sv")]);
    let design = db.elaborate("gen_top");

    // WIDE defaults to 1: the "wide" branch exists, "narrow" does not.
    let done = design
        .signal("gen_top.wide.done")
        .expect("generate-if then branch signal");
    assert!(
        !done.drivers.is_empty(),
        "assign done = a[0] drives the signal"
    );
    assert!(design.signal("gen_top.narrow.done").is_none());

    // N defaults to 4, so the generate-case default branch is taken.
    assert!(design.signal("gen_top.many.flag").is_some());
    assert!(design.signal("gen_top.two.flag").is_none());

    // A generate construct outside a generate region is elaborated too.
    assert!(design.signal("gen_top.big.busy").is_some());
    assert!(
        design.diagnostics().is_empty(),
        "unexpected diagnostics: {:?}",
        design.diagnostics()
    );
}

#[test]
fn generate_declarations_keep_their_source_spans() {
    let db = DesignDb::load_files([fixture("generate.sv")]);
    let design = db.elaborate("gen_top");
    let text = source("generate.sv");

    let declaration = design
        .declaration("gen_top.wide.done")
        .expect("generate signal declaration");
    assert_eq!(declaration.start_line, line_of(&text, "logic done;"));
    assert_eq!(declaration.start_col, col_of(&text, "done;"));

    // "What is at this position" still answers for a generate declaration.
    match db.item_at(
        &design,
        fixture("generate.sv"),
        declaration.start_line,
        declaration.start_col,
    ) {
        Some(SourceItem::Signal { name, .. }) => assert_eq!(name, "done"),
        other => panic!("expected a signal item for the generate declaration, got {other:?}"),
    }
}

#[test]
fn generated_instances_link_drivers_and_loads_per_iteration() {
    let db = DesignDb::load_files([fixture("generate.sv")]);
    let design = db.elaborate("gen_top");

    for index in 0..4 {
        let port = design
            .signal(&format!("gen_top.g[{index}].u_leaf.a"))
            .expect("child input port");
        assert!(
            port.drivers
                .iter()
                .any(|driver| driver.kind == wave_insight_lib::design::UsageKind::PortConnection),
            "the connection drives g[{index}].u_leaf.a"
        );
        let output = design
            .signal(&format!("gen_top.g[{index}].u_leaf.y"))
            .expect("child output port");
        assert!(
            output
                .loads
                .iter()
                .any(|load| load.kind == wave_insight_lib::design::UsageKind::PortConnection),
            "the connection loads g[{index}].u_leaf.y"
        );
    }

    // Every connection of a generated instance points at the same 4-bit parent
    // signal, so its load list has one reference per iteration.
    assert_eq!(
        design.signal("gen_top.y").expect("gen_top.y").drivers.len(),
        4
    );
}

// -- arrayed instances --------------------------------------------------------

#[test]
fn arrayed_instances_expand_to_one_instance_per_element() {
    let db = DesignDb::load_files([fixture("arrayed.sv")]);
    let design = db.elaborate("arrayed_top");

    // top + two "u" elements + four "v" elements.
    assert_eq!(design.instances().len(), 7, "{:?}", design.instances());
    for index in [1, 0] {
        let instance = design
            .instance(&format!("arrayed_top.u[{index}]"))
            .unwrap_or_else(|| panic!("missing arrayed instance u[{index}]"));
        assert_eq!(instance.module, "arr_leaf");
        assert!(design
            .signal(&format!("arrayed_top.u[{index}].d"))
            .is_some());
    }
    for index in 0..4 {
        assert!(
            design
                .instance(&format!("arrayed_top.v[{index}]"))
                .is_some(),
            "missing arrayed instance v[{index}]"
        );
    }
    // Verdi order: descending indices for u[1:0].
    let children: Vec<&str> = design
        .instance("arrayed_top")
        .expect("top")
        .children
        .iter()
        .map(|index| design.instances()[*index].path.as_str())
        .collect();
    assert_eq!(children[0], "arrayed_top.u[1]");
    assert_eq!(children[1], "arrayed_top.u[0]");
}

#[test]
fn arrayed_instance_connections_link_each_element() {
    let db = DesignDb::load_files([fixture("arrayed.sv")]);
    let design = db.elaborate("arrayed_top");

    let a = design.signal("arrayed_top.a").expect("arrayed_top.a");
    assert_eq!(
        a.loads.len(),
        2,
        "both u elements read arrayed_top.a through their connection"
    );
    let b = design.signal("arrayed_top.b").expect("arrayed_top.b");
    assert_eq!(b.loads.len(), 4, "all four v elements read arrayed_top.b");
    let q = design.signal("arrayed_top.q").expect("arrayed_top.q");
    assert_eq!(q.drivers.len(), 2, "both u elements drive arrayed_top.q");
}

// -- diagnostics --------------------------------------------------------------

#[test]
fn unresolved_names_become_diagnostics_instead_of_being_dropped() {
    let db = DesignDb::load_files([fixture("link_errors.sv")]);

    let design = db.elaborate("link_top");
    let unresolved: Vec<&str> = design
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedName)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(
        unresolved
            .iter()
            .any(|message| message.contains("missing_load")),
        "unresolved load in the leaf module: {unresolved:?}"
    );
    assert!(
        unresolved
            .iter()
            .any(|message| message.contains("missing_driver")),
        "unresolved driver in the top module: {unresolved:?}"
    );
    // The rest of the design is still elaborated.
    assert!(design.signal("link_top.u_link.a").is_some());
}

#[test]
fn an_unresolved_wildcard_connection_is_a_diagnostic() {
    let db = DesignDb::load_files([fixture("link_errors.sv")]);
    let design = db.elaborate("wild_top");
    let wildcard: Vec<&str> = design
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.kind == DiagnosticKind::UnresolvedName)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(
        wildcard
            .iter()
            .any(|message| message.contains("wildcard") && message.contains("rst")),
        "expected a wildcard connection diagnostic for rst: {wildcard:?}"
    );
    assert!(
        design.signal("wild_top.clk").expect("clk").loads.len() == 1,
        "the resolvable clk connection is still linked"
    );
}

#[test]
fn a_port_width_mismatch_is_a_diagnostic() {
    let db = DesignDb::load_files([fixture("link_errors.sv")]);
    let design = db.elaborate("width_top");
    let mismatch: Vec<&str> = design
        .diagnostics()
        .iter()
        .filter(|diagnostic| diagnostic.kind == DiagnosticKind::WidthMismatch)
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(mismatch.len(), 1, "{mismatch:?}");
    let message = mismatch[0];
    assert!(message.contains("4 bits"), "{message}");
    assert!(message.contains("8 bits"), "{message}");
    assert!(message.contains("data"), "{message}");
}

#[test]
fn a_clean_design_has_no_link_diagnostics() {
    let db = DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv"), fixture("generate.sv")]);
    let design = db.elaborate("top");
    let link_diagnostics: Vec<&str> = design
        .diagnostics()
        .iter()
        .filter(|diagnostic| {
            matches!(
                diagnostic.kind,
                DiagnosticKind::UnresolvedName | DiagnosticKind::WidthMismatch
            )
        })
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(link_diagnostics.is_empty(), "{link_diagnostics:?}");
}

#[test]
fn a_non_constant_generate_condition_stays_a_diagnostic() {
    let mut db = DesignDb::new();
    db.add_source(
        "nonconstant_generate.sv",
        "module nc_top(input logic clk);\n    generate\n        if (UNDEFINED_CONST) begin : maybe\n            logic x;\n            assign x = clk;\n        end\n    endgenerate\nendmodule\n",
    );
    let unsupported = db
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.kind == DiagnosticKind::UnsupportedConstruct);
    assert!(
        unsupported,
        "expected an unsupported-construct warning: {:?}",
        db.diagnostics()
    );
    let design = db.elaborate("nc_top");
    assert!(design.instance("nc_top").is_some());
    assert!(design.signal("nc_top.maybe.x").is_none());
}
