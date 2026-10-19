//! The language server core: editor buffers in, LSP answers out.
//!
//! This is the protocol-independent half of the LSP service.  It owns a
//! [`DesignDb`] (plus the elaboration cached from it), the open documents, and
//! the method dispatch.  It never touches stdin/stdout, threads or the clock,
//! which is what lets the desktop binary and the wasm build share it.
//!
//! # Supported surface (first round)
//!
//! | request | answer |
//! |---|---|
//! | `initialize` | capabilities for the four features below, `positionEncoding: utf-16` |
//! | `shutdown` / `exit` | lifecycle, as specified |
//! | `textDocument/definition` | declaration of the signal under the cursor, or the definition of the module name |
//! | `textDocument/references` | declaration (optional) + every driver/load span, or every instantiation of a module |
//! | `textDocument/hover` | short markdown/plaintext card: declaration, module, width, direction, driver/load counts |
//! | `textDocument/documentSymbol` | modules with parameter/port/signal/instance children |
//!
//! Notifications: `initialized`, `textDocument/didOpen`, `didChange`, `didSave`,
//! `didClose`.  Completion and diagnostics are deliberately out of scope for
//! this round.
//!
//! # Which signal does a position mean?
//!
//! The design database answers [`DesignDb::item_at`] for "what is under this
//! position"; the innermost span wins, so a name inside an `assign` is a
//! driver/load of one signal and a declaration is the signal itself.  A
//! `Driver`/`Load` answer already carries the hierarchical path.  A `Signal` or
//! `Port` answer is module-local, so the path is looked up in the flat table by
//! `(module, local name)` — for a module instantiated several times that is the
//! first elaborated instance, and hover lists the siblings.
//!
//! When the item is a module name (a declaration or the module of an
//! instantiation), `definition`/`references` answer for the module instead of a
//! signal.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::design::{
    DesignDb, ElaboratedDesign, FlatSignal, ModuleDef, RangeDim, SourceItem, Span,
};

use super::document::{approximate_span_range, Document};
use super::json::Json;
use super::protocol::{self, Location, Position, Range, RpcError};
use super::uri;

// LSP `SymbolKind` values used by `documentSymbol`.
const SYMBOL_MODULE: i64 = 2;
const SYMBOL_FIELD: i64 = 8;
const SYMBOL_VARIABLE: i64 = 13;
const SYMBOL_CONSTANT: i64 = 14;
const SYMBOL_OBJECT: i64 = 19;

/// How hover text is rendered; chosen from the client's `initialize`
/// capabilities, and settable directly by the wasm host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HoverFormat {
    #[default]
    Markdown,
    PlainText,
}

impl HoverFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            HoverFormat::Markdown => "markdown",
            HoverFormat::PlainText => "plaintext",
        }
    }
}

/// The design database, the open buffers and the answers built from them.
#[derive(Debug)]
pub struct LspServer {
    db: DesignDb,
    /// Cache of [`DesignDb::elaborate`] for the current top module; `None` when
    /// the database holds no modules.
    design: Option<ElaboratedDesign>,
    /// Set by every document change; cleared when the design is refreshed.
    design_dirty: bool,
    /// Explicitly requested top module, if the host configured one.
    top: Option<String>,
    documents: Vec<Document>,
    hover_format: HoverFormat,
    initialized: bool,
    shutdown: bool,
    exit: bool,
}

impl Default for LspServer {
    fn default() -> Self {
        LspServer {
            db: DesignDb::new(),
            design: None,
            design_dirty: true,
            top: None,
            documents: Vec::new(),
            hover_format: HoverFormat::default(),
            initialized: false,
            shutdown: false,
            exit: false,
        }
    }
}

impl LspServer {
    /// An empty server waiting for `didOpen` documents.
    pub fn new() -> Self {
        LspServer::default()
    }

    /// A server over an existing database, e.g. one the desktop binary loaded
    /// from the command line or the UI deserialized from the core.
    pub fn with_db(db: DesignDb) -> Self {
        LspServer {
            db,
            ..LspServer::default()
        }
    }

    /// The design database, for hosts that want to load files directly.
    pub fn db(&self) -> &DesignDb {
        &self.db
    }

    /// Mutable access for hosts that add sources before opening documents.
    pub fn db_mut(&mut self) -> &mut DesignDb {
        self.design_dirty = true;
        &mut self.db
    }

    /// Force the elaboration root, e.g. the top module the waveform UI picked.
    /// `None` restores automatic root detection (a module no other module
    /// instantiates).
    pub fn set_top(&mut self, top: Option<String>) {
        self.top = top;
        self.design_dirty = true;
    }

    /// Override the hover markup preference (the client's `initialize`
    /// capabilities are read automatically, so this is only needed by a host
    /// that bypasses `initialize`).
    pub fn set_hover_format(&mut self, format: HoverFormat) {
        self.hover_format = format;
    }

    pub fn hover_format(&self) -> HoverFormat {
        self.hover_format
    }

    /// The current elaboration (refreshed if a document changed).
    pub fn design(&mut self) -> Option<&ElaboratedDesign> {
        self.ensure_design();
        self.design.as_ref()
    }

    /// The open documents, in `didOpen` order.
    pub fn documents(&self) -> &[Document] {
        &self.documents
    }

