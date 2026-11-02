//! Criterion benchmarks for the language server query hot paths.
//!
//! The design is generated in memory (100 leaf modules in 100 files plus a top
//! file that instantiates them, ~3.5k signals and ~7k driver/load references)
//! and elaborated once; every measured iteration then answers position queries
//! the way a keystroke-triggered editor request does.
//!
//! Run with: cargo bench -p wave-insight-lib --bench lsp -- --quick
//!
//! The numbers are the before/after evidence for the span index: the same
//! benchmarks were run once with the index disabled (brute-force scan) and once
//! with it enabled.
//!
//! The `lsp` group also measures completion, the per-keystroke request path:
//! plain module-scoped identifiers, a prefix filter and member access after an
//! instance dot (`u0.`). Each leaf module and `top.sv` carry a `// complete:`
//! marker line that the benchmark puts the caret at.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hint::black_box;
use std::path::{Path, PathBuf};

use criterion::{criterion_group, criterion_main, Criterion};
use serde_json::json;

use wave_insight_lib::design::{DesignDb, ElaboratedDesign};
use wave_insight_lib::lsp::{path_to_uri, LspServer};

/// One leaf module body: 2 ports plus a bank of signals with assigns, so the
/// flat table has declarations and driver/load references to index.
fn module_source(index: usize) -> String {
    let mut source = String::new();
    writeln!(
        &mut source,
        "module leaf{index}(input logic clk, output logic [7:0] y);"
    )
    .expect("writing to a String cannot fail");
    for signal in 0..32 {
        writeln!(&mut source, "    logic [7:0] s{signal};").expect("write");
    }
    writeln!(&mut source, "    assign y = s0;").expect("write");
    for signal in 0..32 {
        writeln!(
            &mut source,
            "    assign s{signal} = clk ? 8'h{signal:02x} : 8'h00;"
        )
        .expect("write");
    }
    //completion cursors: a plain module-scoped position (empty prefix) and a
    //prefix-filtered one; the comment is valid SystemVerilog, so the design and
    //the document text stay identical
    source.push_str("    // complete:\n");
    source.push_str("    // complete: s1\n");
    source.push_str("endmodule\n");
    source
}

fn top_source(leaves: usize) -> String {
    let mut source = String::from("module top(input logic clk, output logic [7:0] y);\n");
    for index in 0..leaves {
        writeln!(&mut source, "    leaf{index} u{index} (.clk(clk), .y());").expect("write");
    }
    //completion cursor after an instance dot: u0. resolves to leaf0's members
    source.push_str("    // complete: u0.\n");
    source.push_str("endmodule\n");
    source
}

fn bench_root() -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(r"C:\bench")
    } else {
        PathBuf::from("/bench")
    }
}

/// One query position: which generated file, and the 1-based line/column.
#[derive(Clone)]
struct Position {
    file: PathBuf,
    line: u32,
    column: u32,
}

struct Fixture {
    db: DesignDb,
    design: ElaboratedDesign,
    /// Sources by path, in the order they were added.
    files: Vec<(PathBuf, String)>,
    /// Positions spread over the generated files (declarations, a module name
    /// and a position outside any span).
    positions: Vec<Position>,
    uris: HashMap<PathBuf, String>,
}

impl Fixture {
    fn new(leaves: usize) -> Self {
        let root = bench_root();
        let mut files = Vec::new();
        for index in 0..leaves {
            files.push((
                root.join(format!("leaf{index:03}.sv")),
                module_source(index),
            ));
        }
        files.push((root.join("top.sv"), top_source(leaves)));

        let mut db = DesignDb::new();
        for (path, source) in &files {
            db.add_source(path, source);
        }
        let design = db.elaborate("top");
        let uris: HashMap<PathBuf, String> = files
            .iter()
            .map(|(path, _)| (path.clone(), path_to_uri(path)))
            .collect();
        let file_index: HashMap<&Path, usize> = files
            .iter()
            .enumerate()
            .map(|(index, (path, _))| (path.as_path(), index))
            .collect();

        let mut positions = Vec::new();
        for signal in design.signals().iter().step_by(40) {
            if let Some(&index) = file_index.get(signal.declaration.file.as_path()) {
                positions.push(Position {
                    file: files[index].0.clone(),
                    line: signal.declaration.start_line,
                    column: signal.declaration.start_col,
                });
            }
        }
        if let Some(module) = db.modules().first() {
            if let Some(&index) = file_index.get(module.span.file.as_path()) {
                positions.push(Position {
                    file: files[index].0.clone(),
                    line: module.span.start_line,
                    column: module.span.start_col,
                });
            }
        }
        positions.push(Position {
            file: root.join("leaf000.sv"),
            line: 1,
            column: 1,
        });

        Self {
            db,
            design,
            files,
            positions,
            uris,
        }
    }

    /// A server with all generated documents open and the top configured.
    fn server(&self) -> LspServer {
        let mut server = LspServer::with_db(self.db.clone());
        let initialize = json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": { "capabilities": {} },
        });
        let reply = server.handle_message(&initialize.to_string());
        assert_eq!(reply.len(), 1);
        assert!(
            reply[0].contains("\"result\""),
            "initialize failed: {}",
            reply[0]
        );
        server.set_top(Some("top".to_owned()));
        for (path, source) in &self.files {
            let open = json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": { "textDocument": {
                    "uri": self.uris[path],
                    "languageId": "systemverilog",
                    "version": 1,
                    "text": source,
                }},
            });
            assert!(server.handle_message(&open.to_string()).is_empty());
        }
        let _ = server.take_notifications();
        server
    }
}

