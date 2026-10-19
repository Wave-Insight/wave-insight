//! The browser side of the language server: document sync, JSON-RPC plumbing
//! and the answers the monaco providers consume.
//!
//! # Why a URI of our own?
//!
//! The design database keys every source by the name it was given to
//! [`DesignState::add_source`](crate::nav::DesignState::add_source), which in
//! the browser is the bare file name the drop handler read out of the `File`
//! (`top.sv`). The LSP addresses documents by URI and maps `file:` URIs back to
//! paths, so a real `file:///top.sv` would become `\top.sv` on Windows and stop
//! matching. A URI with a different scheme is kept *verbatim* as the database
//! key (`wave_insight_lib::lsp::uri`), which round-trips exactly, so
//! [`document_uri`] and [`file_of_uri`] are the two halves of one spelling and
//! every `didOpen` uses the same key as the app's own database.
//!
//! # Fallback
//!
//! The LSP is the primary answer for every question, but navigation must keep
//! working when it cannot answer (a server that failed to initialise, a query
//! the round-one surface does not implement, a malformed reply). Every getter
//! therefore tries the LSP first and falls back to the app's [`DesignState`],
//! which is the same parse of the same files. Failures are counted, never
//! thrown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use js_sys::{Function, Object, Reflect};
use serde_json::{json, Value};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use yew::prelude::Callback;

use crate::nav::{Goto, SharedDesign};

use super::shim::DesignLsp;

/// The monaco language the providers are registered on and the `languageId` a
/// `didOpen` announces.
pub const LANGUAGE_ID: &str = "systemverilog";

/// Prefix of the URI (and therefore of the database key) of one design source.
const DOCUMENT_URI_PREFIX: &str = "wave-insight://source/";

const HEX: [u8; 16] = *b"0123456789ABCDEF";