    /// True after `shutdown` was requested.
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown
    }

    /// True after the client sent the `exit` notification; the stdio loop stops
    /// on it.
    pub fn exit_requested(&self) -> bool {
        self.exit
    }

    // -- transport-independent entry points --------------------------------

    /// Handle one JSON-RPC message and return the replies.
    ///
    /// A request produces exactly one response; a notification produces none
    /// (the `Vec` exists because later rounds may also queue server-initiated
    /// notifications such as `publishDiagnostics`).
    pub fn handle_message(&mut self, message: &str) -> Vec<String> {
        let incoming = match protocol::parse_incoming(message) {
            Ok(incoming) => incoming,
            Err(error) => return vec![protocol::error_text(&Json::Null, &error)],
        };
        match incoming {
            protocol::Incoming::Response { .. } => Vec::new(),
            protocol::Incoming::Notification { method, params } => {
                self.handle_notification(&method, &params);
                Vec::new()
            }
            protocol::Incoming::Request { id, method, params } => {
                match self.handle_request(&method, &params) {
                    Ok(result) => vec![protocol::result_text(&id, result)],
                    Err(error) => vec![protocol::error_text(&id, &error)],
                }
            }
        }
    }

    /// One message in, one reply string out — the wasm-shaped entry point.
    ///
    /// Returns `""` when the message was a notification (nothing to send back).
    pub fn handle_message_text(&mut self, message: &str) -> String {
        self.handle_message(message).into_iter().next().unwrap_or_default()
    }

    fn handle_request(&mut self, method: &str, params: &Json) -> Result<Json, RpcError> {
        if self.shutdown {
            return Err(RpcError::invalid_request(
                "the server has been shut down; send `exit`",
            ));
        }
        match method {
            "initialize" => {
                if self.initialized {
                    return Err(RpcError::invalid_request("`initialize` was already received"));
                }
                self.initialized = true;
                self.read_client_capabilities(params);
                Ok(self.initialize_result())
            }
            "shutdown" => {
                self.shutdown = true;
                Ok(Json::Null)
            }
            _ if !self.initialized => Err(RpcError::server_not_initialized(method)),
            "textDocument/definition" => self.definition(params),
            "textDocument/references" => self.references(params),
            "textDocument/hover" => self.hover(params),
            "textDocument/documentSymbol" => self.document_symbol(params),
            _ => Err(RpcError::method_not_found(method)),
        }
    }

    fn handle_notification(&mut self, method: &str, params: &Json) {
        match method {
            "initialized" => {}
            "exit" => self.exit = true,
            "textDocument/didOpen" => self.did_open(params),
            "textDocument/didChange" => self.did_change(params),
            "textDocument/didSave" => self.did_save(params),
            "textDocument/didClose" => self.did_close(params),
            // Known but irrelevant for this round, and unknown notifications
            // must be ignored anyway (JSON-RPC).
            "workspace/didChangeConfiguration"
            | "workspace/didChangeWatchedFiles"
            | "workspace/didChangeWorkspaceFolders"
            | "$/cancelRequest"
            | "$/setTrace" => {}
            _ => {}
        }
    }

    fn initialize_result(&self) -> Json {
        Json::object([
            (
                "capabilities",
                Json::object([
                    ("positionEncoding", Json::string("utf-16")),
                    (
                        "textDocumentSync",
                        Json::object([
                            ("openClose", Json::Bool(true)),
                            // 1 = full.  Incremental edits are applied anyway,
                            // clients that honour capabilities send full text.
                            ("change", Json::Integer(1)),
                            ("save", Json::object([("includeText", Json::Bool(true))])),
                        ]),
                    ),
                    ("definitionProvider", Json::Bool(true)),
                    ("referencesProvider", Json::Bool(true)),
                    ("hoverProvider", Json::Bool(true)),
                    ("documentSymbolProvider", Json::Bool(true)),
                ]),
            ),
            (
                "serverInfo",
                Json::object([
                    ("name", Json::string("wave-insight-lsp")),
                    ("version", Json::string(env!("CARGO_PKG_VERSION"))),
                ]),
            ),
        ])
    }

    fn read_client_capabilities(&mut self, params: &Json) {
        let preferred = params
            .get("capabilities")
            .and_then(|capabilities| capabilities.get("textDocument"))
            .and_then(|text_document| text_document.get("hover"))
            .and_then(|hover| hover.get("contentFormat"))
            .and_then(Json::as_array)
            .and_then(|formats| formats.first())
            .and_then(Json::as_str);
        if let Some(preferred) = preferred {
            if preferred.eq_ignore_ascii_case("plaintext") {
                self.hover_format = HoverFormat::PlainText;
            } else if preferred.eq_ignore_ascii_case("markdown") {
                self.hover_format = HoverFormat::Markdown;
            }
        }
    }

    // -- document lifecycle ------------------------------------------------

    fn did_open(&mut self, params: &Json) {
        let Some(text_document) = params.get("textDocument") else {
            return;
        };
        let Some(uri) = text_document.get_str("uri") else {
            return;
        };
        let text = text_document.get_str("text").unwrap_or_default();
        let version = text_document.get_i64("version").unwrap_or(0);
        self.store_document(uri, version, text);
    }

    fn did_change(&mut self, params: &Json) {
        let Some(text_document) = params.get("textDocument") else {
            return;
        };
        let Some(uri) = text_document.get_str("uri") else {
            return;
        };
        let version = text_document.get_i64("version");
        let Some(index) = self.documents.iter().position(|document| document.uri == uri) else {
            // A change for a document that was never opened (or was closed);
            // reopening it from the change text is the least surprising repair.
            if let Some(text) = params
                .get_array("contentChanges")
                .and_then(|changes| changes.last())
                .and_then(|change| change.get_str("text"))
            {
                self.store_document(uri, version.unwrap_or(0), text);
            }
            return;
        };

        if let Some(changes) = params.get_array("contentChanges") {
            for change in changes {
                let Some(text) = change.get_str("text") else {
                    continue;
                };
                let range = change.get("range").and_then(Range::from_json);
                self.documents[index].apply_change(range, text);
            }
        }
        if let Some(version) = version {
            self.documents[index].version = version;
        }
        self.reparse(index);
    }

    fn did_save(&mut self, params: &Json) {
        let uri = params
            .get("textDocument")
            .and_then(|text_document| text_document.get_str("uri"));
        let Some(text) = params.get_str("text") else {
            return;
        };
        let Some(uri) = uri else {
            return;
        };
        if let Some(index) = self.documents.iter().position(|document| document.uri == uri) {
            self.documents[index].text = text.to_owned();
            self.reparse(index);
        } else {
            self.store_document(uri, 0, text);
        }
    }

    fn did_close(&mut self, params: &Json) {
        let uri = params
            .get("textDocument")
            .and_then(|text_document| text_document.get_str("uri"));
        let Some(uri) = uri else {
            return;
        };
        // The buffer goes away, the source stays in the design: closing an
        // editor tab does not remove a module from the design, and definition
        // targets in a closed file must keep resolving.
        self.documents.retain(|document| document.uri != uri);
    }

    fn store_document(&mut self, uri: &str, version: i64, text: &str) {
        let path = uri::to_path(uri);
        match self
            .documents
            .iter()
            .position(|document| document.uri == uri)
        {
            Some(index) => {
                self.documents[index] =
                    Document::new(uri, path.clone(), version, text);
            }
            None => self
                .documents
                .push(Document::new(uri, path.clone(), version, text)),
        }
        self.db.add_source(&path, text);
        self.design_dirty = true;
    }

    /// Re-parse the buffer at `index` into the design database.
    fn reparse(&mut self, index: usize) {
        let path = self.documents[index].path.clone();
        let text = std::mem::take(&mut self.documents[index].text);
        self.db.update_source(&path, &text);
        self.documents[index].text = text;
        self.design_dirty = true;
    }

    // -- requests ----------------------------------------------------------

    fn definition(&mut self, params: &Json) -> Result<Json, RpcError> {
        let Some(query) = self.resolve_position(params)? else {
            return Ok(Json::Null);
        };
        self.ensure_design();
        let Some(design) = self.design.as_ref() else {
            return Ok(Json::Null);
        };
        let item = self.item_at(design, &query);
        let span = match &item {
            Some(SourceItem::Driver { signal, .. }) | Some(SourceItem::Load { signal, .. }) => {
                design.declaration(signal).cloned()
            }
            Some(SourceItem::Signal { span, .. }) => Some(span.clone()),
            Some(SourceItem::Port { module, name, span }) => self
                .db
                .module(module)
                .and_then(|module| module.port(name))
                // In a non-ANSI header the cursor may sit on the name in the
                // port list; jump to the line that actually declares it.
                .map(|port| port.span.clone())
                .or_else(|| Some(span.clone())),
            Some(SourceItem::Module { name, .. }) => {
                self.db.module(name).map(|module| module.span.clone())
            }
            Some(SourceItem::Instance { module, .. }) => {
                self.db.module(module).map(|module| module.span.clone())
            }
            None => None,
        };
        let span = span.or_else(|| {
            // The cursor may be just outside the item's span; fall back to the
            // design's own "which signals cover this position" query.
            let paths = self
                .db
                .signals_at(design, &query.path, query.line, query.column);
            paths
                .first()
                .and_then(|path| design.declaration(path).cloned())
        });
        Ok(match span {
            Some(span) => self.location_json(&span),
            None => Json::Null,
        })
    }

    fn references(&mut self, params: &Json) -> Result<Json, RpcError> {
        let include_declaration = params
            .get("context")
            .and_then(|context| context.get_bool("includeDeclaration"))
            .unwrap_or(true);
        let Some(query) = self.resolve_position(params)? else {
            return Ok(Json::Array(Vec::new()));
        };
        self.ensure_design();
        let Some(design) = self.design.as_ref() else {
            return Ok(Json::Array(Vec::new()));
        };
        let item = self.item_at(design, &query);
        let spans: Vec<Span> = match &item {
            Some(SourceItem::Module { name, .. }) => {
                module_reference_spans(&self.db, name)
            }
            Some(SourceItem::Instance { module, .. }) => {
                module_reference_spans(&self.db, module)
            }
            Some(SourceItem::Driver { signal, .. }) | Some(SourceItem::Load { signal, .. }) => {
                signal_reference_spans(design, signal, include_declaration)
            }
            Some(SourceItem::Signal { module, name, .. })
            | Some(SourceItem::Port { module, name, .. }) => {
                match signal_path_by_local_name(design, module, name) {
                    Some(path) => signal_reference_spans(design, &path, include_declaration),
                    None => Vec::new(),
                }
            }
            None => {
                let paths = self
                    .db
                    .signals_at(design, &query.path, query.line, query.column);
                match paths.first() {
                    Some(path) => signal_reference_spans(design, path, include_declaration),
                    None => Vec::new(),
                }
            }
        };
        Ok(Json::Array(
            spans
                .iter()
                .map(|span| self.location_json(span))
                .collect::<Vec<Json>>(),
        ))
    }

    fn hover(&mut self, params: &Json) -> Result<Json, RpcError> {
        let Some(query) = self.resolve_position(params)? else {
            return Ok(Json::Null);
        };
        self.ensure_design();
        let Some(design) = self.design.as_ref() else {
            return Ok(Json::Null);
        };
        let item = self.item_at(design, &query);
        let format = self.hover_format;
        let (contents, span) = match &item {
            Some(SourceItem::Driver { signal, span, .. })
            | Some(SourceItem::Load { signal, span, .. }) => {
                (signal_hover(&self.db, design, signal, format), span.clone())
            }
            Some(SourceItem::Signal { module, name, span })
            | Some(SourceItem::Port { module, name, span }) => {
                // The item is module-local; the flat table turns it into a
                // hierarchical path (first instance of a shared module body).
                let text = signal_path_by_local_name(design, module, name)
                    .and_then(|path| signal_hover(&self.db, design, &path, format));
                (text, span.clone())
            }
            Some(SourceItem::Module { name, span }) => {
                (module_hover(&self.db, name, format), span.clone())
            }
            Some(SourceItem::Instance { path, span, .. }) => {
                (instance_hover(design, path, format), span.clone())
            }
            None => return Ok(Json::Null),
        };
        let Some(contents) = contents else {
            return Ok(Json::Null);
        };
        let range = self.range_for(&span);
        Ok(Json::object([
            (
                "contents",
                Json::object([
                    ("kind", Json::string(format.as_str())),
                    ("value", Json::string(contents)),
                ]),
            ),
            ("range", range.to_json()),
        ]))
    }

    fn document_symbol(&mut self, params: &Json) -> Result<Json, RpcError> {
        let uri = params
            .get("textDocument")
            .and_then(|text_document| text_document.get_str("uri"))
            .ok_or_else(|| RpcError::invalid_params("textDocument.uri is missing"))?;
        // Symbols are ranges, and ranges need the buffer text; a file that is
        // not open has no symbols to show.
        let Some(path) = self.document(uri).map(|document| document.path.clone()) else {
            return Ok(Json::Null);
        };
        let mut symbols = Vec::new();
        for module in self.db.modules() {
            if module.span.file != path {
                continue;
            }
            symbols.push(self.module_symbol(module));
        }
        Ok(Json::Array(symbols))
    }

    // -- helpers -----------------------------------------------------------

    /// Re-elaborate if a document changed since the last answer.
    fn ensure_design(&mut self) {
        if !self.design_dirty && self.design.is_some() {
            return;
        }
        self.design_dirty = false;
        self.design = None;
        let Some(top) = self.resolve_top() else {
            return;
        };
        self.design = Some(self.db.elaborate(&top));
    }

    /// The module to elaborate: the configured one, else the first module no
    /// other module instantiates, else the first module at all.
    fn resolve_top(&self) -> Option<String> {
        if let Some(top) = &self.top {
            if self.db.module(top).is_some() {
                return Some(top.clone());
            }
        }
        let instantiated: HashSet<&str> = self
            .db
            .modules()
            .iter()
            .flat_map(|module| module.instances.iter().map(|instance| instance.module.as_str()))
            .collect();
        self.db
            .modules()
            .iter()
            .find(|module| !instantiated.contains(module.name.as_str()))
            .or_else(|| self.db.modules().first())
            .map(|module| module.name.clone())
    }

    fn document(&self, uri: &str) -> Option<&Document> {
        self.documents
            .iter()
            .find(|document| document.uri == uri)
    }

    fn document_for_path(&self, path: &Path) -> Option<&Document> {
        self.documents
            .iter()
            .find(|document| document.path == path)
    }

    /// The URI to report for a database path: the client's own spelling when
    /// the document is open, a `file:` URI otherwise.
    fn uri_for_path(&self, path: &Path) -> String {
        self.document_for_path(path)
            .map(|document| document.uri.clone())
            .unwrap_or_else(|| uri::to_uri(path))
    }

    fn range_for(&self, span: &Span) -> Range {
        match self.document_for_path(&span.file) {
            Some(document) => document.span_range(span),
            None => approximate_span_range(span),
        }
    }

    fn location_json(&self, span: &Span) -> Json {
        Location::new(self.uri_for_path(&span.file), self.range_for(span)).to_json()
    }

    /// Decode `textDocument.uri` + `position` into database coordinates.
    ///
    /// `Ok(None)` means "answer with null": the document is not open, or the
    /// position is outside it.  `Err` is a malformed request.
    fn resolve_position(&self, params: &Json) -> Result<Option<Query>, RpcError> {
        let uri = params
            .get("textDocument")
            .and_then(|text_document| text_document.get_str("uri"))
            .ok_or_else(|| RpcError::invalid_params("textDocument.uri is missing"))?;
        let position = params
            .get("position")
            .and_then(Position::from_json)
            .ok_or_else(|| RpcError::invalid_params("position is missing or malformed"))?;
        let Some(document) = self.document(uri) else {
            return Ok(None);
        };
        let Some((line, column)) = document.source_position(position) else {
            return Ok(None);
        };
        Ok(Some(Query {
            path: document.path.clone(),
            line,
            column,
        }))
    }

    /// "What is under this position?" — the database answer, extended with
    /// module-instantiation names.
    ///
    /// [`DesignDb::item_at`] indexes declarations, references and (elaborated)
    /// instances; a cursor on the module name or on the instance name of an
    /// instantiation has no answer there, but both spans are part of the module
    /// model, so they are looked up here.
    fn item_at(&self, design: &ElaboratedDesign, query: &Query) -> Option<SourceItem> {
        self.db
            .item_at(design, &query.path, query.line, query.column)
            .or_else(|| instance_item_at(&self.db, design, &query.path, query.line, query.column))
    }

    fn module_symbol(&self, module: &ModuleDef) -> Json {
        let mut children = Vec::new();
        let mut spans = vec![module.span.clone()];

        for parameter in &module.parameters {
            spans.push(parameter.span.clone());
            let keyword = if parameter.is_local { "localparam" } else { "parameter" };
            let detail = match &parameter.default {
                Some(default) => format!("{keyword} = {default}"),
                None => keyword.to_owned(),
            };
            children.push(symbol_json(
                &parameter.name,
                detail,
                SYMBOL_CONSTANT,
                self.range_for(&parameter.span),
                self.range_for(&parameter.span),
                Vec::new(),
            ));
        }

        for port in &module.ports {
            spans.push(port.span.clone());
            let mut detail = format!("{} {}", port.direction.as_str(), port.kind.as_str());
            let packed = dimensions_text(&port.packed);
            if !packed.is_empty() {
                detail.push(' ');
                detail.push_str(&packed);
            }
            detail.push_str(&format!(" ({})", width_text(port.width)));
            children.push(symbol_json(
                &port.name,
                detail,
                SYMBOL_FIELD,
                self.range_for(&port.span),
                self.range_for(&port.span),
                Vec::new(),
            ));
        }

        for signal in &module.signals {
            spans.push(signal.span.clone());
            let mut detail = signal.kind.as_str().to_owned();
            let packed = dimensions_text(&signal.packed);
            if !packed.is_empty() {
                detail.push(' ');
                detail.push_str(&packed);
            }
            detail.push_str(&format!(" ({})", width_text(signal.width)));
            children.push(symbol_json(
                &signal.name,
                detail,
                SYMBOL_VARIABLE,
                self.range_for(&signal.span),
                self.range_for(&signal.span),
                Vec::new(),
            ));
        }

        for instance in &module.instances {
            spans.push(instance.span.clone());
            children.push(symbol_json(
                &instance.name,
                format!("instance of {}", instance.module),
                SYMBOL_OBJECT,
                self.range_for(&instance.span),
                self.range_for(&instance.span),
                Vec::new(),
            ));
        }

        // The module span is the name; the outline needs a range that also
        // covers the body, so the union of the name and every child is used.
        let range = union_span(&spans).unwrap_or_else(|| module.span.clone());
        symbol_json(
            &module.name,
            "module".to_owned(),
            SYMBOL_MODULE,
            self.range_for(&range),
            self.range_for(&module.span),
            children,
        )
    }
}

