//! Transport and instance-aware query tests for the design database:
//! serde bounds on the public model, `signals_at`/`item_at_in_instance`, and
//! incremental `update_source`.

use std::path::{Path, PathBuf};

use wave_insight_lib::design::{
    Connection, DesignDb, Diagnostic, DiagnosticKind, Direction, ElaboratedDesign, FlatSignal,
    InstanceDecl, InstanceNode, ModuleDef, NameRef, ParamAssignment, ParamDef, ParamValue,
    PortBinding, PortConnectionDecl, PortDef, RangeDim, Severity, SignalDef, SignalKind,
    SignalReference, SignalUsage, SourceItem, Span, UsageKind, UsageRole,
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

// Bounds are checked for every public model type; the JSON round trip below
// then exercises the derives on real fixture data.
fn assert_serde<T: serde::Serialize + serde::de::DeserializeOwned>() {}

#[test]
fn public_model_types_implement_serde() {
    assert_serde::<Span>();
    assert_serde::<Direction>();
    assert_serde::<SignalKind>();
    assert_serde::<RangeDim>();
    assert_serde::<PortDef>();
    assert_serde::<SignalDef>();
    assert_serde::<ParamDef>();
    assert_serde::<ParamAssignment>();
    assert_serde::<NameRef>();
    assert_serde::<PortConnectionDecl>();
    assert_serde::<InstanceDecl>();
    assert_serde::<UsageRole>();
    assert_serde::<UsageKind>();
    assert_serde::<SignalUsage>();
    assert_serde::<ModuleDef>();
    assert_serde::<ParamValue>();
    assert_serde::<PortBinding>();
    assert_serde::<Connection>();
    assert_serde::<InstanceNode>();
    assert_serde::<SignalReference>();
    assert_serde::<FlatSignal>();
    assert_serde::<ElaboratedDesign>();
    assert_serde::<SourceItem>();
    assert_serde::<Severity>();
    assert_serde::<DiagnosticKind>();
    assert_serde::<Diagnostic>();
    assert_serde::<DesignDb>();
}

#[test]
fn design_db_json_round_trip() {
    let db = hierarchy_db();
    let db_json = serde_json::to_string(&db).expect("serialize DesignDb");
    assert!(
        !db_json.contains("module_index") && !db_json.contains("file_modules"),
        "private lookup caches must not be transported"
    );
    let restored: DesignDb = serde_json::from_str(&db_json).expect("deserialize DesignDb");
    assert_eq!(db, restored);
    // `module_index` was skipped: `module()` must still resolve by scanning.
    assert!(restored.module("cpu").is_some());
    assert_eq!(restored.module("cpu").expect("cpu").ports.len(), 5);
    assert_eq!(restored.module("top").expect("top").instances.len(), 2);
    assert!(restored.module("missing").is_none());

    let design = db.elaborate("top");
    let design_json = serde_json::to_string(&design).expect("serialize ElaboratedDesign");
    assert!(
        !design_json.contains("signal_index"),
        "the signal lookup cache must not be transported"
    );
    let restored_design: ElaboratedDesign =
        serde_json::from_str(&design_json).expect("deserialize ElaboratedDesign");
    assert_eq!(design, restored_design);

    // `signal_index` was skipped: path lookups fall back to the flat table.
    let pc = restored_design
        .signal("top.u_cpu.pc")
        .expect("signal resolves after round trip");
    assert_eq!(pc.width, Some(8));
    assert_eq!(restored_design.signals().len(), design.signals().len());
    let declaration = restored_design
        .declaration("top.u_cpu.pc_reg")
        .expect("declaration resolves after round trip");
    assert_eq!(
        declaration.start_line,
        design
            .declaration("top.u_cpu.pc_reg")
            .expect("declaration in original")
            .start_line
    );
}

#[test]
fn deserialized_db_update_source_still_detects_duplicate_modules() {
    let mut db = DesignDb::new();
    assert!(db.add_source("first.sv", "module shared; endmodule"));
    let json = serde_json::to_string(&db).expect("serialize DesignDb");
    let mut restored: DesignDb = serde_json::from_str(&json).expect("deserialize DesignDb");
    assert_eq!(restored.modules().len(), 1);

    // Both lookup maps were skipped.  The first mutation rebuilds them from the
    // serialized modules' spans, so a duplicate pushed in from another file is
    // still diagnosed instead of silently shadowing the first definition.
    assert!(restored.update_source("second.sv", "module shared; endmodule"));
    assert!(
        restored
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.kind == DiagnosticKind::DuplicateDeclaration),
        "expected a duplicate module diagnostic, got {:?}",
        restored.diagnostics()
    );
    assert_eq!(restored.modules().len(), 1);
    assert_eq!(restored.module("shared").expect("shared").span.file, PathBuf::from("first.sv"));

    // The rebuilt per-file ownership still releases a name when its file is
    // renamed.
    assert!(restored.update_source("first.sv", "module renamed; endmodule"));
    assert!(restored.module("shared").is_none());
    assert!(restored.module("renamed").is_some());
}