/// The URI a design source is addressed by, inside and outside the LSP.
///
/// The mapping is a pure function of the file name: the same file name always
/// produces the same URI, which is what keeps one document from being opened
/// twice under two spellings.
pub fn document_uri(file: &str) -> String {
    let mut uri = String::with_capacity(DOCUMENT_URI_PREFIX.len() + file.len());
    uri.push_str(DOCUMENT_URI_PREFIX);
    for byte in file.bytes() {
        let safe = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':');
        if safe {
            uri.push(char::from(byte));
        } else {
            uri.push('%');
            uri.push(char::from(HEX[usize::from(byte >> 4)]));
            uri.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    uri
}

/// The design source a [`document_uri`] addresses, i.e. the inverse mapping.
pub fn file_of_uri(uri: &str) -> Option<String> {
    let encoded = uri.strip_prefix(DOCUMENT_URI_PREFIX)?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                decoded.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// LSP value shapes (0-based, as the protocol counts them)
// ---------------------------------------------------------------------------

/// One LSP position: 0-based line and 0-based UTF-16 column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

impl Position {
    pub fn new(line: u32, character: u32) -> Self {
        Self { line, character }
    }
}

/// A half-open LSP range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

impl Range {
    pub fn new(start: Position, end: Position) -> Self {
        Self { start, end }
    }
}

/// A `Location`: a document URI plus the range inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

/// The hover card the providers show: markdown/plaintext plus the range it
/// belongs to.
pub type Hover = (String, Option<Range>);

fn parse_position(value: &Value) -> Option<Position> {
    let line = value.get("line")?.as_u64()?;
    let character = value.get("character")?.as_u64()?;
    Some(Position::new(line as u32, character as u32))
}

pub(crate) fn parse_range(value: &Value) -> Option<Range> {
    Some(Range::new(
        parse_position(value.get("start")?)?,
        parse_position(value.get("end")?)?,
    ))
}

fn parse_location(value: &Value) -> Option<Location> {
    let uri = value.get("uri")?.as_str()?.to_owned();
    Some(Location {
        uri,
        range: parse_range(value.get("range")?)?,
    })
}

fn location_of(file: &str, line: u32, column: u32) -> Location {
    // 1-based database coordinates → the 0-based LSP ones; a span end is
    // approximated by its start when only one position is known.
    let position = Position::new(line.saturating_sub(1), column.saturating_sub(1));
    Location {
        uri: document_uri(file),
        range: Range::new(position, position),
    }
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/// One `didOpen` document, kept in sync by [`LspBridge::open`].
struct OpenDocument {
    uri: String,
    version: i64,
    text: String,
}

/// Everything the bridge owns; behind one `RefCell` so the providers, the app
/// and the debug hook all share it.
struct Bridge {
    lsp: DesignLsp,
    /// The app's own database, i.e. the fallback for every question.
    design: SharedDesign,
    /// How to ask the UI to bring a position into view.
    goto: Callback<Goto>,
    /// Request ids; only ever incremented.
    next_id: i64,
    /// `file name → document`, in `didOpen` order of first arrival.
    documents: HashMap<String, OpenDocument>,
    initialized: bool,
    /// Counters and the last failure, surfaced in the debug hook and in the
    /// top bar status line.
    requests: u64,
    failures: u64,
    last_error: Option<String>,
    /// The module the server elaborates from, when the app configured one.
    top: Option<String>,
}

impl Bridge {
    fn notification(&mut self, method: &str, params: Value) {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let reply = self.lsp.handle(&message.to_string());
        if !reply.is_empty() {
            // A notification must not be answered; a reply here means the
            // server rejected it. Keep it instead of dropping it silently.
            self.failures += 1;
            self.last_error = Some(format!("{method}: unexpected reply"));
        }
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let reply = self.lsp.handle(&message.to_string());
        self.requests += 1;
        if reply.is_empty() {
            return Err(format!("{method}: no reply"));
        }
        let value: Value = serde_json::from_str(&reply)
            .map_err(|error| format!("{method}: unparsable reply: {error}"))?;
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
            return Err(format!("{method}: {message} (code {code})"));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }

    fn record(&mut self, error: &str) {
        self.failures += 1;
        self.last_error = Some(error.to_owned());
    }

    fn initialize(&mut self) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        // `monaco` renders markdown hovers; ask for it explicitly so the server
        // does not have to guess (its own default is markdown too).
        let params = json!({
            "processId": Value::Null,
            "clientInfo": { "name": "wave-insight-web" },
            "rootUri": Value::Null,
            "capabilities": {
                "textDocument": {
                    "hover": { "contentFormat": ["markdown", "plaintext"] }
                }
            },
        });
        match self.request("initialize", params) {
            Ok(_) => self.notification("initialized", json!({})),
            Err(error) => self.record(&error),
        }
    }
}

/// Shared handle on the browser's language server client.
///
/// Cheap to clone and compares by identity, so it can travel through the Yew
/// properties that drive the source pane.
#[derive(Clone)]
pub struct LspBridge(Rc<RefCell<Bridge>>);

impl LspBridge {
    /// Build the client, send `initialize` once and publish the debug hook.
    pub fn new(design: SharedDesign, goto: Callback<Goto>) -> Self {
        let bridge = Self(Rc::new(RefCell::new(Bridge {
            lsp: DesignLsp::new(),
            design,
            goto,
            next_id: 0,
            documents: HashMap::new(),
            initialized: false,
            requests: 0,
            failures: 0,
            last_error: None,
            top: None,
        })));
        bridge.0.borrow_mut().initialize();
        publish_debug_hook(Rc::clone(&bridge.0));
        bridge
    }

    /// The app's design database, for callers that need it directly.
    pub fn design(&self) -> SharedDesign {
        self.0.borrow().design.clone()
    }

    // -- document lifecycle ------------------------------------------------

    /// `didOpen` the first time, `didChange` (full text) whenever the buffer
    /// differs. Called for every loaded source, not only the visible tab.
    pub fn open(&self, file: &str, text: &str) {
        if file.is_empty() {
            return;
        }
        let mut bridge = self.0.borrow_mut();
        let (method, params) = match bridge.documents.get_mut(file) {
            Some(document) => {
                if document.text == text {
                    return;
                }
                document.text = text.to_owned();
                document.version += 1;
                (
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": document.uri, "version": document.version },
                        "contentChanges": [{ "text": text }],
                    }),
                )
            }
            None => {
                let uri = document_uri(file);
                bridge.documents.insert(
                    file.to_owned(),
                    OpenDocument {
                        uri: uri.clone(),
                        version: 1,
                        text: text.to_owned(),
                    },
                );
                (
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": LANGUAGE_ID,
                            "version": 1,
                            "text": text,
                        },
                    }),
                )
            }
        };
        bridge.notification(method, params);
    }

    /// `didClose`: the tab went away. The source stays in the design, exactly
    /// as the server documents, so declarations keep resolving.
    pub fn close(&self, file: &str) {
        let mut bridge = self.0.borrow_mut();
        let Some(document) = bridge.documents.remove(file) else {
            return;
        };
        bridge.notification(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": document.uri } }),
        );
    }

    /// True when the file is open as an editor document right now.
    pub fn is_open(&self, file: &str) -> bool {
        self.0.borrow().documents.contains_key(file)
    }

    /// The text the LSP holds for `file`, so an on-demand model can be created
    /// with the same content the server parses.
    pub fn document_text(&self, file: &str) -> Option<String> {
        self.0
            .borrow()
            .documents
            .get(file)
            .map(|document| document.text.clone())
    }

    /// Tell the server which module to elaborate (the waveform's root scope).
    pub fn set_top(&self, top: Option<&str>) {
        let mut bridge = self.0.borrow_mut();
        let top = top.filter(|top| !top.is_empty());
        if bridge.top.as_deref() == top {
            return;
        }
        bridge.top = top.map(str::to_owned);
        if let Some(top) = top {
            bridge.lsp.set_top(top);
        }
    }

    // -- provider queries (1-based monaco coordinates in, LSP shapes out) ---

    /// Where the signal under `line:column` is declared.
    pub fn definition(&self, file: &str, line: u32, column: u32) -> Option<Location> {
        let uri = document_uri(file);
        let from_lsp = {
            let mut bridge = self.0.borrow_mut();
            match bridge.request(
                "textDocument/definition",
                position_params(&uri, line, column),
            ) {
                Ok(result) => parse_location(&result),
                Err(error) => {
                    bridge.record(&error);
                    None
                }
            }
        };
        from_lsp.or_else(|| self.declaration_fallback(file, line, column))
    }

    /// The hover card for `line:column` (declaration, width, direction, ...).
    pub fn hover(&self, file: &str, line: u32, column: u32) -> Option<Hover> {
        let uri = document_uri(file);
        let from_lsp = {
            let mut bridge = self.0.borrow_mut();
            match bridge.request("textDocument/hover", position_params(&uri, line, column)) {
                Ok(result) => parse_hover(&result),
                Err(error) => {
                    bridge.record(&error);
                    None
                }
            }
        };
        from_lsp.or_else(|| self.hover_fallback(file, line, column))
    }

    /// Declaration (optional) plus every driver and load of the signal under
    /// the cursor.
    pub fn references(&self, file: &str, line: u32, column: u32) -> Vec<Location> {
        let uri = document_uri(file);
        let mut params = position_params(&uri, line, column);
        if let Some(object) = params.as_object_mut() {
            object.insert("context".to_owned(), json!({ "includeDeclaration": true }));
        }
        let from_lsp = {
            let mut bridge = self.0.borrow_mut();
            match bridge.request("textDocument/references", params) {
                Ok(result) => parse_locations(&result),
                Err(error) => {
                    bridge.record(&error);
                    Vec::new()
                }
            }
        };
        if from_lsp.is_empty() {
            self.reference_fallback(file, line, column)
        } else {
            from_lsp
        }
    }

    /// The `documentSymbol` tree of `file`, as the raw LSP objects (0-based).
    ///
    /// The outline has no `DesignState` fallback: the database keeps spans per
    /// module, not the module/port/signal tree of one buffer, so a dead server
    /// means an empty outline while definition, references and hover keep
    /// working from the fallback.
    pub fn symbols(&self, file: &str) -> Vec<Value> {
        let uri = document_uri(file);
        let mut bridge = self.0.borrow_mut();
        match bridge.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": uri } })) {
            Ok(Value::Array(symbols)) => symbols,
            Ok(_) => Vec::new(),
            Err(error) => {
                bridge.record(&error);
                Vec::new()
            }
        }
    }

    /// Ask the UI to show `location`.
    pub fn jump(&self, location: &Location) {
        let Some(file) = file_of_uri(&location.uri) else {
            return;
        };
        let goto = Goto {
            file,
            line: location.range.start.line + 1,
            col: location.range.start.character + 1,
            token: 0,
        };
        let callback = self.0.borrow().goto.clone();
        callback.emit(goto);
    }

    // -- fallbacks ---------------------------------------------------------

    /// The elaborated signal path covering a 1-based source position.
    fn signal_at(&self, file: &str, line: u32, column: u32) -> Option<String> {
        let design = self.design();
        let state = design.borrow();
        state
            .signals_at(file, line, column)
            .into_iter()
            .next()
    }

    fn declaration_fallback(&self, file: &str, line: u32, column: u32) -> Option<Location> {
        let path = self.signal_at(file, line, column)?;
        let design = self.design();
        let state = design.borrow();
        let span = state.declaration(&path)?;
        Some(location_of(
            &span.file.display().to_string(),
            span.start_line,
            span.start_col,
        ))
    }

    fn hover_fallback(&self, file: &str, line: u32, column: u32) -> Option<Hover> {
        let path = self.signal_at(file, line, column)?;
        let design = self.design();
        let state = design.borrow();
        let details = state.signal_details(&path)?;
        let mut text = format!("`{}`\n\n", details.path);
        match details.width {
            Some(width) => text.push_str(&format!("- width: {width} bits\n")),
            None => text.push_str("- width: unresolved\n"),
        }
        if let Some(direction) = &details.direction {
            text.push_str(&format!("- direction: {direction}\n"));
        }
        text.push_str(&format!("- kind: {}\n", details.kind));
        text.push_str(&format!(
            "- drivers: {}, loads: {}\n",
            details.drivers.len(),
            details.loads.len()
        ));
        if let Some(declaration) = &details.declaration {
            text.push_str(&format!("- declared at `{}`\n", declaration.label));
        }
        Some((text, None))
    }

    fn reference_fallback(&self, file: &str, line: u32, column: u32) -> Vec<Location> {
        let path = self.signal_at(file, line, column).unwrap_or_default();
        if path.is_empty() {
            return Vec::new();
        }
        let design = self.design();
        let state = design.borrow();
        let Some(details) = state.signal_details(&path) else {
            return Vec::new();
        };
        let rows = details
            .declaration
            .iter()
            .chain(details.drivers.iter())
            .chain(details.loads.iter());
        // A row only knows where a reference starts, so the fallback range is
        // empty; monaco reveals the line either way.
        rows.map(|row| location_of(&row.file, row.line, row.col))
            .collect()
    }

    // -- test seam ---------------------------------------------------------
}