/// A position decoded into database coordinates.
#[derive(Clone, Debug)]
struct Query {
    path: PathBuf,
    /// 1-based, as the design database counts.
    line: u32,
    /// 1-based character column, as the design database counts.
    column: u32,
}

// ---------------------------------------------------------------------------
// Design-database based helpers (free functions so they can borrow `db` and
// `design` separately from the server)
// ---------------------------------------------------------------------------

/// Instance declarations are not part of [`DesignDb::item_at`]'s index; answer
/// for their two names (the instance name and the instantiated module name)
/// from the module model.
fn instance_item_at(
    db: &DesignDb,
    design: &ElaboratedDesign,
    file: &Path,
    line: u32,
    column: u32,
) -> Option<SourceItem> {
    let covers = |span: &Span| span.file.as_path() == file && span.contains(line, column);
    for module in db.modules() {
        for instance in &module.instances {
            if covers(&instance.span) {
                // An elaborated instance knows the hierarchical path; a module
                // that is not in the hierarchy still answers with its local
                // name so hover/definition keep working.
                let path = design
                    .instances()
                    .iter()
                    .find(|node| {
                        node.span.file == instance.span.file
                            && node.span.start_line == instance.span.start_line
                            && node.span.start_col == instance.span.start_col
                    })
                    .map(|node| node.path.clone())
                    .unwrap_or_else(|| instance.name.clone());
                return Some(SourceItem::Instance {
                    path,
                    module: instance.module.clone(),
                    span: instance.span.clone(),
                });
            }
            if covers(&instance.module_span) {
                return Some(SourceItem::Module {
                    name: instance.module.clone(),
                    span: instance.module_span.clone(),
                });
            }
        }
    }
    None
}

