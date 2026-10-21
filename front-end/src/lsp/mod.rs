//! The browser language server client: shim, document sync, monaco providers.
//!
//! [`wave_insight_lib::lsp::LspServer`] already speaks the four requests the
//! source pane needs (definition, references, hover, documentSymbol) and it
//! compiles to wasm, but until now nothing in the browser called it: navigation
//! went straight to [`crate::nav::DesignState`]. This module closes that gap.
//!
//! * [`shim`] — the `#[wasm_bindgen]` `DesignLsp` the library's documentation
//!   describes. It is the only wasm-specific piece of the server.
//! * [`bridge`] — the client: `initialize` once, `didOpen`/`didChange`/
//!   `didClose` for every design source, one JSON-RPC request per provider
//!   answer, parsed with the front end's `serde_json`. Every query falls back
//!   to `DesignState` when the server cannot answer, so navigation keeps
//!   working when the LSP is absent or unhappy.
//! * [`provider`] — the four monaco providers registered on `systemverilog`
//!   through [`MonacoEditor::on_ready`](crate::code_reader::monaco_glue::MonacoEditor::on_ready).
//!
//! Two debug seams are published for the headless browser driver:
//! `window.__waveInsightLsp` (raw JSON-RPC in/out against the live server) and
//! `window.__waveInsightProviders` (the registered provider functions).

mod bridge;
mod provider;
mod shim;

pub(crate) mod inlay;

pub use bridge::{document_uri, file_of_uri, LspBridge, LANGUAGE_ID};
pub use provider::install;