fn position_request(uri: &str, method: &str, line: u32, column: u32) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": line - 1, "character": column - 1 },
        },
    })
    .to_string()
}

/// The fixture source of the generated file whose path ends with `name`.
fn fixture_source<'a>(fixture: &'a Fixture, name: &str) -> (&'a PathBuf, &'a str) {
    fixture
        .files
        .iter()
        .find(|(path, _)| path.ends_with(name))
        .map(|(path, source)| (path, source.as_str()))
        .unwrap_or_else(|| panic!("the fixture has a `{name}` file"))
}

/// The line whose trimmed text is exactly `text`, as a 1-based (line, column)
/// caret at the end of that line - the shape [`position_request`] takes.
fn caret_at_end_of(source: &str, text: &str) -> (u32, u32) {
    let index = source
        .lines()
        .position(|line| line.trim() == text)
        .unwrap_or_else(|| panic!("`{text}` is not a line of the fixture source"));
    let line = source.lines().nth(index).expect("the line exists");
    (index as u32 + 1, line.chars().count() as u32 + 1)
}

/// Fail the bench setup when a completion request does not answer items: a
/// benchmark on an empty list would silently measure nothing.
fn assert_completion_items(server: &mut LspServer, request: &str, mode: &str) {
    let reply = server.handle_message(request);
    assert!(
        reply
            .first()
            .is_some_and(|reply| reply.contains("\"label\"")),
        "{mode} completion should answer items: {reply:?}"
    );
}

fn bench_design_queries(c: &mut Criterion) {
    let fixture = Fixture::new(100);
    let mut group = c.benchmark_group("design");
    group.sample_size(10);

    group.bench_function("item_at", |b| {
        b.iter(|| {
            for position in &fixture.positions {
                black_box(fixture.db.item_at(
                    &fixture.design,
                    &position.file,
                    position.line,
                    position.column,
                ));
            }
        })
    });

    group.bench_function("signals_at", |b| {
        b.iter(|| {
            for position in &fixture.positions {
                black_box(fixture.db.signals_at(
                    &fixture.design,
                    &position.file,
                    position.line,
                    position.column,
                ));
            }
        })
    });

    group.finish();
}

fn bench_lsp_requests(c: &mut Criterion) {
    let fixture = Fixture::new(100);
    let mut group = c.benchmark_group("lsp");
    group.sample_size(10);

    for method in ["textDocument/definition", "textDocument/references"] {
        let name = if method.ends_with("definition") {
            "definition"
        } else {
            "references"
        };
        let mut server = fixture.server();
        let positions: Vec<(String, u32, u32)> = fixture
            .positions
            .iter()
            .map(|position| {
                (
                    fixture.uris[&position.file].clone(),
                    position.line,
                    position.column,
                )
            })
            .collect();
        group.bench_function(name, move |b| {
            let mut index = 0usize;
            b.iter(|| {
                let (uri, line, column) = &positions[index % positions.len()];
                index += 1;
                black_box(server.handle_message(&position_request(uri, method, *line, *column)))
            })
        });
    }

    {
        // A leaf file has one module; the unindexed documentSymbol used to walk
        // all 101 modules of the database to find it.
        let mut server = fixture.server();
        let uri = fixture.uris[&bench_root().join("leaf000.sv")].clone();
        let message = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "textDocument/documentSymbol",
            "params": { "textDocument": { "uri": uri } },
        })
        .to_string();
        group.bench_function("document_symbol", move |b| {
            b.iter(|| black_box(server.handle_message(&message)))
        });
    }

    {
        // Completion: the request an editor sends on every keystroke. The
        // generated fixture carries a `// complete:` marker line in each leaf
        // module and a `// complete: u0.` marker in top.sv, so the three modes
        // are: module-scoped with an empty prefix (ports, 32 signals,
        // parameters, genvars, instances and all 101 module names), a prefix
        // filter (`s1`) and member access after an instance dot (`u0.`).
        let (leaf_path, leaf_source) = fixture_source(&fixture, "leaf000.sv");
        let (top_path, top_source) = fixture_source(&fixture, "top.sv");
        let leaf_uri = fixture.uris[leaf_path].clone();
        let top_uri = fixture.uris[top_path].clone();
        let (plain_line, plain_column) = caret_at_end_of(leaf_source, "// complete:");
        let (prefix_line, prefix_column) = caret_at_end_of(leaf_source, "// complete: s1");
        let (member_line, member_column) = caret_at_end_of(top_source, "// complete: u0.");

        let mut server = fixture.server();
        let plain = position_request(
            &leaf_uri,
            "textDocument/completion",
            plain_line,
            plain_column,
        );
        assert_completion_items(&mut server, &plain, "plain");
        group.bench_function("completion_plain", move |b| {
            b.iter(|| black_box(server.handle_message(&plain)))
        });

        let mut server = fixture.server();
        let prefix = position_request(
            &leaf_uri,
            "textDocument/completion",
            prefix_line,
            prefix_column,
        );
        assert_completion_items(&mut server, &prefix, "prefix");
        group.bench_function("completion_prefix", move |b| {
            b.iter(|| black_box(server.handle_message(&prefix)))
        });

        let mut server = fixture.server();
        let member = position_request(
            &top_uri,
            "textDocument/completion",
            member_line,
            member_column,
        );
        assert_completion_items(&mut server, &member, "member");
        group.bench_function("completion_member", move |b| {
            b.iter(|| black_box(server.handle_message(&member)))
        });
    }

    group.finish();
}

criterion_group!(benches, bench_design_queries, bench_lsp_requests);
criterion_main!(benches);