fn signal_path_by_local_name(
    design: &ElaboratedDesign,
    module: &str,
    name: &str,
) -> Option<String> {
    design
        .signals()
        .iter()
        .find(|signal| signal.module == module && signal.local_name == name)
        .map(|signal| signal.path.clone())
}

fn signal_reference_spans(
    design: &ElaboratedDesign,
    path: &str,
    include_declaration: bool,
) -> Vec<Span> {
    let mut spans = Vec::new();
    if include_declaration {
        if let Some(declaration) = design.declaration(path) {
            spans.push(declaration.clone());
        }
    }
    spans.extend(design.drivers(path).iter().map(|driver| driver.span.clone()));
    spans.extend(design.loads(path).iter().map(|load| load.span.clone()));
    normalise_spans(spans)
}

fn module_reference_spans(db: &DesignDb, name: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    if let Some(module) = db.module(name) {
        spans.push(module.span.clone());
    }
    for module in db.modules() {
        for instance in &module.instances {
            if instance.module == name {
                spans.push(instance.module_span.clone());
            }
        }
    }
    normalise_spans(spans)
}

/// Sort by position and drop duplicates (a signal can carry both a driver and
/// a load reference at one span, e.g. an `inout` port connection).
fn normalise_spans(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then(left.start_line.cmp(&right.start_line))
            .then(left.start_col.cmp(&right.start_col))
            .then(left.end_line.cmp(&right.end_line))
            .then(left.end_col.cmp(&right.end_col))
    });
    spans.dedup_by(|left, right| {
        left.file == right.file
            && left.start_line == right.start_line
            && left.start_col == right.start_col
    });
    spans
}

fn dimensions_text(dimensions: &[RangeDim]) -> String {
    dimensions
        .iter()
        .map(RangeDim::text)
        .collect::<Vec<String>>()
        .join(" ")
}

fn width_text(width: Option<u32>) -> String {
    match width {
        Some(width) => format!("{width} bits"),
        None => "width unresolved".to_owned(),
    }
}

