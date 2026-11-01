//! LSP resolution inside port connections and on instance/module names.
//!
//! Regression coverage for the audit finding that `textDocument/definition` and
//! `textDocument/hover` "returned null at .clk(clk), .pc(pc), ...".  The probe
//! that produced those positions used 1-based editor line/column labels but fed
//! them to LSP unchanged, so they landed one line below the construct (or on
//! punctuation / past the end of the line).  The genuine gap found while
//! re-checking was the port-name half of a named connection: with the span
//! covering only the connected expression, a cursor on `.clk` in
//! `.clk(clk)` had no answer.  The design db now records the whole
//! `.port(expression)` range for a named connection, so both halves resolve.
//!
//! Positions here are computed from the fixture text: 0-based line and UTF-16
//! column, exactly what the protocol expects.  On the shared connection span the
//! answer is the *connected signal* (the parent signal); the child port gets the
//! same span but the parent is registered first.  That is the documented choice.

use std::path::{Path, PathBuf};

use wave_insight_lib::design::DesignDb;
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

/// 0-based line / UTF-16 column of the byte index `index` into `source`.
fn position_at(source: &str, index: usize) -> (u64, u64) {
    let line_start = source[..index].rfind('\n').map(|pos| pos + 1).unwrap_or(0);
    let line = source[..index].matches('\n').count() as u64;
    let column = source[line_start..index].encode_utf16().count() as u64;
    (line, column)
}

/// Position of the first occurrence of `needle`, shifted by `offset` bytes into
/// it (so the test can point at the expression or at the port name).
fn position_of(source: &str, needle: &str, offset: usize) -> (u64, u64) {
    let index = source
        .find(needle)
        .unwrap_or_else(|| panic!("needle not found in the fixture: {needle}"));
    position_at(source, index + offset)
}

fn request(
    server: &mut LspServer,
    id: u64,
    method: &str,
    uri: &str,
    position: (u64, u64),
) -> String {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": {
            "textDocument": { "uri": uri },
            "position": { "line": position.0, "character": position.1 },
        },
    })
    .to_string();
    let mut replies = server.handle_message(&message);
    assert_eq!(replies.len(), 1, "a request gets one reply");
    replies.remove(0)
}

fn initialize(server: &mut LspServer) {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {},
    })
    .to_string();
    let replies = server.handle_message(&message);
    assert_eq!(replies.len(), 1, "initialize gets one reply");
    assert!(
        !replies[0].contains("\"error\""),
        "initialize failed: {}",
        replies[0]
    );
}

fn open(server: &mut LspServer, path: &Path, text: &str) {
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": { "textDocument": { "uri": path_to_uri(path), "version": 1, "text": text } },
    })
    .to_string();
    assert!(server.handle_message(&message).is_empty());
}

fn server() -> (LspServer, String, String) {
    let top_path = fixture("top.sv");
    let cpu_path = fixture("cpu.sv");
    let mut server = LspServer::with_db(DesignDb::load_files([&top_path, &cpu_path]));
    let top_text = text("top.sv");
    let uri = path_to_uri(&top_path);
    initialize(&mut server);
    open(&mut server, &top_path, &top_text);
    (server, top_text, uri)
}

fn assert_resolves(server: &mut LspServer, uri: &str, what: &str, position: (u64, u64)) {
    let definition = request(server, 1, "textDocument/definition", uri, position);
    let hover = request(server, 2, "textDocument/hover", uri, position);
    assert!(
        !definition.contains("\"result\":null"),
        "{what} at {position:?}: definition was null ({definition})"
    );
    assert!(
        !hover.contains("\"result\":null"),
        "{what} at {position:?}: hover was null ({hover})"
    );
}

#[test]
fn definition_and_hover_resolve_inside_named_port_connections() {
    let (mut server, top, uri) = server();

    // The expression half of `.port(expression)`.
    for (needle, offset, expected) in [
        (".clk(clk)", 5, "top.clk"),
        (".reset(reset)", 7, "top.reset"),
        (".instr(instr)", 7, "top.instr"),
        (".pc(pc)", 4, "top.pc"),
        (".data_out(data_out)", 10, "top.data_out"),
    ] {
        let position = position_of(&top, needle, offset);
        assert_resolves(&mut server, &uri, needle, position);

        let hover = request(&mut server, 3, "textDocument/hover", &uri, position);
        assert!(
            hover.contains(expected),
            "{needle}: hover did not mention {expected}: {hover}"
        );
    }

    // The port-name half of the same connection (new behaviour: the span covers
    // `.port(expression)`, not only the expression).
    for (needle, declaration_line) in [
        (".clk(clk)", 3),
        (".reset(reset)", 4),
        (".instr(instr)", 5),
        (".pc(pc)", 8),
        (".data_out(data_out)", 9),
    ] {
        let position = position_of(&top, needle, 2);
        assert_resolves(&mut server, &uri, needle, position);
        let definition = request(&mut server, 4, "textDocument/definition", &uri, position);
        let expected = format!("\"line\":{}", declaration_line);
        assert!(
            definition.contains(&expected),
            "{needle} port name should resolve to the connected signal on line {declaration_line}: {definition}"
        );
    }
}

#[test]
fn definition_and_hover_resolve_inside_positional_connections() {
    let (mut server, top, uri) = server();

    // A concatenation expression: any cursor inside it is a load of top.instr.
    let position = position_of(&top, "{8'b0, instr}", 8);
    assert_resolves(&mut server, &uri, "concatenation", position);
    let hover = request(&mut server, 3, "textDocument/hover", &uri, position);
    assert!(hover.contains("top.instr"), "{hover}");

    for needle in ["pc16", "result"] {
        let position = position_of(&top, needle, 1);
        assert_resolves(&mut server, &uri, needle, position);
    }
}

#[test]
fn definition_and_hover_resolve_on_instance_and_module_names() {
    let (mut server, top, uri) = server();

    for (needle, offset) in [("u_cpu (", 2), ("u_cpu16 (", 2)] {
        let position = position_of(&top, needle, offset);
        assert_resolves(&mut server, &uri, needle, position);
        let hover = request(&mut server, 3, "textDocument/hover", &uri, position);
        assert!(
            hover.contains("instance"),
            "{needle}: expected instance hover: {hover}"
        );
    }

    // The instantiated module name answers with the module definition.
    for (needle, offset) in [("cpu #(", 1), ("cpu #(16, 32)", 1)] {
        let position = position_of(&top, needle, offset);
        assert_resolves(&mut server, &uri, needle, position);
        let definition = request(&mut server, 4, "textDocument/definition", &uri, position);
        assert!(definition.contains("cpu.sv"), "{needle}: {definition}");
    }
}

#[test]
fn audit_probe_coordinates_are_one_based_labels() {
    // The audit probe passed editor (1-based) line/column labels to LSP
    // unchanged, so they were interpreted as 0-based protocol coordinates and
    // landed one line below the construct.  Re-checking them as 0-based
    // positions: these two are on connection punctuation / the port name, which
    // the widened `.port(expression)` span now covers.
    let (mut server, _top, uri) = server();
    // 0-based (14, 14) is the '(' of `.reset(reset)`: the connected signal.
    assert_resolves(&mut server, &uri, "audit .reset probe", (14, 14));
    // 0-based (17, 13) is inside the `.data_out` port name: the connected signal.
    assert_resolves(&mut server, &uri, "audit .data_out probe", (17, 13));
    // The other two probes are past the end of a line of punctuation
    // ('    );' and '        pc16,'), where there is no source construct to
    // resolve; a null answer is correct there.
}