#[test]
fn signals_at_returns_every_instance_of_a_shared_module_body() {
    let db = hierarchy_db();
    let design = db.elaborate("top");
    let cpu_source = source("cpu.sv");
    let line = line_of(&cpu_source, "reg   [WIDTH-1:0] pc_reg;");
    let col = col_of(&cpu_source, "pc_reg;");

    let paths = db.signals_at(&design, fixture("cpu.sv"), line, col);
    assert_eq!(
        paths,
        vec![
            "top.u_cpu.pc_reg".to_string(),
            "top.u_cpu16.pc_reg".to_string(),
        ]
    );

    // Port declarations are shared too.
    let port_paths = db.signals_at(
        &design,
        fixture("cpu.sv"),
        line_of(&cpu_source, "input  logic [WIDTH-1:0] instr,"),
        col_of(&cpu_source, "instr,"),
    );
    assert_eq!(
        port_paths,
        vec![
            "top.u_cpu.instr".to_string(),
            "top.u_cpu16.instr".to_string(),
        ]
    );

    // A position outside every declaration/usage yields nothing.
    assert!(db
        .signals_at(&design, fixture("cpu.sv"), 1, 1)
        .is_empty());
}

#[test]
fn signals_at_covers_port_connection_expressions() {
    let db = hierarchy_db();
    let design = db.elaborate("top");
    let top_source = source("top.sv");
    let line = line_of(&top_source, ".pc(pc),");
    let col = col_of(&top_source, "pc),");

    let paths = db.signals_at(&design, fixture("top.sv"), line, col);
    // The parent signal is read and the child port is driven at the same
    // source position.
    assert_eq!(
        paths,
        vec!["top.pc".to_string(), "top.u_cpu.pc".to_string()]
    );
}

#[test]
fn item_at_in_instance_is_instance_correct() {
    let db = hierarchy_db();
    let design = db.elaborate("top");
    let cpu_source = source("cpu.sv");
    let line = line_of(&cpu_source, "pc_reg <= next_pc;");
    let col = col_of(&cpu_source, "pc_reg <= next_pc;");

    // The same line exists in both instances; the scope picks the right one.
    match db.item_at_in_instance(&design, "top.u_cpu", fixture("cpu.sv"), line, col) {
        Some(SourceItem::Driver { signal, kind, .. }) => {
            assert_eq!(signal, "top.u_cpu.pc_reg");
            assert_eq!(kind, UsageKind::ProceduralAssign);
        }
        other => panic!("expected scoped driver, got {other:?}"),
    }
    match db.item_at_in_instance(&design, "top.u_cpu16", fixture("cpu.sv"), line, col) {
        Some(SourceItem::Driver { signal, .. }) => {
            assert_eq!(signal, "top.u_cpu16.pc_reg");
        }
        other => panic!("expected scoped driver, got {other:?}"),
    }
}