/// One raw JSON-RPC message in, one reply out. Used by the debug hook.
fn handle_text(bridge: &mut Bridge, message: &str) -> String {
    let reply = bridge.lsp.handle(message);
    if !reply.is_empty() {
        bridge.requests += 1;
    }
    reply
}

/// Publish `window.__waveInsightLsp` so a browser session (and the headless
/// driver) can talk to the same server instance the UI uses, without going
/// through the DOM.
///
/// The closures live for the lifetime of the page, like the bridge itself, so
/// they are `forget`-ed instead of being stored somewhere artificial.
fn publish_debug_hook(state: Rc<RefCell<Bridge>>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let hook = Object::new();

    let shared = Rc::clone(&state);
    let handle = Closure::<dyn FnMut(String) -> String>::new(move |message: String| {
        handle_text(&mut shared.borrow_mut(), &message)
    });
    set(&hook, "handle", handle.as_ref().unchecked_ref::<Function>().into());
    handle.forget();

    let shared = Rc::clone(&state);
    let documents = Closure::<dyn Fn() -> String>::new(move || {
        let bridge = shared.borrow();
        let mut names: Vec<&String> = bridge.documents.keys().collect();
        names.sort();
        let mut entries = serde_json::Map::new();
        for name in names {
            if let Some(document) = bridge.documents.get(name) {
                entries.insert(
                    name.clone(),
                    json!({
                        "uri": document.uri,
                        "version": document.version,
                        "bytes": document.text.len(),
                    }),
                );
            }
        }
        Value::Object(entries).to_string()
    });
    set(&hook, "documents", documents.as_ref().unchecked_ref::<Function>().into());
    documents.forget();

    let shared = Rc::clone(&state);
    let stats = Closure::<dyn Fn() -> String>::new(move || {
        let bridge = shared.borrow();
        json!({
            "initialized": bridge.initialized,
            "requests": bridge.requests,
            "failures": bridge.failures,
            "lastError": bridge.last_error,
            "top": bridge.top,
            "documents": bridge.documents.len(),
        })
        .to_string()
    });
    set(&hook, "stats", stats.as_ref().unchecked_ref::<Function>().into());
    stats.forget();

    let _ = Reflect::set(&window, &JsValue::from_str("__waveInsightLsp"), &hook);
}

