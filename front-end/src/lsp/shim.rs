//! The `#[wasm_bindgen]` face of the language server.
//!
//! `wave_insight_lib::lsp` deliberately stays free of wasm dependencies; this
//! module is the shim its module documentation describes ("Wiring the wasm
//! build"): a struct that owns an [`LspServer`] and turns the JSON-RPC text in
//! and out into a JavaScript visible API.
//!
//! The browser client in [`super::bridge`] talks to the very same object with
//! plain Rust method calls, so the wasm build runs exactly the code this file
//! exports and there is no second implementation to drift.

use wasm_bindgen::prelude::*;
use wave_insight_lib::lsp::LspServer;

/// One language server instance, owned by the page.
#[wasm_bindgen]
pub struct DesignLsp {
    server: LspServer,
}

#[wasm_bindgen]
impl DesignLsp {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            server: LspServer::new(),
        }
    }

    /// One JSON-RPC message in, one JSON-RPC reply out; `""` means the message
    /// was a notification and has no answer.
    pub fn handle(&mut self, message: &str) -> String {
        self.server.handle_message_text(message)
    }

    /// The module the design is elaborated from, i.e. the waveform's root
    /// scope; `None` (the default) auto-detects the top module.
    pub fn set_top(&mut self, top: &str) {
        self.server.set_top(Some(top.to_owned()));
    }
}

impl Default for DesignLsp {
    fn default() -> Self {
        Self::new()
    }
}