#[test]
fn item_at_in_instance_keeps_module_and_child_instance_answers() {
    let db = hierarchy_db();
    let design = db.elaborate("top");
    let cpu_source = source("cpu.sv");
    let top_source = source("top.sv");

    // A port declaration inside the cpu module.
    match db.item_at_in_instance(
        &design,
        "top.u_cpu",
        fixture("cpu.sv"),
        line_of(&cpu_source, "input  logic [WIDTH-1:0] instr,"),
        col_of(&cpu_source, "instr,"),
    ) {
        Some(SourceItem::Port { module, name, .. }) => {
            assert_eq!(module, "cpu");
            assert_eq!(name, "instr");
        }
        other => panic!("expected scoped port, got {other:?}"),
    }

    // The top instance's source owns its child instantiations.
    match db.item_at_in_instance(
        &design,
        "top",
        fixture("top.sv"),
        line_of(&top_source, "u_cpu ("),
        col_of(&top_source, "u_cpu ("),
    ) {
        Some(SourceItem::Instance { path, module, .. }) => {
            assert_eq!(path, "top.u_cpu");
            assert_eq!(module, "cpu");
        }
        other => panic!("expected scoped instance, got {other:?}"),
    }

    // Unknown instance paths cannot be scoped.
    assert!(db
        .item_at_in_instance(&design, "top.nope", fixture("cpu.sv"), 1, 1)
        .is_none());
}

#[test]
fn update_source_replaces_only_one_files_modules() {
    let mut db = DesignDb::new();
    assert!(db.add_source(
        "leaf.sv",
        "module leaf(input logic a); endmodule"
    ));
    assert!(db.add_source(
        "top.sv",
        "module top(input logic a); leaf u(.a(a)); endmodule"
    ));
    assert_eq!(db.modules().len(), 2);
    assert_eq!(db.module("leaf").expect("leaf").ports.len(), 1);

    assert!(db.update_source(
        "leaf.sv",
        "module leaf(input logic a, input logic b); endmodule"
    ));
    assert_eq!(db.modules().len(), 2);
    assert_eq!(db.module("leaf").expect("leaf").ports.len(), 2);
    assert!(db.module("top").is_some());
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());

    // The updated database still elaborates the untouched file.
    let design = db.elaborate("top");
    assert!(design.instance("top.u").is_some());
    assert!(design.diagnostics().is_empty(), "{:?}", design.diagnostics());
}

#[test]
fn update_source_removes_renamed_modules_and_stale_diagnostics() {
    let mut db = DesignDb::new();
    assert!(!db.add_source("broken.sv", "module broken(input logic a;"));
    assert!(!db.diagnostics().is_empty());

    // Repairing the file clears its stale parse diagnostic and replaces the
    // module definition, including its name.
    assert!(db.update_source("broken.sv", "module renamed(input logic a); endmodule"));
    assert!(db.module("broken").is_none());
    assert!(db.module("renamed").is_some());
    assert!(db.diagnostics().is_empty(), "{:?}", db.diagnostics());
}

#[test]
fn update_source_accepts_unknown_paths_like_add_source() {
    let mut db = DesignDb::new();
    assert!(db.update_source("new.sv", "module new_module; endmodule"));
    assert_eq!(db.files(), vec![PathBuf::from("new.sv")]);
    assert!(db.module("new_module").is_some());
}

///`item_at` used to answer `None` for an instantiation unless the module was
///part of the elaborated hierarchy, which forced every consumer to walk
///`ModuleDef::instances` itself.
#[test]
fn item_at_resolves_an_instance_declaration() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data");
    let file = root.join("top.sv");
    let db = DesignDb::load_files([file.clone(), root.join("cpu.sv")]);
    let design = db.elaborate("top");

    let text = std::fs::read_to_string(&file).unwrap();
    let declaration_line = text
        .lines()
        .position(|line| line.contains("u_cpu "))
        .expect("fixture should instantiate u_cpu") as u32
        + 1;
    let line = text.lines().nth(declaration_line as usize - 1).unwrap();

    //the instantiated module name answers with the module definition
    let module_col = line.find("cpu").unwrap() as u32 + 1;
    match db.item_at(&design, &file, declaration_line, module_col) {
        Some(wave_insight_lib::design::SourceItem::Module { name, .. }) => {
            assert_eq!(name, "cpu");
        },
        other => panic!("expected the module under the instance type, got {other:?}"),
    }

    //the instance name itself answers with the declaration
    let instance_col = line.find("u_cpu").unwrap() as u32 + 1;
    match db.item_at(&design, &file, declaration_line, instance_col) {
        Some(wave_insight_lib::design::SourceItem::Instance { path, .. }) => {
            assert!(path.ends_with("u_cpu"), "got {path}");
        },
        other => panic!("expected the instance declaration, got {other:?}"),
    }
}