/// The source declaration of one signal, rebuilt from the model (the database
/// keeps spans, not text).
fn declaration_text(db: &DesignDb, signal: &FlatSignal) -> String {
    let mut text = String::new();
    if let Some(direction) = signal.direction {
        text.push_str(direction.as_str());
        text.push(' ');
    }
    text.push_str(signal.kind.as_str());
    let dimensions = db
        .module(&signal.module)
        .map(|module| {
            if let Some(port) = module.port(&signal.local_name) {
                port.packed.clone()
            } else if let Some(definition) = module.signal(&signal.local_name) {
                definition.packed.clone()
            } else {
                Vec::new()
            }
        })
        .unwrap_or_default();
    let dimensions = dimensions_text(&dimensions);
    if !dimensions.is_empty() {
        text.push(' ');
        text.push_str(&dimensions);
    }
    text.push(' ');
    text.push_str(&signal.local_name);
    text
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Hover card for a flattened signal, with the declaration, module, width,
/// direction and the driver/load counts.
fn signal_hover(
    db: &DesignDb,
    design: &ElaboratedDesign,
    path: &str,
    format: HoverFormat,
) -> Option<String> {
    let signal = design.signal(path)?;
    let declaration = declaration_text(db, signal);
    let width = width_text(signal.width);
    let location = format!(
        "{}:{}:{}",
        file_name(&signal.declaration.file),
        signal.declaration.start_line,
        signal.declaration.start_col
    );
    let siblings: Vec<&str> = design
        .signals()
        .iter()
        .filter(|other| {
            other.module == signal.module && other.local_name == signal.local_name
        })
        .map(|other| other.path.as_str())
        .collect();

    let mut lines: Vec<String> = Vec::new();
    match format {
        HoverFormat::Markdown => {
            lines.push(format!("```verilog\n{declaration}\n```"));
            lines.push(String::new());
            lines.push(format!("`{path}` in module `{}`", signal.module));
            lines.push(String::new());
            lines.push(format!("- width: {width}"));
            if let Some(direction) = signal.direction {
                lines.push(format!("- direction: {}", direction.as_str()));
            }
            lines.push(format!("- kind: {}", signal.kind.as_str()));
            lines.push(format!(
                "- drivers: {}, loads: {}",
                signal.drivers.len(),
                signal.loads.len()
            ));
            lines.push(format!("- declared at `{location}`"));
            if siblings.len() > 1 {
                lines.push(format!(
                    "- also instantiated as {}",
                    siblings
                        .iter()
                        .map(|sibling| format!("`{sibling}`"))
                        .collect::<Vec<String>>()
                        .join(", ")
                ));
            }
        }
        HoverFormat::PlainText => {
            lines.push(declaration);
            lines.push(String::new());
            lines.push(format!("{path} in module {}", signal.module));
            lines.push(format!("width: {width}"));
            if let Some(direction) = signal.direction {
                lines.push(format!("direction: {}", direction.as_str()));
            }
            lines.push(format!("kind: {}", signal.kind.as_str()));
            lines.push(format!(
                "drivers: {}, loads: {}",
                signal.drivers.len(),
                signal.loads.len()
            ));
            lines.push(format!("declared at {location}"));
            if siblings.len() > 1 {
                lines.push(format!("also instantiated as {}", siblings.join(", ")));
            }
        }
    }
    Some(lines.join("\n"))
}

fn module_hover(db: &DesignDb, name: &str, format: HoverFormat) -> Option<String> {
    let module = db.module(name)?;
    let instantiations = db
        .modules()
        .iter()
        .flat_map(|module| module.instances.iter())
        .filter(|instance| instance.module == name)
        .count();
    let location = format!(
        "{}:{}:{}",
        file_name(&module.span.file),
        module.span.start_line,
        module.span.start_col
    );
    let summary = format!(
        "{} ports, {} parameters, {} signals, {} instances",
        module.ports.len(),
        module.parameters.len(),
        module.signals.len(),
        module.instances.len()
    );
    let mut lines = Vec::new();
    match format {
        HoverFormat::Markdown => {
            lines.push(format!("**module `{name}`**"));
            lines.push(String::new());
            lines.push(format!("- {summary}"));
            lines.push(format!("- installed {instantiations} time(s)"));
            lines.push(format!("- declared at `{location}`"));
        }
        HoverFormat::PlainText => {
            lines.push(format!("module {name}"));
            lines.push(String::new());
            lines.push(summary);
            lines.push(format!("instantiated {instantiations} time(s)"));
            lines.push(format!("declared at {location}"));
        }
    }
    Some(lines.join("\n"))
}

fn instance_hover(
    design: &ElaboratedDesign,
    path: &str,
    format: HoverFormat,
) -> Option<String> {
    let instance = design.instance(path)?;
    let parameters = instance
        .parameters
        .iter()
        .map(|parameter| match (&parameter.text, parameter.value) {
            (Some(text), _) => format!("{}={text}", parameter.name),
            (None, Some(value)) => format!("{}={value}", parameter.name),
            _ => parameter.name.clone(),
        })
        .collect::<Vec<String>>()
        .join(", ");
    let mut lines = Vec::new();
    match format {
        HoverFormat::Markdown => {
            lines.push(format!("**instance `{path}`**"));
            lines.push(String::new());
            lines.push(format!("- module: `{}`", instance.module));
            lines.push(format!("- ports: {}", instance.ports.len()));
            if !parameters.is_empty() {
                lines.push(format!("- parameters: `{parameters}`"));
            }
        }
        HoverFormat::PlainText => {
            lines.push(format!("instance {path}"));
            lines.push(String::new());
            lines.push(format!("module: {}", instance.module));
            lines.push(format!("ports: {}", instance.ports.len()));
            if !parameters.is_empty() {
                lines.push(format!("parameters: {parameters}"));
            }
        }
    }
    Some(lines.join("\n"))
}

fn symbol_json(
    name: &str,
    detail: String,
    kind: i64,
    range: Range,
    selection_range: Range,
    children: Vec<Json>,
) -> Json {
    Json::object([
        ("name", Json::string(name)),
        ("detail", Json::string(detail)),
        ("kind", Json::Integer(kind)),
        ("range", range.to_json()),
        ("selectionRange", selection_range.to_json()),
        ("children", Json::Array(children)),
    ])
}

/// Smallest span covering all inputs (they all live in one file).
fn union_span(spans: &[Span]) -> Option<Span> {
    let mut iterator = spans.iter();
    let first = iterator.next()?;
    let mut start = (first.start_line, first.start_col);
    let mut end = (first.end_line, first.end_col);
    for span in iterator {
        if (span.start_line, span.start_col) < start {
            start = (span.start_line, span.start_col);
        }
        if (span.end_line, span.end_col) > end {
            end = (span.end_line, span.end_col);
        }
    }
    Some(Span::new(
        first.file.clone(),
        start.0,
        start.1,
        end.0,
        end.1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("data")
            .join(name)
    }

    fn fixture_text(name: &str) -> String {
        std::fs::read_to_string(fixture(name))
            .unwrap_or_else(|error| panic!("fixture `{name}` should be readable: {error}"))
    }

    /// 0-based line and UTF-16 column of the first occurrence of `needle`.
    fn position_of(text: &str, needle: &str) -> Position {
        let offset = text
            .find(needle)
            .unwrap_or_else(|| panic!("`{needle}` not found in the fixture"));
        let line = text[..offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count() as u32;
        let line_start = text[..offset]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let character = text[line_start..offset]
            .chars()
            .map(|character| character.len_utf16() as u32)
            .sum();
        Position::new(line, character)
    }

    fn empty_object() -> Json {
        Json::Object(Vec::new())
    }

    fn send(server: &mut LspServer, message: Json) -> Vec<Json> {
        server
            .handle_message(&message.to_text())
            .iter()
            .map(|reply| {
                Json::parse(reply)
                    .unwrap_or_else(|error| panic!("server reply is not JSON ({error}): {reply}"))
            })
            .collect()
    }

    fn request(server: &mut LspServer, method: &str, params: Json) -> Json {
        let replies = send(
            server,
            Json::object([
                ("jsonrpc", Json::string("2.0")),
                ("id", Json::int(1)),
                ("method", Json::string(method)),
                ("params", params),
            ]),
        );
        assert_eq!(replies.len(), 1, "a request gets one reply, got {replies:?}");
        if let Some(error) = replies[0].get("error") {
            panic!("`{method}` failed: {error:?}");
        }
        replies[0].get("result").cloned().unwrap_or(Json::Null)
    }

    fn request_error(server: &mut LspServer, method: &str, params: Json) -> Json {
        let replies = send(
            server,
            Json::object([
                ("jsonrpc", Json::string("2.0")),
                ("id", Json::int(1)),
                ("method", Json::string(method)),
                ("params", params),
            ]),
        );
        assert_eq!(replies.len(), 1, "a request gets one reply, got {replies:?}");
        replies[0]
            .get("error")
            .cloned()
            .unwrap_or_else(|| panic!("expected an error reply, got {replies:?}"))
    }

    fn notify(server: &mut LspServer, method: &str, params: Json) {
        let replies = send(
            server,
            Json::object([
                ("jsonrpc", Json::string("2.0")),
                ("method", Json::string(method)),
                ("params", params),
            ]),
        );
        assert!(replies.is_empty(), "notifications have no reply: {replies:?}");
    }

    fn initialize(server: &mut LspServer) -> Json {
        request(
            server,
            "initialize",
            Json::object([
                ("processId", Json::Null),
                ("capabilities", empty_object()),
            ]),
        )
    }

    fn did_open(server: &mut LspServer, uri: &str, text: &str) {
        notify(
            server,
            "textDocument/didOpen",
            Json::object([(
                "textDocument",
                Json::object([
                    ("uri", Json::string(uri)),
                    ("languageId", Json::string("systemverilog")),
                    ("version", Json::int(1)),
                    ("text", Json::string(text)),
                ]),
            )]),
        );
    }

    fn open_fixture(server: &mut LspServer, name: &str) -> String {
        let uri = uri::to_uri(&fixture(name));
        did_open(server, &uri, &fixture_text(name));
        uri
    }

    /// A server with `top.sv` and `cpu.sv` open, initialized.
    fn fixture_server() -> (LspServer, String, String) {
        let mut server = LspServer::new();
        initialize(&mut server);
        let top_uri = open_fixture(&mut server, "top.sv");
        let cpu_uri = open_fixture(&mut server, "cpu.sv");
        (server, top_uri, cpu_uri)
    }

    fn position_params(uri: &str, position: Position) -> Json {
        Json::object([
            ("textDocument", Json::object([("uri", Json::string(uri))])),
            ("position", position.to_json()),
        ])
    }

    fn document_params(uri: &str) -> Json {
        Json::object([("textDocument", Json::object([("uri", Json::string(uri))]))])
    }

    fn decode_location(value: &Json) -> Location {
        let uri = value
            .get_str("uri")
            .unwrap_or_else(|| panic!("location without a `uri`: {value:?}"));
        let range = value
            .get("range")
            .and_then(Range::from_json)
            .unwrap_or_else(|| panic!("location without a `range`: {value:?}"));
        Location::new(uri, range)
    }

    fn decode_locations(value: &Json) -> Vec<Location> {
        value
            .as_array()
            .unwrap_or_else(|| panic!("expected an array of locations, got {value:?}"))
            .iter()
            .map(decode_location)
            .collect()
    }

    fn child_names(symbol: &Json) -> Vec<String> {
        symbol
            .get_array("children")
            .unwrap_or(&[])
            .iter()
            .filter_map(|child| child.get_str("name").map(str::to_owned))
            .collect()
    }

    // -- definition --------------------------------------------------------

    #[test]
    fn definition_resolves_references_declarations_and_module_names() {
        let (mut server, top_uri, cpu_uri) = fixture_server();
        let cpu = fixture_text("cpu.sv");
        let top = fixture_text("top.sv");

        // On the declaration: the answer is the declaration itself.
        let declaration = position_of(&cpu, "pc_reg;");
        let location = decode_location(&request(
            &mut server,
            "textDocument/definition",
            position_params(&cpu_uri, declaration),
        ));
        assert_eq!(location.uri, cpu_uri);
        assert_eq!(location.range.start, declaration);

        // On a load of `pc_reg` inside `assign next_pc = pc_reg + 1'b1;`.
        let load = position_of(&cpu, "pc_reg + 1'b1");
        let location = decode_location(&request(
            &mut server,
            "textDocument/definition",
            position_params(&cpu_uri, load),
        ));
        assert_eq!(location.uri, cpu_uri);
        assert_eq!(location.range.start, declaration);

        // On the module name of an instantiation: the module definition.
        let module_reference = position_of(&top, "cpu #(.WIDTH(8)");
        let location = decode_location(&request(
            &mut server,
            "textDocument/definition",
            position_params(&top_uri, module_reference),
        ));
        assert_eq!(location.uri, cpu_uri);
        assert_eq!(
            location.range.start.line, 1,
            "`module cpu #(` is on the second line of cpu.sv"
        );

        // On the instance name of an instantiation: the module it instances.
        let instance_name = position_of(&top, "u_cpu (");
        let location = decode_location(&request(
            &mut server,
            "textDocument/definition",
            position_params(&top_uri, instance_name),
        ));
        assert_eq!(location.uri, cpu_uri);
        assert_eq!(location.range.start.line, 1);

        // On a plain load in top.sv: the declaration of `data_out` (line 10).
        // The needle starts at `=`, so the identifier is two characters on.
        let assignment = position_of(&top, "= data_out;");
        let location = decode_location(&request(
            &mut server,
            "textDocument/definition",
            position_params(
                &top_uri,
                Position::new(assignment.line, assignment.character + 2),
            ),
        ));
        assert_eq!(location.uri, top_uri);
        assert_eq!(location.range.start.line, 9);
        assert_eq!(
            location.range.start.character,
            position_of(&top, "data_out;").character
        );
    }

    // -- references --------------------------------------------------------

    #[test]
    fn references_returns_the_declaration_and_every_driver_and_load() {
        let (mut server, top_uri, cpu_uri) = fixture_server();
        let cpu = fixture_text("cpu.sv");
        let declaration = position_of(&cpu, "pc_reg;");

        let locations = decode_locations(&request(
            &mut server,
            "textDocument/references",
            Json::object([
                ("textDocument", Json::object([("uri", Json::string(&cpu_uri))])),
                ("position", declaration.to_json()),
                (
                    "context",
                    Json::object([("includeDeclaration", Json::Bool(true))]),
                ),
            ]),
        ));
        assert!(locations.iter().all(|location| location.uri == cpu_uri));
        let lines: Vec<u32> = locations
            .iter()
            .map(|location| location.range.start.line)
            .collect();
        // Declaration (line 13), the two loads (19, 20) and the two drivers
        // (25, 27), all 0-based.
        assert_eq!(lines, vec![12, 18, 19, 24, 26], "{locations:?}");

        // Without the declaration only the references remain.
        let locations = decode_locations(&request(
            &mut server,
            "textDocument/references",
            Json::object([
                ("textDocument", Json::object([("uri", Json::string(&cpu_uri))])),
                ("position", declaration.to_json()),
                (
                    "context",
                    Json::object([("includeDeclaration", Json::Bool(false))]),
                ),
            ]),
        ));
        assert_eq!(locations.len(), 4);

        // On a module name, references are the definition and every
        // instantiation of it (top.sv instantiates cpu twice).
        let top = fixture_text("top.sv");
        let locations = decode_locations(&request(
            &mut server,
            "textDocument/references",
            Json::object([
                ("textDocument", Json::object([("uri", Json::string(&top_uri))])),
                ("position", position_of(&top, "cpu #(.WIDTH(8)").to_json()),
                (
                    "context",
                    Json::object([("includeDeclaration", Json::Bool(true))]),
                ),
            ]),
        ));
        assert_eq!(locations.len(), 3, "{locations:?}");
        assert_eq!(
            locations
                .iter()
                .filter(|location| location.uri == cpu_uri)
                .count(),
            1
        );
        assert_eq!(
            locations
                .iter()
                .filter(|location| location.uri == top_uri)
                .count(),
            2
        );
    }

    // -- hover -------------------------------------------------------------

    #[test]
    fn hover_shows_the_declaration_width_direction_and_instances() {
        let (mut server, top_uri, cpu_uri) = fixture_server();
        let cpu = fixture_text("cpu.sv");

        let hover = request(
            &mut server,
            "textDocument/hover",
            position_params(&cpu_uri, position_of(&cpu, "pc_reg;")),
        );
        let contents = hover.get("contents").expect("hover contents");
        assert_eq!(contents.get_str("kind"), Some("markdown"));
        let value = contents.get_str("value").expect("markdown value");
        assert!(value.contains("reg [WIDTH-1:0] pc_reg"), "{value}");
        assert!(value.contains("width: 8 bits"), "{value}");
        assert!(value.contains("drivers: 2, loads: 2"), "{value}");
        assert!(value.contains("top.u_cpu.pc_reg"), "{value}");
        assert!(
            value.contains("top.u_cpu16.pc_reg"),
            "hover should list the sibling instance: {value}"
        );
        // The card is anchored at the declaration the client asked about.
        let range =
            Range::from_json(hover.get("range").expect("hover range")).expect("valid hover range");
        assert_eq!(range.start, position_of(&cpu, "pc_reg;"));

        // A port gets its direction; `instr` is 8 bits wide in cpu.sv.
        let hover = request(
            &mut server,
            "textDocument/hover",
            position_params(&cpu_uri, position_of(&cpu, "instr,")),
        );
        let value = hover
            .get("contents")
            .and_then(|contents| contents.get_str("value"))
            .expect("hover value for the port");
        assert!(value.contains("direction: input"), "{value}");
        assert!(value.contains("width: 8 bits"), "{value}");
        assert!(value.contains("input logic [WIDTH-1:0] instr"), "{value}");

        // A module name answers for the module.
        let top = fixture_text("top.sv");
        let hover = request(
            &mut server,
            "textDocument/hover",
            position_params(&top_uri, position_of(&top, "top (")),
        );
        let value = hover
            .get("contents")
            .and_then(|contents| contents.get_str("value"))
            .expect("hover value for the module");
        assert!(value.contains("module `top`"), "{value}");
        assert!(value.contains("4 ports"), "{value}");

        // An instance name answers for the instance, with its hierarchical path.
        let hover = request(
            &mut server,
            "textDocument/hover",
            position_params(&top_uri, position_of(&top, "u_cpu (")),
        );
        let value = hover
            .get("contents")
            .and_then(|contents| contents.get_str("value"))
            .expect("hover value for the instance");
        assert!(value.contains("instance `top.u_cpu`"), "{value}");
        assert!(value.contains("module: `cpu`"), "{value}");
    }

    #[test]
    fn hover_follows_the_client_markup_preference() {
        let mut server = LspServer::new();
        request(
            &mut server,
            "initialize",
            Json::object([(
                "capabilities",
                Json::object([(
                    "textDocument",
                    Json::object([(
                        "hover",
                        Json::object([(
                            "contentFormat",
                            Json::array([Json::string("plaintext"), Json::string("markdown")]),
                        )]),
                    )]),
                )]),
            )]),
        );
        assert_eq!(server.hover_format(), HoverFormat::PlainText);
        let cpu_uri = open_fixture(&mut server, "cpu.sv");
        let hover = request(
            &mut server,
            "textDocument/hover",
            position_params(&cpu_uri, position_of(&fixture_text("cpu.sv"), "instr,")),
        );
        let contents = hover.get("contents").expect("hover contents");
        assert_eq!(contents.get_str("kind"), Some("plaintext"));
        let value = contents.get_str("value").expect("plaintext value");
        assert!(!value.contains("```"), "{value}");
        assert!(value.contains("width: 8 bits"), "{value}");
        assert!(value.contains("direction: input"), "{value}");
    }

    // -- document symbols --------------------------------------------------

    #[test]
    fn document_symbol_lists_modules_parameters_ports_signals_and_instances() {
        let (mut server, top_uri, cpu_uri) = fixture_server();

        let symbols = request(
            &mut server,
            "textDocument/documentSymbol",
            document_params(&top_uri),
        );
        let symbols = symbols.as_array().expect("symbol array").to_vec();
        assert_eq!(symbols.len(), 1);
        let top = &symbols[0];
        assert_eq!(top.get_str("name"), Some("top"));
        assert_eq!(top.get_str("detail"), Some("module"));
        assert_eq!(top.get_i64("kind"), Some(SYMBOL_MODULE));
        let range = top
            .get("range")
            .and_then(Range::from_json)
            .expect("module range");
        let selection = top
            .get("selectionRange")
            .and_then(Range::from_json)
            .expect("valid selection range");
        assert!(range.start.line <= selection.start.line);
        let names = child_names(top);
        for expected in [
            "clk", "reset", "instr", "result", "pc", "data_out", "pc16", "u_cpu", "u_cpu16",
        ] {
            assert!(
                names.contains(&expected.to_owned()),
                "missing `{expected}` in {names:?}"
            );
        }

        let symbols = request(
            &mut server,
            "textDocument/documentSymbol",
            document_params(&cpu_uri),
        );
        let symbols = symbols.as_array().expect("symbol array").to_vec();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].get_str("name"), Some("cpu"));
        let names = child_names(&symbols[0]);
        for expected in [
            "WIDTH", "DEPTH", "clk", "reset", "instr", "pc", "data_out", "next_pc", "pc_reg",
            "active",
        ] {
            assert!(
                names.contains(&expected.to_owned()),
                "missing `{expected}` in {names:?}"
            );
        }
        // Ports and signals carry a detail string the outline can show.
        let details: Vec<&str> = symbols[0]
            .get_array("children")
            .expect("children")
            .iter()
            .filter_map(|child| child.get_str("detail"))
            .collect();
        assert!(
            details
                .iter()
                .any(|detail| detail.contains("input logic [WIDTH-1:0] (8 bits)")),
            "{details:?}"
        );

        // An unknown document has no symbols to show.
        let unknown = uri::to_uri(&fixture("vectors.sv"));
        assert_eq!(
            request(
                &mut server,
                "textDocument/documentSymbol",
                document_params(&unknown)
            ),
            Json::Null
        );
    }

    // -- document lifecycle ------------------------------------------------

    #[test]
    fn documents_follow_the_editor_buffer() {
        let mut server = LspServer::new();
        initialize(&mut server);
        let uri = uri::to_uri(&fixture("vectors.sv"));

        did_open(
            &mut server,
            &uri,
            "module demo(input logic a);\n    logic b;\nendmodule\n",
        );
        let symbols = request(&mut server, "textDocument/documentSymbol", document_params(&uri));
        let symbols = symbols.as_array().expect("symbols").to_vec();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].get_str("name"), Some("demo"));
        assert_eq!(
            child_names(&symbols[0]),
            vec!["a".to_owned(), "b".to_owned()]
        );

        // Incremental change: rename the module.
        notify(
            &mut server,
            "textDocument/didChange",
            Json::object([
                (
                    "textDocument",
                    Json::object([("uri", Json::string(&uri)), ("version", Json::int(2))]),
                ),
                (
                    "contentChanges",
                    Json::array([Json::object([
                        (
                            "range",
                            Range::new(Position::new(0, 7), Position::new(0, 11)).to_json(),
                        ),
                        ("text", Json::string("renamed")),
                    ])]),
                ),
            ]),
        );
        let symbols = request(&mut server, "textDocument/documentSymbol", document_params(&uri));
        assert_eq!(
            symbols.as_array().expect("symbols")[0].get_str("name"),
            Some("renamed")
        );

        // Full change: the buffer is replaced and the old module is gone.
        notify(
            &mut server,
            "textDocument/didChange",
            Json::object([
                (
                    "textDocument",
                    Json::object([("uri", Json::string(&uri)), ("version", Json::int(3))]),
                ),
                (
                    "contentChanges",
                    Json::array([Json::object([(
                        "text",
                        Json::string("module leaf;\nendmodule\n"),
                    )])]),
                ),
            ]),
        );
        let symbols = request(&mut server, "textDocument/documentSymbol", document_params(&uri));
        let symbols = symbols.as_array().expect("symbols");
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].get_str("name"), Some("leaf"));
        assert!(child_names(&symbols[0]).is_empty());

        // A position on the `module` keyword is not covered by a span.
        assert_eq!(
            request(
                &mut server,
                "textDocument/definition",
                position_params(&uri, Position::new(0, 0))
            ),
            Json::Null
        );
    }

    #[test]
    fn queries_for_a_document_that_is_not_open_answer_null() {
        let mut server = LspServer::new();
        initialize(&mut server);
        let unknown = uri::to_uri(&fixture("legacy.v"));
        assert_eq!(
            request(
                &mut server,
                "textDocument/hover",
                position_params(&unknown, Position::new(0, 0))
            ),
            Json::Null
        );
        assert!(decode_locations(&request(
            &mut server,
            "textDocument/references",
            Json::object([
                ("textDocument", Json::object([("uri", Json::string(&unknown))])),
                ("position", Position::new(0, 0).to_json()),
                ("context", empty_object()),
            ]),
        ))
        .is_empty());
        assert_eq!(
            request(
                &mut server,
                "textDocument/definition",
                position_params(&unknown, Position::new(0, 0))
            ),
            Json::Null
        );
    }

    // -- protocol and lifecycle --------------------------------------------

    #[test]
    fn protocol_errors_and_shutdown_are_reported() {
        let mut server = LspServer::new();
        let error = request_error(
            &mut server,
            "textDocument/hover",
            position_params("file:///tmp/x.sv", Position::new(0, 0)),
        );
        assert_eq!(
            error.get_i64("code"),
            Some(i64::from(protocol::SERVER_NOT_INITIALIZED)),
            "{error:?}"
        );

        let initialize_result = initialize(&mut server);
        let capabilities = initialize_result.get("capabilities").expect("capabilities");
        assert_eq!(capabilities.get_str("positionEncoding"), Some("utf-16"));
        assert_eq!(capabilities.get_bool("definitionProvider"), Some(true));
        assert_eq!(capabilities.get_bool("referencesProvider"), Some(true));
        assert_eq!(capabilities.get_bool("hoverProvider"), Some(true));
        assert_eq!(capabilities.get_bool("documentSymbolProvider"), Some(true));
        assert_eq!(
            initialize_result
                .get("serverInfo")
                .and_then(|info| info.get_str("name")),
            Some("wave-insight-lsp")
        );

        // A second initialize is a protocol error.
        let error = request_error(&mut server, "initialize", empty_object());
        assert_eq!(
            error.get_i64("code"),
            Some(i64::from(protocol::INVALID_REQUEST))
        );

        let error = request_error(
            &mut server,
            "textDocument/completion",
            document_params("file:///tmp/x.sv"),
        );
        assert_eq!(
            error.get_i64("code"),
            Some(i64::from(protocol::METHOD_NOT_FOUND))
        );

        // Malformed JSON becomes a parse error with a null id.
        let replies = server.handle_message("{");
        assert_eq!(replies.len(), 1);
        let reply = Json::parse(&replies[0]).expect("parse error reply is JSON");
        assert_eq!(reply.get("id"), Some(&Json::Null));
        assert_eq!(
            reply.get("error").and_then(|error| error.get_i64("code")),
            Some(i64::from(protocol::PARSE_ERROR))
        );

        assert_eq!(request(&mut server, "shutdown", Json::Null), Json::Null);
        assert!(server.shutdown_requested());
        let error = request_error(
            &mut server,
            "textDocument/hover",
            position_params("file:///tmp/x.sv", Position::new(0, 0)),
        );
        assert_eq!(
            error.get_i64("code"),
            Some(i64::from(protocol::INVALID_REQUEST))
        );

        notify(&mut server, "exit", Json::Null);
        assert!(server.exit_requested());
    }

    #[test]
    fn the_single_message_entry_point_returns_one_reply_or_nothing() {
        let mut server = LspServer::new();
        let reply = server.handle_message_text(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        );
        assert!(reply.contains("wave-insight-lsp"), "{reply}");
        let reply =
            server.handle_message_text(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
        assert!(reply.is_empty(), "notifications have no reply: {reply}");
    }

    #[test]
    fn a_preloaded_database_can_be_elaborated_from_a_configured_top() {
        let db = DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv")]);
        let mut server = LspServer::with_db(db);
        let design = server.design().expect("automatic top elaboration");
        assert_eq!(design.top(), "top");
        assert!(design.signal("top.u_cpu.pc").is_some());

        server.set_top(Some("cpu".to_owned()));
        let design = server.design().expect("explicit top elaboration");
        assert_eq!(design.top(), "cpu");
        assert!(design.instance("cpu").is_some());
        assert!(design.signal("cpu.pc").is_some());
        assert!(design.signal("top.u_cpu.pc").is_none());
    }
}

