//! Differential tests: the span-index queries must answer exactly like the
//! brute-force scan they replace (same item, same span, same order on ties).
//!
//! The brute-force reference below is a copy of the pre-index
//! DesignDb::find_item_at / signals_at algorithms, implemented on the public
//! model so it does not depend on the index under test.

use std::path::{Path, PathBuf};

use wave_insight_lib::design::{DesignDb, ElaboratedDesign, SourceItem, Span};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

fn source(name: &str) -> String {
    std::fs::read_to_string(fixture(name)).expect("fixture should be readable")
}

/// The old scanner: walk modules, then instances, then the flat table; the
/// smallest span wins and the first considered wins on equal size.
fn brute_item_at(
    db: &DesignDb,
    design: &ElaboratedDesign,
    file: &Path,
    line: u32,
    col: u32,
) -> Option<SourceItem> {
    let mut best: Option<(u64, SourceItem)> = None;
    let mut consider = |span: &Span, item: SourceItem| {
        if span.file.as_path() != file || !span.contains(line, col) {
            return;
        }
        let size = span.size_key();
        let replace = match &best {
            Some((best_size, _)) => size < *best_size,
            None => true,
        };
        if replace {
            best = Some((size, item));
        }
    };

    for module in db.modules() {
        consider(
            &module.span,
            SourceItem::Module {
                name: module.name.clone(),
                span: module.span.clone(),
            },
        );
        for port in &module.ports {
            consider(
                &port.span,
                SourceItem::Port {
                    module: module.name.clone(),
                    name: port.name.clone(),
                    span: port.span.clone(),
                },
            );
            if let Some(header_span) = &port.header_span {
                consider(
                    header_span,
                    SourceItem::Port {
                        module: module.name.clone(),
                        name: port.name.clone(),
                        span: header_span.clone(),
                    },
                );
            }
        }
        for signal in &module.signals {
            consider(
                &signal.span,
                SourceItem::Signal {
                    module: module.name.clone(),
                    name: signal.name.clone(),
                    span: signal.span.clone(),
                },
            );
        }
        for declaration in &module.instances {
            consider(
                &declaration.module_span,
                SourceItem::Module {
                    name: declaration.module.clone(),
                    span: declaration.module_span.clone(),
                },
            );
            consider(
                &declaration.span,
                SourceItem::Instance {
                    path: format!("{}.{}", module.name, declaration.name),
                    module: declaration.module.clone(),
                    span: declaration.span.clone(),
                },
            );
        }
    }
    for instance in design.instances() {
        consider(
            &instance.module_span,
            SourceItem::Module {
                name: instance.module.clone(),
                span: instance.module_span.clone(),
            },
        );
        consider(
            &instance.span,
            SourceItem::Instance {
                path: instance.path.clone(),
                module: instance.module.clone(),
                span: instance.span.clone(),
            },
        );
    }
    for signal in design.signals() {
        consider(
            &signal.declaration,
            SourceItem::Signal {
                module: signal.module.clone(),
                name: signal.local_name.clone(),
                span: signal.declaration.clone(),
            },
        );
        for driver in &signal.drivers {
            consider(
                &driver.span,
                SourceItem::Driver {
                    signal: signal.path.clone(),
                    span: driver.span.clone(),
                    kind: driver.kind,
                },
            );
        }
        for load in &signal.loads {
            consider(
                &load.span,
                SourceItem::Load {
                    signal: signal.path.clone(),
                    span: load.span.clone(),
                    kind: load.kind,
                },
            );
        }
    }
    best.map(|(_, item)| item)
}

fn brute_signals_at(design: &ElaboratedDesign, file: &Path, line: u32, col: u32) -> Vec<String> {
    let mut paths = Vec::new();
    for signal in design.signals() {
        let covers = |span: &Span| span.file.as_path() == file && span.contains(line, col);
        let hit = covers(&signal.declaration)
            || signal.drivers.iter().any(|driver| covers(&driver.span))
            || signal.loads.iter().any(|load| covers(&load.span));
        if hit && !paths.contains(&signal.path) {
            paths.push(signal.path.clone());
        }
    }
    paths
}

