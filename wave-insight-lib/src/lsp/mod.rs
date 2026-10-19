//! Language server for the SystemVerilog design database.
//!
//! One core, two transports:
//!
//! * **desktop** — the `back-end` crate builds an `lsp` binary that pumps
//!   stdin/stdout through `lsp-server` framing and forwards the JSON-RPC text
//!   to [`LspServer::handle_message`];
//! * **wasm** — the Yew front end wraps a [`LspServer`] in a
//!   `#[wasm_bindgen]` struct and calls [`LspServer::handle_message_text`], one
//!   JSON-RPC message in, one reply out.
//!
//! Nothing in this module reads stdin, spawns threads, touches the clock or
//! depends on `lsp-server`/`lsp-types`, so `cargo check --target
//! wasm32-unknown-unknown` covers the exact code the browser runs.
//!
//! # Surface (first round)
//!
//! `initialize`, `initialized`, `shutdown`, `exit`,
//! `textDocument/didOpen|didChange|didSave|didClose`,
//! `textDocument/definition`, `textDocument/references`,
//! `textDocument/hover`, `textDocument/documentSymbol`.
//!
//! Completion and diagnostics are not implemented yet; unknown requests are
//! answered with JSON-RPC `-32601`, unknown notifications are ignored.
//! Diagnostic publishing is the obvious next step: [`DesignDb::diagnostics`]
//! already carries everything `textDocument/publishDiagnostics` needs.
//!
//! # Coordinates
//!
//! The design database uses 1-based lines and 1-based `char` columns with
//! `PathBuf` files; LSP uses 0-based lines, 0-based UTF-16 columns and URIs.
//! The `document` module converts positions against the live buffer text and
//! the `uri` module maps `file:` URIs to the same path spelling the database was
//! given (and keeps virtual URIs such as `inmemory://model/1` as opaque keys).
//!
//! # Example
//!
//! ```
//! use wave_insight_lib::lsp::json::Json;
//! use wave_insight_lib::lsp::LspServer;
//!
//! let mut server = LspServer::new();
//! let initialize = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
//! let reply = server.handle_message(initialize).pop().expect("initialize is answered");
//! assert!(reply.contains("documentSymbolProvider"));
//!
//! let uri = "file:///tmp/top.sv";
//! let text = "module top(input logic clk);\n  logic busy;\n  assign busy = clk;\nendmodule\n";
//! let did_open = Json::object([
//!     ("jsonrpc", Json::string("2.0")),
//!     ("method", Json::string("textDocument/didOpen")),
//!     (
//!         "params",
//!         Json::object([(
//!             "textDocument",
//!             Json::object([
//!                 ("uri", Json::string(uri)),
//!                 ("version", Json::int(1)),
//!                 ("text", Json::string(text)),
//!             ]),
//!         )]),
//!     ),
//! ]);
//! assert!(server.handle_message(&did_open.to_text()).is_empty(), "notifications are not answered");
//!
//! let hover = Json::object([
//!     ("jsonrpc", Json::string("2.0")),
//!     ("id", Json::int(2)),
//!     ("method", Json::string("textDocument/hover")),
//!     (
//!         "params",
//!         Json::object([
//!             ("textDocument", Json::object([("uri", Json::string(uri))])),
//!             (
//!                 "position",
//!                 Json::object([("line", Json::int(1)), ("character", Json::int(8))]),
//!             ),
//!         ]),
//!     ),
//! ]);
//! let reply = server.handle_message(&hover.to_text()).pop().expect("hover is answered");
//! assert!(reply.contains("width: 1 bits"), "{reply}");
//! assert!(reply.contains("`top.busy`"), "{reply}");
//! ```
//!
//! # Wiring the wasm build
//!
//! The library deliberately does not depend on `wasm-bindgen`; the front end
//! adds the (very thin) export itself:
//!
//! ```ignore
//! use wasm_bindgen::prelude::*;
//! use wave_insight_lib::lsp::LspServer;
//!
//! #[wasm_bindgen]
//! pub struct DesignLsp {
//!     server: LspServer,
//! }
//!
//! #[wasm_bindgen]
//! impl DesignLsp {
//!     #[wasm_bindgen(constructor)]
//!     pub fn new() -> Self {
//!         Self { server: LspServer::new() }
//!     }
//!
//!     /// One JSON-RPC message in, one JSON-RPC reply out; `""` means the
//!     /// message was a notification.
//!     pub fn handle(&mut self, message: &str) -> String {
//!         self.server.handle_message_text(message)
//!     }
//!
//!     pub fn set_top(&mut self, top: &str) {
//!         self.server.set_top(Some(top.to_owned()));
//!     }
//! }
//! ```
//!
//! Then speak LSP to it from JavaScript, e.g. with
//! `MonacoEditor::editor()` as the editor and monaco's own
//! `languages.registerDefinitionProvider` / `registerReferenceProvider` /
//! `registerHoverProvider` / `registerDocumentSymbolProvider`, forwarding each
//! `monaco.languages.*` request as a JSON-RPC message.  `initialize` can be sent
//! before the first `didOpen`; the server does not require it for notifications.
//!
//! # Implementation notes
//!
//! * No JSON dependency: [`json`] is a small strict parser/writer, so a
//!   malformed client message becomes a `-32700` reply instead of a panic.
//! * Elaboration is cached and rebuilt lazily after the first request that
//!   follows a buffer change; a `didChange` only reparses the one file
//!   ([`DesignDb::update_source`]).
//! * A module body that is instantiated several times shares one source range;
//!   queries answer for the first elaborated instance and hover lists the
//!   siblings.

mod document;
pub mod json;
mod protocol;
mod server;
mod uri;

pub use document::Document;
pub use protocol::{Location, Position, Range};
pub use server::{HoverFormat, LspServer};

pub use uri::normalize_path;
pub use uri::to_path as uri_to_path;
pub use uri::to_uri as path_to_uri;