impl PartialEq for LspBridge {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for LspBridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bridge = self.0.borrow();
        formatter
            .debug_struct("LspBridge")
            .field("documents", &bridge.documents.len())
            .field("requests", &bridge.requests)
            .field("failures", &bridge.failures)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// `textDocument` + `position` for a 1-based monaco position.
fn position_params(uri: &str, line: u32, column: u32) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": {
            "line": line.saturating_sub(1),
            "character": column.saturating_sub(1),
        },
    })
}

fn parse_hover(result: &Value) -> Option<Hover> {
    let contents = result.get("contents")?;
    let text = match contents {
        Value::String(text) => text.clone(),
        // Our server sends `{kind, value}`; monaco wants just the value.
        Value::Object(_) => contents.get("value")?.as_str()?.to_owned(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text.clone()),
                Value::Object(_) => item.get("value")?.as_str().map(str::to_owned),
                _ => None,
            })
            .collect::<Vec<String>>()
            .join("\n"),
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    Some((text, result.get("range").and_then(parse_range)))
}

fn parse_locations(result: &Value) -> Vec<Location> {
    match result {
        Value::Array(items) => items.iter().filter_map(parse_location).collect(),
        Value::Null => Vec::new(),
        // A single location is legal for `definition`; be liberal.
        other => parse_location(other).into_iter().collect(),
    }
}

fn set(object: &JsValue, key: &str, value: JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), &value);
}