/// Every position of the fixture file including out-of-span positions just
/// before the first and after the last line.
fn for_each_position(text: &str, mut visit: impl FnMut(u32, u32)) {
    let lines: Vec<&str> = text.split('\n').collect();
    for (index, line) in lines.iter().enumerate() {
        let line_number = index as u32 + 1;
        for column in 1..=line.chars().count() as u32 + 2 {
            visit(line_number, column);
        }
    }
    visit(lines.len() as u32 + 1, 1);
    visit(lines.len() as u32 + 5, 10);
}

fn assert_indexed_agrees(db: &DesignDb, files: &[&str], tops: &[&str]) {
    for top in tops {
        let design = db.elaborate(top);
        for file_name in files {
            let path = fixture(file_name);
            let text = source(file_name);
            let mut positions = 0usize;
            for_each_position(&text, |line, col| {
                positions += 1;
                let indexed = db.item_at(&design, &path, line, col);
                let brute = brute_item_at(db, &design, &path, line, col);
                assert_eq!(
                    indexed, brute,
                    "{file_name}:{line}:{col} (top {top}) item_at differs"
                );
                let indexed_signals = db.signals_at(&design, &path, line, col);
                let brute_signals = brute_signals_at(&design, &path, line, col);
                assert_eq!(
                    indexed_signals, brute_signals,
                    "{file_name}:{line}:{col} (top {top}) signals_at differs"
                );
            });
            assert!(positions > 0);
        }
    }
}

#[test]
fn indexed_queries_match_the_brute_force_scan_on_the_fixtures() {
    let top = DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv")]);
    assert_indexed_agrees(&top, &["top.sv", "cpu.sv", "legacy.v"], &["top"]);

    let generate = DesignDb::load_files([fixture("generate.sv")]);
    assert_indexed_agrees(&generate, &["generate.sv"], &["gen_top"]);

    let links = DesignDb::load_files([fixture("link_errors.sv")]);
    assert_indexed_agrees(
        &links,
        &["link_errors.sv"],
        &["link_top", "wild_top", "width_top"],
    );

    let forms = DesignDb::load_files([fixture("connect_forms.sv")]);
    assert_indexed_agrees(
        &forms,
        &["connect_forms.sv"],
        &["implicit_conn", "wildcard_conn"],
    );
}

#[test]
fn a_deserialized_design_falls_back_to_the_scan_and_still_agrees() {
    let db = DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv")]);
    let design = db.elaborate("top");
    // The index is deliberately not serialized.
    let json = serde_json::to_string(&design).expect("serialize design");
    let restored: ElaboratedDesign = serde_json::from_str(&json).expect("deserialize design");
    let text = source("cpu.sv");
    let path = fixture("cpu.sv");
    for_each_position(&text, |line, col| {
        assert_eq!(
            db.item_at(&design, &path, line, col),
            db.item_at(&restored, &path, line, col),
            "deserialized fallback differs at {line}:{col}"
        );
        assert_eq!(
            db.signals_at(&design, &path, line, col),
            db.signals_at(&restored, &path, line, col),
        );
    });
}

#[test]
fn span_end_points_are_inclusive_like_the_scanner() {
    let db = DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv")]);
    let design = db.elaborate("top");
    let path = fixture("cpu.sv");
    let signal = design
        .signal("top.u_cpu.pc_reg")
        .expect("pc_reg in the flat table");
    let span = &signal.declaration;
    // start, last column of the start line, last line/column
    let last_col = span.end_col.saturating_sub(1).max(span.start_col);
    for (line, col) in [
        (span.start_line, span.start_col),
        (span.start_line, last_col),
        (span.end_line, span.end_col),
    ] {
        assert_eq!(
            db.item_at(&design, &path, line, col),
            brute_item_at(&db, &design, &path, line, col),
            "edge {line}:{col}"
        );
    }
    assert!(db
        .item_at(&design, &path, span.end_line, span.end_col + 1)
        .is_none_or(|item| item.span() != span));
}

#[test]
fn modules_in_file_lists_only_that_files_modules() {
    let db = DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv")]);
    let top_modules: Vec<&str> = db
        .modules_in_file(fixture("top.sv"))
        .iter()
        .map(|module| module.name.as_str())
        .collect();
    assert_eq!(top_modules, vec!["top"]);
    let cpu_modules: Vec<&str> = db
        .modules_in_file(fixture("cpu.sv"))
        .iter()
        .map(|module| module.name.as_str())
        .collect();
    assert_eq!(cpu_modules, vec!["cpu"]);
    assert!(db.modules_in_file(fixture("legacy.v")).is_empty());
}
