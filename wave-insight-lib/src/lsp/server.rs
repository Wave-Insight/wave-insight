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
//! | `initialize` | capabilities for the features below, `positionEncoding: utf-16` |
//! | `shutdown` / `exit` | lifecycle, as specified |
//! | `textDocument/definition` | declaration of the signal under the cursor, or the definition of the module name |
//! | `textDocument/references` | declaration (optional) + every driver/load span, or every instantiation of a module |
//! | `textDocument/hover` | short markdown/plaintext card: declaration, module, width, direction, driver/load counts |
//! | `textDocument/documentSymbol` | modules with parameter/port/signal/instance children |
//! | `textDocument/completion` | identifiers visible at the cursor: ports, signals (generate-scoped too), parameters/localparams, genvars, instances and every module name; a prefix filters case-insensitively and `instance.`/`a.b.` returns that module's members |
//!
//! Client notifications: `initialized`, `textDocument/didOpen`, `didChange`,
//! `didSave`, `didClose`.  After every document notification the server queues a
//! `textDocument/publishDiagnostics` for that document merging the design
//! database diagnostics (parse errors, skipped constructs) with the elaboration
//! diagnostics (unresolved names, width mismatches, non-constant generate
//! bounds); `set_top` republishes every open document because a different root
//! can change those warnings.  The queue is drained with
//! [`LspServer::take_notifications`].
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
    DesignDb, Diagnostic, ElaboratedDesign, FlatSignal, ModuleDef, RangeDim, Severity,
    SignalReference, SourceItem, Span,
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
    /// Server-initiated notifications queued while handling a message
    /// (today: `textDocument/publishDiagnostics`).  Replies stay the return
    /// value of [`LspServer::handle_message`]; notifications are drained with
    /// [`LspServer::take_notifications`].
    notifications: Vec<String>,
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
            notifications: Vec::new(),
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
    ///
    /// Changing the root can change every elaboration diagnostic (unresolved
    /// names, width mismatches, ...), so the diagnostics of every open document
    /// are republished.
    pub fn set_top(&mut self, top: Option<String>) {
        self.top = top;
        self.design_dirty = true;
        self.publish_all_document_diagnostics();
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

    /// Take the server-initiated notifications queued by the messages handled
    /// so far.
    ///
    /// [`LspServer::handle_message`] keeps its contract (it returns the *replies*
    /// to the client); notifications the server wants to push, currently
    /// `textDocument/publishDiagnostics` after `didOpen`/`didChange`/`didSave`
    /// and an empty diagnostics array on `didClose`, are queued here instead.
    /// Both transports drain this after every message: the stdio loop in
    /// `back-end` forwards them to the client, the wasm shim hands them to the
    /// browser client.  A host that never drains simply does not see
    /// diagnostics; nothing else changes.
    pub fn take_notifications(&mut self) -> Vec<String> {
        std::mem::take(&mut self.notifications)
    }

    /// Number of queued notifications, for hosts that poll and for tests.
    pub fn pending_notifications(&self) -> usize {
        self.notifications.len()
    }

    // -- transport-independent entry points --------------------------------

    /// Handle one JSON-RPC message and return the replies.
    ///
    /// A request produces exactly one response; a notification produces none.
    /// Server-initiated notifications (diagnostics) are not replies: they are
    /// queued and returned by [`LspServer::take_notifications`].  Draining is
    /// not required for the server to work, but a transport that never drains
    /// starves the editor of diagnostics.
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
    /// Notifications the server produces are queued; the wasm shim returns them
    /// through [`LspServer::take_notifications`].
    pub fn handle_message_text(&mut self, message: &str) -> String {
        self.handle_message(message)
            .into_iter()
            .next()
            .unwrap_or_default()
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
                    return Err(RpcError::invalid_request(
                        "`initialize` was already received",
                    ));
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
            "textDocument/completion" => self.completion(params),
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
                    (
                        // Signal/instance/module completion; "." (hierarchical
                        // names) triggers client-side, the server returns its
                        // whole visible set and lets the client filter.
                        "completionProvider",
                        Json::object([
                            ("resolveProvider", Json::Bool(false)),
                            ("triggerCharacters", Json::Array(vec![Json::string(".")])),
                        ]),
                    ),
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
        self.publish_document_diagnostics(uri);
    }

    fn did_change(&mut self, params: &Json) {
        let Some(text_document) = params.get("textDocument") else {
            return;
        };
        let Some(uri) = text_document.get_str("uri") else {
            return;
        };
        let version = text_document.get_i64("version");
        let Some(index) = self
            .documents
            .iter()
            .position(|document| document.uri == uri)
        else {
            // A change for a document that was never opened (or was closed);
            // reopening it from the change text is the least surprising repair.
            if let Some(text) = params
                .get_array("contentChanges")
                .and_then(|changes| changes.last())
                .and_then(|change| change.get_str("text"))
            {
                self.store_document(uri, version.unwrap_or(0), text);
                self.publish_document_diagnostics(uri);
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
        self.publish_document_diagnostics(uri);
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
        if let Some(index) = self
            .documents
            .iter()
            .position(|document| document.uri == uri)
        {
            self.documents[index].text = text.to_owned();
            self.reparse(index);
        } else {
            self.store_document(uri, 0, text);
        }
        self.publish_document_diagnostics(uri);
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
        // targets in a closed file must keep resolving.  The editor still has to
        // drop the markers of the tab, so an empty diagnostics array is pushed
        // with the version the document had.
        let version = self
            .documents
            .iter()
            .find(|document| document.uri == uri)
            .map(|document| document.version);
        self.documents.retain(|document| document.uri != uri);
        self.clear_document_diagnostics(uri, version);
    }

    fn store_document(&mut self, uri: &str, version: i64, text: &str) {
        let path = uri::to_path(uri);
        match self
            .documents
            .iter()
            .position(|document| document.uri == uri)
        {
            Some(index) => {
                self.documents[index] = Document::new(uri, path.clone(), version, text);
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

    // -- diagnostics -------------------------------------------------------

    /// Queue a `textDocument/publishDiagnostics` notification for `uri`.
    ///
    /// Two sources are merged, both filtered to spans in *this* document:
    ///
    /// 1. the design database diagnostics (parse errors, skipped constructs,
    ///    duplicate declarations) and
    /// 2. the elaboration diagnostics ([`ElaboratedDesign::diagnostics`]:
    ///    unresolved names, width mismatches, non-constant generate bounds, ...).
    ///
    /// Elaboration is the same lazily cached one the hover/definition requests
    /// use: the first publication after a change pays for the elaboration that
    /// the next query would run anyway, later publications reuse the cache.  A
    /// diagnostic that both sources report on the same span/kind/message is
    /// published once.  Diagnostics without a span cannot be placed in an
    /// editor and are skipped rather than pinned to line 0; diagnostics whose
    /// span is in another file are published when that file is opened.  The
    /// document version travels with the notification so a client can drop
    /// stale answers.
    fn publish_document_diagnostics(&mut self, uri: &str) {
        self.ensure_design();
        let diagnostics: Vec<Json> = {
            let Some(document) = self.documents.iter().find(|document| document.uri == uri) else {
                return;
            };
            let path = document.path.as_path();
            let mut seen = HashSet::new();
            let mut diagnostics = Vec::new();
            let mut add = |diagnostic: &Diagnostic, span: &Span| {
                let key = (
                    span.clone(),
                    diagnostic.severity,
                    diagnostic.kind,
                    diagnostic.message.clone(),
                );
                if !seen.insert(key) {
                    return;
                }
                diagnostics.push(diagnostic_json(diagnostic, document.span_range(span)));
            };
            for diagnostic in self.db.diagnostics() {
                if let Some(span) = &diagnostic.span {
                    if span.file.as_path() == path {
                        add(diagnostic, span);
                    }
                }
            }
            if let Some(design) = self.design.as_ref() {
                for diagnostic in design.diagnostics() {
                    if let Some(span) = &diagnostic.span {
                        if span.file.as_path() == path {
                            add(diagnostic, span);
                        }
                    }
                }
            }
            diagnostics
        };
        let Some(version) = self
            .documents
            .iter()
            .find(|document| document.uri == uri)
            .map(|document| document.version)
        else {
            return;
        };
        self.notifications.push(protocol::notification_text(
            "textDocument/publishDiagnostics",
            Json::object([
                ("uri", Json::string(uri)),
                ("version", Json::Integer(version)),
                ("diagnostics", Json::Array(diagnostics)),
            ]),
        ));
    }

    /// Republish every open document's diagnostics.
    ///
    /// Called after the elaboration root changes, since a different top module
    /// can turn elaboration warnings on or off for any open file.
    fn publish_all_document_diagnostics(&mut self) {
        let uris: Vec<String> = self
            .documents
            .iter()
            .map(|document| document.uri.clone())
            .collect();
        for uri in uris {
            self.publish_document_diagnostics(&uri);
        }
    }

    /// Queue an empty `textDocument/publishDiagnostics` so the editor drops the
    /// markers of a document that was closed (or is about to disappear).
    fn clear_document_diagnostics(&mut self, uri: &str, version: Option<i64>) {
        let mut fields: Vec<(&str, Json)> = vec![("uri", Json::string(uri))];
        if let Some(version) = version {
            fields.push(("version", Json::Integer(version)));
        }
        fields.push(("diagnostics", Json::Array(Vec::new())));
        self.notifications.push(protocol::notification_text(
            "textDocument/publishDiagnostics",
            Json::object(fields),
        ));
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
            Some(SourceItem::Module { name, .. }) => module_reference_spans(&self.db, name),
            Some(SourceItem::Instance { module, .. }) => module_reference_spans(&self.db, module),
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

    /// `textDocument/completion`: identifiers visible at the cursor.
    ///
    /// The document and position resolve exactly like hover/definition; the
    /// enclosing module is found from the per-file module list (the same index
    /// documentSymbol uses) with a binary search, so no elaboration is needed
    /// and a big design is O(log n + k).
    ///
    /// Two modes, chosen from the text before the caret
    /// ([`Document::dotted_prefix_before`]):
    ///
    /// * **plain name** (`pc`, or nothing after a space/operator): the
    ///   enclosing module's ports, signals (including generate-scoped ones),
    ///   parameters/localparams, genvars and instance names, plus every module
    ///   name of the database for instantiation;
    /// * **member access** (`u_cpu.`, `u_cpu.pc`, `u_mid.u_leaf.`): the target
    ///   module is resolved by walking instance names (a module name is accepted
    ///   as the first qualifier too) and only that module's ports, signals,
    ///   parameters, genvars and instances are returned.  An unknown qualifier
    ///   yields an empty list.
    ///
    /// Two further qualifier shapes are understood:
    ///
    /// * **arrayed instance elements** ("u[1].", "u[i]."): an instance declared
    ///   with unpacked dimensions is resolved through its selector to the
    ///   instantiated module; a genvar/expression selector is accepted as "any
    ///   element".  An arrayed instance *without* a selector is ambiguous and
    ///   yields an empty list;
    /// * **generate scopes** ("g[0].", "g[i]."): the scope's own names
    ///   (instances/signals declared in that block) are listed; a literal
    ///   selector must name an existing unrolled element, a genvar/expression
    ///   selector matches every element (duplicates deduped).
    ///
    /// A select on a signal ("q[3:0].") is not a member access and yields an
    /// empty list rather than silently stripping the select.  In every mode the
    /// partial last segment filters labels case-insensitively ("U_CPU" matches
    /// "u_cpu").  Locals sort before module names.
    fn completion(&mut self, params: &Json) -> Result<Json, RpcError> {
        let Some(query) = self.resolve_position(params)? else {
            return Ok(Json::Array(Vec::new()));
        };
        let prefix = completion_prefix(self, params);
        let next_character = completion_next_character(self, params);
        let dot = last_top_level_dot(&prefix);
        // A dot typed before the caret is the normal member access.  A dot that
        // has not been typed yet but comes right after a select expression
        // (caret in "q[3:0]" of "q[3:0].") is the same member intent; a plain
        // word before a dot ("single." with the caret before the dot) stays a
        // plain-name prefix.
        let has_dot = dot.is_some_and(|index| index > 0);
        let select_member_intent = !has_dot && next_character == Some('.') && prefix.contains('[');

        let modules = self.db.modules_in_file(&query.path);
        let Some(module) = module_at_position(&modules, query.line) else {
            // Before the first module (file header comment etc.): nothing is
            // visible yet, an empty list (not an error) is the right answer.
            return Ok(Json::Array(Vec::new()));
        };

        if has_dot || select_member_intent {
            let (qualifier, filter) = if has_dot {
                split_completion_prefix(&prefix)
            } else {
                (prefix.clone(), String::new())
            };
            let segments = split_top_level(&qualifier);
            let Some(target) = self.resolve_completion_target(module, &segments) else {
                // Unknown instance/scope (or a select on a signal, which has no
                // members): an honest empty list, not the module-scoped one.
                return Ok(Json::Array(Vec::new()));
            };
            let filter = member_completion_filter(&filter).to_ascii_lowercase();
            return Ok(Json::Array(self.completion_items(&target, &filter, false)));
        }

        // Plain-name mode: only an identifier-shaped partial word filters.  A
        // complete selector ("u[1:0]") or a number ("4" inside "a[7:4]") is an
        // expression the caret is parked in, not a name being typed, so it falls
        // back to the enclosing module instead of matching no label at all.
        //
        // A leading dot is a port-connection name being typed (".cl" in a port
        // list): the segment after the dot is the partial name and filters the
        // list (".cl" -> clk, "." -> everything).  If that segment matches
        // nothing (the caret is parked inside an arbitrary expression) the
        // enclosing scope stays visible instead of an empty list.
        let leading_dot = prefix.starts_with('.');
        let filter = if leading_dot {
            let segment = prefix.rsplit('.').next().unwrap_or_default();
            member_completion_filter(segment).to_ascii_lowercase()
        } else {
            plain_completion_filter(&prefix).to_ascii_lowercase()
        };
        let target = CompletionTarget::Module(module);
        let mut items = self.completion_items(&target, &filter, true);
        if leading_dot && items.is_empty() && !filter.is_empty() {
            items = self.completion_items(&target, "", true);
        }
        Ok(Json::Array(items))
    }

    /// Walk a dotted qualifier to what completion should list.
    ///
    /// The design models three kinds of steps:
    ///
    /// * a generate-scope component (`g[0]`, `g[i]`): every unrolled scope with
    ///   that label is entered; a numeric selector must name an existing
    ///   element, a genvar/expression selector matches every element;
    /// * an instance name, optionally with an element selector for an arrayed
    ///   instance (`u[1]`, `u[i]`); an arrayed instance *without* a selector is
    ///   ambiguous and resolves to nothing instead of pretending to know the
    ///   element;
    /// * a module name as the first qualifier.
    ///
    /// A select on a signal (`q[3:0]`) is not a member access and also resolves
    /// to nothing (kept explicit rather than silently stripping the select).
    fn resolve_completion_target<'a>(
        &'a self,
        start: &'a ModuleDef,
        segments: &[&str],
    ) -> Option<CompletionTarget<'a>> {
        let mut module = start;
        let mut scope: Vec<String> = Vec::new();
        for segment in segments {
            if segment.is_empty() {
                return None;
            }
            if scope_component_exists(module, &scope, segment) {
                scope.push((*segment).to_owned());
                continue;
            }
            let (base, has_selector) = split_selector(segment);
            if base.is_empty() {
                return None;
            }
            let instance = module.instances.iter().find(|instance| {
                instance.name == base
                    && instance.scope.len() == scope.len()
                    && scope_pattern_matches(&scope, &instance.scope)
            });
            let Some(instance) = instance else {
                if scope.is_empty() && !has_selector {
                    return self.db.module(base).map(CompletionTarget::Module);
                }
                return None;
            };
            if has_selector {
                if instance.array.is_empty() {
                    return None;
                }
            } else if !instance.array.is_empty() {
                return None;
            }
            module = self.db.module(&instance.module)?;
            scope.clear();
        }
        Some(if scope.is_empty() {
            CompletionTarget::Module(module)
        } else {
            CompletionTarget::Scope(module, scope)
        })
    }

    /// The completion items of one resolved target (whole module or a generate
    /// scope), case-insensitively filtered by `filter` (empty = everything).
    fn completion_items(
        &self,
        target: &CompletionTarget<'_>,
        filter: &str,
        include_modules: bool,
    ) -> Vec<Json> {
        match target {
            CompletionTarget::Module(module) => {
                self.module_items(module, &[], filter, include_modules)
            }
            CompletionTarget::Scope(module, scope) => {
                self.module_items(module, scope, filter, false)
            }
        }
    }

    /// The completion items of one module, limited to `scope` when it is not
    /// empty (generate-scope member completion).  Module names are only
    /// included for the plain-name mode.
    fn module_items(
        &self,
        module: &ModuleDef,
        scope: &[String],
        filter: &str,
        include_modules: bool,
    ) -> Vec<Json> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut items = Vec::new();
        let mut push = |label: &str, kind: i64, detail: String, group: &str| {
            if !label.to_ascii_lowercase().starts_with(filter) {
                return;
            }
            if !seen.insert(label.to_owned()) {
                return;
            }
            items.push(completion_item(
                label,
                kind,
                detail,
                &format!("{group}{label}"),
            ));
        };

        // Ports: Variable, detail direction/type/width.  Ports belong to the
        // module body, never to a generate scope.
        if scope.is_empty() {
            for port in &module.ports {
                let detail = format!(
                    "{} {}",
                    port.direction.as_str(),
                    type_detail(port.kind.as_str(), &port.packed, port.width)
                );
                push(&port.name, COMPLETION_VARIABLE, detail, "0");
            }
        }
        // Signals, including declarations inside generate blocks (their scope
        // is part of the detail so "g[0].x" stays distinguishable).
        for signal in &module.signals {
            if !scope.is_empty() && !scope_pattern_matches_prefix(&signal.scope, scope) {
                continue;
            }
            let mut detail = type_detail(signal.kind.as_str(), &signal.packed, signal.width);
            if !signal.scope.is_empty() {
                detail.push_str(&format!(" in {}", signal.scope.join(".")));
            }
            push(&signal.name, COMPLETION_VARIABLE, detail, "0");
        }
        // Parameters and localparams: Field (module body only).
        for parameter in module.parameters.iter().filter(|_| scope.is_empty()) {
            let keyword = if parameter.is_local {
                "localparam"
            } else {
                "parameter"
            };
            let detail = match &parameter.default {
                Some(default) => format!("{keyword} = {default}"),
                None => keyword.to_owned(),
            };
            push(&parameter.name, COMPLETION_FIELD, detail, "0");
        }
        // Genvars (declarations and generate-loop variables): Field.
        for genvar in module.genvars.iter().filter(|_| scope.is_empty()) {
            push(genvar, COMPLETION_FIELD, "genvar".to_owned(), "0");
        }
        // Instance names: Reference, detail names the instantiated module.
        for instance in &module.instances {
            if !scope.is_empty() && !scope_pattern_matches_prefix(&instance.scope, scope) {
                continue;
            }
            let mut detail = format!("instance of {}", instance.module);
            if !instance.array.is_empty() {
                detail.push_str(" (arrayed)");
            }
            if !instance.scope.is_empty() {
                detail.push_str(&format!(" in {}", instance.scope.join(".")));
            }
            push(&instance.name, COMPLETION_REFERENCE, detail, "0");
        }
        if include_modules {
            // Every module of the database: Module kind, sorted after locals.
            for definition in self.db.modules() {
                push(
                    &definition.name,
                    COMPLETION_MODULE,
                    "module".to_owned(),
                    "1",
                );
            }
        }
        items
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
        // Indexed by file: only the modules declared in this document are
        // visited, not every module of the database.
        for module in self.db.modules_in_file(&path) {
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
            .flat_map(|module| {
                module
                    .instances
                    .iter()
                    .map(|instance| instance.module.as_str())
            })
            .collect();
        self.db
            .modules()
            .iter()
            .find(|module| !instantiated.contains(module.name.as_str()))
            .or_else(|| self.db.modules().first())
            .map(|module| module.name.clone())
    }

    fn document(&self, uri: &str) -> Option<&Document> {
        self.documents.iter().find(|document| document.uri == uri)
    }

    fn document_for_path(&self, path: &Path) -> Option<&Document> {
        self.documents.iter().find(|document| document.path == path)
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
            let keyword = if parameter.is_local {
                "localparam"
            } else {
                "parameter"
            };
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
    spans.extend(
        design
            .drivers(path)
            .iter()
            .map(|driver| driver.span.clone()),
    );
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

/// LSP `CompletionItemKind` numbers used by the completion handler.
const COMPLETION_FIELD: i64 = 5;
const COMPLETION_VARIABLE: i64 = 6;
const COMPLETION_MODULE: i64 = 9;
const COMPLETION_REFERENCE: i64 = 18;

/// One completion item: label, kind, a human detail and the sort group
/// (locals "0", module names "1" so globals come last).
fn completion_item(label: &str, kind: i64, detail: String, sort_text: &str) -> Json {
    Json::object([
        ("label", Json::string(label)),
        ("kind", Json::Integer(kind)),
        ("detail", Json::string(detail)),
        ("sortText", Json::string(sort_text)),
    ])
}

/// The module whose source range contains `line`.
///
/// [`ModuleDef::span`] only covers the module declaration line, so the end of a
/// module is the start of the next module in the same file (files are parsed in
/// order); the last module runs to the end of the file.  Modules cannot nest in
/// the SystemVerilog subset the front end supports, so this is the innermost
/// module as well.
/// The text typed before the caret (see [`Document::dotted_prefix_before`]).
fn completion_prefix(server: &LspServer, params: &Json) -> String {
    let Some(uri) = params
        .get("textDocument")
        .and_then(|text_document| text_document.get_str("uri"))
    else {
        return String::new();
    };
    let Some(position) = params.get("position").and_then(Position::from_json) else {
        return String::new();
    };
    server
        .document(uri)
        .map(|document| document.dotted_prefix_before(position))
        .unwrap_or_default()
}

/// The character right at the caret (one past the text already typed).
fn completion_next_character(server: &LspServer, params: &Json) -> Option<char> {
    let uri = params
        .get("textDocument")
        .and_then(|text_document| text_document.get_str("uri"))?;
    let position = params.get("position").and_then(Position::from_json)?;
    server
        .document(uri)
        .and_then(|document| document.character_at(position))
}

/// The filter a plain-name completion uses.
///
/// Only a partial identifier filters: a complete selector (`u[1:0]`), a number
/// from an expression (`a[7:4]`), a leading `.` or any other non-identifier
/// prefix means "no word is being typed here", so the enclosing module scope is
/// returned instead of an empty list.
fn plain_completion_filter(prefix: &str) -> &str {
    if prefix.is_empty() || prefix.contains('[') || prefix.starts_with('.') {
        return "";
    }
    match prefix.chars().next() {
        Some(first) if first.is_alphabetic() || first == '_' || first == '$' => prefix,
        _ => "",
    }
}

/// The filter a member completion uses: a partial selector after the last dot
/// (`u[1].q[3`) filters by the name so far (`q`).
fn member_completion_filter(filter: &str) -> &str {
    filter.split('[').next().unwrap_or("")
}

/// Split a completion prefix into qualifier and partial last segment.
///
/// Dots inside selectors do not split, so u[1].q keeps u[1] as the qualifier.
fn split_completion_prefix(prefix: &str) -> (String, String) {
    if prefix.is_empty() {
        return (String::new(), String::new());
    }
    match last_top_level_dot(prefix) {
        Some(index) => (prefix[..index].to_owned(), prefix[index + 1..].to_owned()),
        None => (String::new(), prefix.to_owned()),
    }
}

/// Byte index of the last '.' that is outside any [...] selector.
fn last_top_level_dot(text: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut last = None;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => depth = (depth - 1).max(0),
            b'.' if depth == 0 => last = Some(index),
            _ => {}
        }
    }
    last
}

/// Split a dotted qualifier on top-level dots (selectors kept intact).
fn split_top_level(text: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'[' => depth += 1,
            b']' => depth = (depth - 1).max(0),
            b'.' if depth == 0 => {
                segments.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    segments.push(&text[start..]);
    segments
}

/// A dotted segment: the base name and whether it carries an element selector.
fn split_selector(segment: &str) -> (&str, bool) {
    match segment.find('[') {
        Some(index) => (&segment[..index], true),
        None => (segment, false),
    }
}

/// Does one qualifier component match one scope component?
///
/// A literal numeric selector must name the actual unrolled scope element
/// (g[0] matches g[0] only); a genvar or expression selector matches any element
/// of that scope name (g[i] matches g[0] and g[1]).
fn completion_component_matches(scope_component: &str, qualifier: &str) -> bool {
    let (scope_base, scope_has_selector) = split_selector(scope_component);
    let (qualifier_base, qualifier_has_selector) = split_selector(qualifier);
    if scope_base != qualifier_base || scope_has_selector != qualifier_has_selector {
        return false;
    }
    if !qualifier_has_selector {
        return scope_component == qualifier;
    }
    let selector = qualifier
        .find('[')
        .map(|index| &qualifier[index + 1..qualifier.len().saturating_sub(1)])
        .unwrap_or_default();
    if selector.trim().parse::<u64>().is_ok() {
        scope_component == qualifier
    } else {
        true
    }
}

/// True when the generated scope of an item starts with the qualifier pattern.
fn scope_pattern_matches_prefix(item_scope: &[String], pattern: &[String]) -> bool {
    item_scope.len() >= pattern.len()
        && pattern
            .iter()
            .zip(item_scope.iter())
            .all(|(qualifier, component)| completion_component_matches(component, qualifier))
}

/// Exact-length scope match (arrayed instance lookup inside a generate scope).
fn scope_pattern_matches(pattern: &[String], item_scope: &[String]) -> bool {
    pattern.len() == item_scope.len() && scope_pattern_matches_prefix(item_scope, pattern)
}

/// Does the module declare a generate scope component matching this qualifier
/// at the current depth?
fn scope_component_exists(module: &ModuleDef, scope: &[String], qualifier: &str) -> bool {
    let (base, _) = split_selector(qualifier);
    if base.is_empty() {
        return false;
    }
    let matches = |item_scope: &[String]| {
        item_scope.len() > scope.len()
            && scope_pattern_matches_prefix(&item_scope[..scope.len()], scope)
            && completion_component_matches(&item_scope[scope.len()], qualifier)
    };
    module
        .instances
        .iter()
        .any(|instance| matches(&instance.scope))
        || module.signals.iter().any(|signal| matches(&signal.scope))
}

/// What a dotted qualifier resolved to: a whole module, or one generate scope
/// inside a module (member completion for g[0]. etc.).
enum CompletionTarget<'a> {
    Module(&'a ModuleDef),
    Scope(&'a ModuleDef, Vec<String>),
}

fn module_at_position<'a>(modules: &[&'a ModuleDef], line: u32) -> Option<&'a ModuleDef> {
    // The per-file list is in declaration order (ascending start lines), so a
    // binary search finds the last module that starts at or before the cursor.
    let index = modules.partition_point(|module| module.span.start_line <= line);
    let module = *modules.get(index.checked_sub(1)?)?;
    let end = modules
        .get(index)
        .map(|next| next.span.start_line)
        .unwrap_or(u32::MAX);
    (line < end).then_some(module)
}

/// Type text for a completion detail: the packed dimensions when declared,
/// otherwise the width (`logic [7:0]`, `input logic`, `reg 8 bits`).
fn type_detail(kind: &str, dimensions: &[RangeDim], width: Option<u32>) -> String {
    let dimensions = dimensions_text(dimensions);
    if !dimensions.is_empty() {
        format!("{kind} {dimensions}")
    } else {
        match width {
            Some(1) | None => kind.to_owned(),
            Some(width) => format!("{kind} {width} bits"),
        }
    }
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
        .filter(|other| other.module == signal.module && other.local_name == signal.local_name)
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
            if let Some(driven) = reference_ranges(&signal.drivers, &signal.local_name) {
                lines.push(format!("- driven here: {driven}"));
            }
            if let Some(read) = reference_ranges(&signal.loads, &signal.local_name) {
                lines.push(format!("- read here: {read}"));
            }
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
            if let Some(driven) = reference_ranges(&signal.drivers, &signal.local_name) {
                lines.push(format!("driven here: {driven}"));
            }
            if let Some(read) = reference_ranges(&signal.loads, &signal.local_name) {
                lines.push(format!("read here: {read}"));
            }
            lines.push(format!("declared at {location}"));
            if siblings.len() > 1 {
                lines.push(format!("also instantiated as {}", siblings.join(", ")));
            }
        }
    }
    Some(lines.join("\n"))
}

/// The distinct bit ranges of a signal's driver/load references, formatted
/// with the signal name (`q[3:0], q[7:4]`); `None` when every reference is
/// whole-signal.
fn reference_ranges(references: &[SignalReference], name: &str) -> Option<String> {
    let mut ranges: Vec<String> = Vec::new();
    for reference in references {
        let Some(bits) = reference.bits.as_deref() else {
            continue;
        };
        let range = format!("{name}{bits}");
        if !ranges.contains(&range) {
            ranges.push(range);
        }
    }
    if ranges.is_empty() {
        None
    } else {
        Some(ranges.join(", "))
    }
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

fn instance_hover(design: &ElaboratedDesign, path: &str, format: HoverFormat) -> Option<String> {
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

/// One design-database diagnostic as the LSP diagnostic object.
///
/// [`DesignDb::diagnostics`] carries every parse error, skipped construct and
/// unresolved name it found; the kinds map to the LSP `code` so a client can
/// filter them, and the severity numbers are the LSP ones (error 1, warning 2,
/// information 3).  The source is the server name, which is what the editor
/// shows next to the message.
fn diagnostic_json(diagnostic: &Diagnostic, range: Range) -> Json {
    let severity = match diagnostic.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        Severity::Info => 3,
    };
    Json::object([
        ("range", range.to_json()),
        ("severity", Json::Integer(severity)),
        ("code", Json::string(diagnostic.kind.as_str())),
        ("source", Json::string("wave-insight")),
        ("message", Json::string(diagnostic.message.clone())),
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
        let line = text[..offset].bytes().filter(|byte| *byte == b'\n').count() as u32;
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
        assert_eq!(
            replies.len(),
            1,
            "a request gets one reply, got {replies:?}"
        );
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
        assert_eq!(
            replies.len(),
            1,
            "a request gets one reply, got {replies:?}"
        );
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
        assert!(
            replies.is_empty(),
            "notifications have no reply: {replies:?}"
        );
    }

    fn initialize(server: &mut LspServer) -> Json {
        request(
            server,
            "initialize",
            Json::object([("processId", Json::Null), ("capabilities", empty_object())]),
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

    /// Drain and decode the notifications queued by the last message.
    fn take_notifications(server: &mut LspServer) -> Vec<Json> {
        server
            .take_notifications()
            .iter()
            .map(|notification| {
                Json::parse(notification).unwrap_or_else(|error| {
                    panic!("server notification is not JSON ({error}): {notification}")
                })
            })
            .collect()
    }

    /// Only the `publishDiagnostics` notifications of a queue.
    fn published(notifications: Vec<Json>) -> Vec<Json> {
        notifications
            .into_iter()
            .filter(|notification| {
                notification.get_str("method") == Some("textDocument/publishDiagnostics")
            })
            .collect()
    }

    /// Parse one diagnostic's range into (start_line, start_character, end_line, end_character).
    fn diagnostic_range(diagnostic: &Json) -> (i64, i64, i64, i64) {
        let range = diagnostic.get("range").expect("diagnostic range");
        let position = |name: &str| -> (i64, i64) {
            let position = range.get(name).expect("range position");
            (
                position.get_i64("line").expect("line"),
                position.get_i64("character").expect("character"),
            )
        };
        let (start_line, start_character) = position("start");
        let (end_line, end_character) = position("end");
        (start_line, start_character, end_line, end_character)
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
                (
                    "textDocument",
                    Json::object([("uri", Json::string(&cpu_uri))]),
                ),
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
                (
                    "textDocument",
                    Json::object([("uri", Json::string(&cpu_uri))]),
                ),
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
                (
                    "textDocument",
                    Json::object([("uri", Json::string(&top_uri))]),
                ),
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
        let symbols = request(
            &mut server,
            "textDocument/documentSymbol",
            document_params(&uri),
        );
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
        let symbols = request(
            &mut server,
            "textDocument/documentSymbol",
            document_params(&uri),
        );
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
        let symbols = request(
            &mut server,
            "textDocument/documentSymbol",
            document_params(&uri),
        );
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
                (
                    "textDocument",
                    Json::object([("uri", Json::string(&unknown))])
                ),
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

    // -- completion --------------------------------------------------------

    fn completion_labels(reply: &Json) -> Vec<String> {
        reply
            .as_array()
            .unwrap_or_else(|| panic!("completion result is an array: {reply:?}"))
            .iter()
            .filter_map(|item| item.get_str("label").map(str::to_owned))
            .collect()
    }

    fn completion_item_named<'a>(reply: &'a Json, label: &str) -> &'a Json {
        reply
            .as_array()
            .expect("completion array")
            .iter()
            .find(|item| item.get_str("label") == Some(label))
            .unwrap_or_else(|| panic!("no completion item {label}: {reply:?}"))
    }

    #[test]
    fn completion_inside_a_module_lists_ports_signals_instances_and_modules() {
        let (mut server, top_uri, _cpu_uri) = fixture_server();
        let top = fixture_text("top.sv");
        let position = position_of(&top, "pc;");
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&top_uri, position),
        );
        let labels = completion_labels(&reply);
        for expected in [
            "clk", "reset", "instr", "result", // ports
            "pc", "data_out", "pc16", // signals
            "u_cpu", "u_cpu16", // instances
            "top", "cpu", // module names
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing {expected} in {labels:?}"
            );
        }

        // Kinds and details are useful enough for the editor to group them.
        let pc = completion_item_named(&reply, "pc");
        assert_eq!(pc.get_i64("kind"), Some(6), "signal is a Variable: {pc:?}");
        assert_eq!(pc.get_str("detail"), Some("logic [7:0]"));
        assert_eq!(pc.get_str("sortText"), Some("0pc"));
        let clk = completion_item_named(&reply, "clk");
        assert_eq!(clk.get_i64("kind"), Some(6));
        assert_eq!(clk.get_str("detail"), Some("input logic"));
        let instance = completion_item_named(&reply, "u_cpu");
        assert_eq!(
            instance.get_i64("kind"),
            Some(18),
            "instance is a Reference"
        );
        assert_eq!(instance.get_str("detail"), Some("instance of cpu"));
        let module = completion_item_named(&reply, "cpu");
        assert_eq!(module.get_i64("kind"), Some(9), "module is kind Module");
        assert_eq!(module.get_str("sortText"), Some("1cpu"));
    }

    #[test]
    fn completion_on_the_cpu_fixture_lists_parameters_and_ports() {
        let (mut server, _top_uri, cpu_uri) = fixture_server();
        let cpu = fixture_text("cpu.sv");
        let position = position_of(&cpu, "assign active");
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&cpu_uri, position),
        );
        let labels = completion_labels(&reply);
        for expected in [
            "WIDTH", "DEPTH", "STEP", "clk", "reset", "instr", "next_pc", "pc_reg", "active",
        ] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing {expected} in {labels:?}"
            );
        }
        assert_eq!(
            completion_item_named(&reply, "WIDTH").get_str("detail"),
            Some("parameter = 8")
        );
        assert_eq!(
            completion_item_named(&reply, "STEP").get_str("detail"),
            Some("localparam = 1")
        );
        assert_eq!(
            completion_item_named(&reply, "WIDTH").get_i64("kind"),
            Some(5),
            "parameter is a Field"
        );
    }

    #[test]
    fn completion_on_generate_and_arrayed_fixtures_does_not_crash() {
        let mut server = LspServer::new();
        initialize(&mut server);
        let generate_uri = open_fixture(&mut server, "generate.sv");
        let generate = fixture_text("generate.sv");
        let position = position_of(&generate, "assign done");
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&generate_uri, position),
        );
        let labels = completion_labels(&reply);
        for expected in ["done", "busy", "flag", "u_leaf", "i", "gen_top"] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing {expected} in {labels:?}"
            );
        }
        // Generate-scoped signals name their block in the detail.
        let done = completion_item_named(&reply, "done");
        assert!(
            done.get_str("detail")
                .unwrap_or_default()
                .contains("in wide"),
            "{done:?}"
        );

        let arrayed_uri = open_fixture(&mut server, "arrayed.sv");
        let arrayed = fixture_text("arrayed.sv");
        let position = position_of(&arrayed, "arr_leaf u[1:0]");
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&arrayed_uri, position),
        );
        let labels = completion_labels(&reply);
        for expected in ["u", "v", "arr_leaf"] {
            assert!(
                labels.iter().any(|label| label == expected),
                "missing {expected} in {labels:?}"
            );
        }
        let arrayed_instance = completion_item_named(&reply, "u");
        assert!(
            arrayed_instance
                .get_str("detail")
                .unwrap_or_default()
                .contains("arrayed"),
            "{arrayed_instance:?}"
        );
    }

    #[test]
    fn completion_outside_a_module_or_for_an_unknown_document_is_empty() {
        let mut server = LspServer::new();
        initialize(&mut server);
        let top_uri = open_fixture(&mut server, "top.sv");
        // Line 1 is the file's leading comment: before the first module.
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&top_uri, Position::new(0, 0)),
        );
        assert_eq!(
            reply.as_array().map(<[Json]>::len),
            Some(0),
            "no module yet: {reply:?}"
        );

        let unknown = uri::to_uri(&fixture("legacy.v"));
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&unknown, Position::new(0, 0)),
        );
        assert_eq!(
            reply.as_array().map(<[Json]>::len),
            Some(0),
            "unknown document: {reply:?}"
        );
    }

    // -- completion v2: prefix filtering and member access ----------------

    fn advance(position: Position, characters: u32) -> Position {
        Position::new(position.line, position.character + characters)
    }

    #[test]
    fn member_completion_after_an_instance_dot() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_members.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_members.sv");
        let source = fixture_text("completion_members.sv");
        // "assign out = u_mid.u_leaf.q;": the first u_mid. occurrence.
        let position = advance(position_of(&source, "u_mid."), "u_mid.".len() as u32);
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        );
        let labels = completion_labels(&reply);
        for expected in ["a", "b", "u_leaf"] {
            assert!(
                labels.iter().any(|label| label == expected),
                "member of member_mid missing {expected}: {labels:?}"
            );
        }
        // Members only: no module names and no names from the outer module.
        assert!(
            !labels.iter().any(|label| label == "member_top"),
            "{labels:?}"
        );
        assert!(!labels.iter().any(|label| label == "clk"), "{labels:?}");
    }

    #[test]
    fn nested_member_completion_and_member_prefix_filtering() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_members.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_members.sv");
        let source = fixture_text("completion_members.sv");

        // u_mid.u_leaf. -> member_leaf members.
        let position = advance(
            position_of(&source, "u_mid.u_leaf."),
            "u_mid.u_leaf.".len() as u32,
        );
        let labels = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        ));
        assert_eq!(labels, vec!["clk", "d", "q"], "{labels:?}");

        // u_mid.u_l -> only the nested instance that starts with "u_l".
        let position = advance(position_of(&source, "u_mid.u_l "), "u_mid.u_l".len() as u32);
        let labels = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        ));
        assert_eq!(labels, vec!["u_leaf"], "{labels:?}");

        // Case-insensitive member prefix: u_mid.U_L matches u_leaf.
        let position = advance(position_of(&source, "u_mid.U_L"), "u_mid.U_L".len() as u32);
        let labels = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        ));
        assert_eq!(labels, vec!["u_leaf"], "{labels:?}");
    }

    #[test]
    fn member_completion_accepts_a_module_name_and_rejects_unknown_qualifiers() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_members.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_members.sv");
        let source = fixture_text("completion_members.sv");

        // A module name works as the first qualifier even without an instance.
        let position = advance(
            position_of(&source, "member_leaf."),
            "member_leaf.".len() as u32,
        );
        let labels = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        ));
        assert_eq!(labels, vec!["clk", "d", "q"], "{labels:?}");

        // An unknown qualifier is an empty list, not an error.
        let position = advance(position_of(&source, "nope."), "nope.".len() as u32);
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        );
        assert_eq!(reply.as_array().map(<[Json]>::len), Some(0), "{reply:?}");
    }

    #[test]
    fn plain_prefix_completion_is_case_insensitive_and_module_scoped() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_members.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_members.sv");
        let source = fixture_text("completion_members.sv");

        // "member_mid u_mid(": cursor right after "u_mid".
        let position = advance(position_of(&source, "u_mid("), "u_mid".len() as u32);
        let labels = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        ));
        assert_eq!(labels, vec!["u_mid"], "{labels:?}");

        // U_LEAF in the probe comment matches the u_leaf instance.
        let position = advance(position_of(&source, "U_LEAF"), "U_LEAF".len() as u32);
        let labels = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        ));
        assert_eq!(labels, vec!["u_leaf"], "{labels:?}");
    }

    #[test]
    fn member_completion_keeps_the_detail_of_the_member() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_members.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_members.sv");
        let source = fixture_text("completion_members.sv");
        let position = advance(
            position_of(&source, "u_mid.u_leaf."),
            "u_mid.u_leaf.".len() as u32,
        );
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&uri, position),
        );
        let d = completion_item_named(&reply, "d");
        assert_eq!(d.get_str("detail"), Some("input logic [7:0]"), "{d:?}");
        let q = completion_item_named(&reply, "q");
        assert_eq!(q.get_str("detail"), Some("output logic [7:0]"), "{q:?}");
    }

    // -- completion v3: arrayed selectors and generate scopes --------------

    fn completion_at(
        server: &mut LspServer,
        uri: &str,
        source: &str,
        needle: &str,
        advance_characters: usize,
    ) -> Json {
        let position = {
            let start = position_of(source, needle);
            Position::new(start.line, start.character + advance_characters as u32)
        };
        request(
            server,
            "textDocument/completion",
            position_params(uri, position),
        )
    }

    #[test]
    fn member_completion_through_an_arrayed_instance_selector() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_arrays.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_arrays.sv");
        let source = fixture_text("completion_arrays.sv");

        for needle in ["u[0].", "u[1].", "u[i]."] {
            let reply = completion_at(&mut server, &uri, &source, needle, needle.len());
            let labels = completion_labels(&reply);
            assert_eq!(labels, vec!["clk", "d", "q"], "{needle}: {labels:?}");
        }

        // An arrayed instance without an element selector is ambiguous and is
        // reported honestly as "no answer", not as one element's members.
        let reply = completion_at(&mut server, &uri, &source, "u.", 2);
        assert_eq!(
            reply.as_array().map(<[Json]>::len),
            Some(0),
            "u.: {reply:?}"
        );
    }

    #[test]
    fn select_on_a_signal_has_no_members_and_keeps_the_modes_honest() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_arrays.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_arrays.sv");
        let source = fixture_text("completion_arrays.sv");

        // q[3:0]. is a member access on a selected signal: no members.
        let reply = completion_at(&mut server, &uri, &source, "q[3:0].", "q[3:0].".len());
        assert_eq!(reply.as_array().map(<[Json]>::len), Some(0), "{reply:?}");

        // q[3:0] without the dot stays a plain-name prefix (and honestly
        // matches no label, because labels are bare identifiers).
        let reply = completion_at(&mut server, &uri, &source, "q[3:0].", "q[3:0]".len());
        assert_eq!(reply.as_array().map(<[Json]>::len), Some(0), "{reply:?}");

        // The plain mode still works next to it.
        let reply = completion_at(&mut server, &uri, &source, "single.", "single".len());
        assert_eq!(completion_labels(&reply), vec!["single"], "{reply:?}");
    }

    #[test]
    fn generate_scope_member_completion() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_scopes.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_scopes.sv");
        let source = fixture_text("completion_scopes.sv");

        // A concrete unrolled scope lists the names declared inside it.
        let reply = completion_at(&mut server, &uri, &source, "g[0].", "g[0].".len());
        assert_eq!(
            completion_labels(&reply),
            vec!["marker", "u_leaf"],
            "{reply:?}"
        );

        // A genvar selector matches every element; duplicates are deduped.
        let reply = completion_at(&mut server, &uri, &source, "g[i].", "g[i].".len());
        assert_eq!(
            completion_labels(&reply),
            vec!["marker", "u_leaf"],
            "{reply:?}"
        );

        // Nested: scope + instance -> that instance's module members.
        for needle in ["g[0].u_leaf.", "g[i].u_leaf."] {
            let reply = completion_at(&mut server, &uri, &source, needle, needle.len());
            assert_eq!(
                completion_labels(&reply),
                vec!["a", "y"],
                "{needle}: {reply:?}"
            );
        }

        // A numeric selector that names no element is an unknown scope.
        let reply = completion_at(&mut server, &uri, &source, "g[9].", "g[9].".len());
        assert_eq!(reply.as_array().map(<[Json]>::len), Some(0), "{reply:?}");
    }

    #[test]
    fn generate_scope_completion_filters_by_the_member_prefix() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_scopes.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_scopes.sv");
        let source = fixture_text("completion_scopes.sv");
        let reply = completion_at(&mut server, &uri, &source, "g[0].u", "g[0].u".len());
        assert_eq!(completion_labels(&reply), vec!["u_leaf"], "{reply:?}");
    }

    // -- completion context classification (Wave 13 follow-up) --------------

    /// Request completion at every character column of one line and return the
    /// columns that answered with an empty list.
    fn empty_columns(
        server: &mut LspServer,
        uri: &str,
        source: &str,
        line_contains: &str,
    ) -> Vec<usize> {
        let lines: Vec<&str> = source.split('\n').collect();
        let line = lines
            .iter()
            .position(|line| line.contains(line_contains))
            .unwrap_or_else(|| panic!("no line containing {line_contains}"));
        let line_text = lines[line];
        let mut empty = Vec::new();
        for character in 0..=line_text.chars().count() {
            let reply = request(
                server,
                "textDocument/completion",
                position_params(uri, Position::new(line as u32, character as u32)),
            );
            let items = reply
                .as_array()
                .unwrap_or_else(|| panic!("completion result is an array: {reply:?}"));
            if items.is_empty() {
                empty.push(character);
            }
        }
        empty
    }

    #[test]
    fn arrayed_port_connection_line_has_completion_at_every_column() {
        let mut server = LspServer::with_db(DesignDb::load_files([fixture("arrayed.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "arrayed.sv");
        let source = fixture_text("arrayed.sv");

        // The verifier's repro line: arr_leaf u[1:0] (.d(a[7:4]), .q(q[7:4]));
        let empty = empty_columns(&mut server, &uri, &source, "arr_leaf u[1:0]");
        assert!(empty.is_empty(), "empty completion at columns {empty:?}");

        // The specific reports (1-based columns 20/30/40 -> 0-based characters
        // 19/29/39) must answer with the enclosing module scope.
        let lines: Vec<&str> = source.split('\n').collect();
        let line = lines
            .iter()
            .position(|line| line.contains("arr_leaf u[1:0]"))
            .expect("arrayed instance line");
        for character in [19u32, 29, 39] {
            let reply = request(
                &mut server,
                "textDocument/completion",
                position_params(&uri, Position::new(line as u32, character)),
            );
            let labels = completion_labels(&reply);
            assert!(
                labels.iter().any(|label| label == "arr_leaf")
                    && labels.iter().any(|label| label == "arrayed_top"),
                "column {character}: expected module scope, got {labels:?}"
            );
        }
    }

    #[test]
    fn non_arrayed_bit_select_connections_have_completion_at_every_column() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("completion_context.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "completion_context.sv");
        let source = fixture_text("completion_context.sv");

        // The same bug class appears without arrays: a bit-select expression in
        // a non-arrayed instance's port connection.
        for line_contains in [
            "context_leaf plain(.d(bus[7:4])",
            "context_leaf arr[1:0] (.d(data[3:0])",
            "context_leaf single_bit(.d(bus[0])",
        ] {
            let empty = empty_columns(&mut server, &uri, &source, line_contains);
            assert!(
                empty.is_empty(),
                "{line_contains}: empty completion at columns {empty:?}"
            );
        }
    }

    #[test]
    fn port_connection_leading_dot_filters_by_the_port_prefix() {
        let (mut server, top_uri, _cpu_uri) = fixture_server();
        let top = fixture_text("top.sv");

        // The QA position: top.sv line 14 is "        .clk(clk),", the caret is
        // right after the typed prefix ".cl" (1-based column 12).
        let position = advance(position_of(&top, ".clk("), 3);
        assert_eq!(
            (position.line, position.character),
            (13, 11),
            "expected line 14, column 12"
        );
        let reply = request(
            &mut server,
            "textDocument/completion",
            position_params(&top_uri, position),
        );
        assert_eq!(completion_labels(&reply), vec!["clk"], "{reply:?}");

        // "." alone offers the full unfiltered module list (port list plus the
        // module scope), exactly like an empty plain prefix.
        let dot_position = advance(position_of(&top, ".clk("), 1);
        let dot_labels = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&top_uri, dot_position),
        ));
        let baseline = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&top_uri, Position::new(13, 0)),
        ));
        assert_eq!(dot_labels, baseline, "{dot_labels:?}");
        for expected in ["clk", "reset", "instr", "result", "u_cpu", "u_cpu16"] {
            assert!(
                dot_labels.iter().any(|label| label == expected),
                "missing {expected} in {dot_labels:?}"
            );
        }

        // ".reset" narrows to the one port label.
        let reset = advance(position_of(&top, ".reset("), ".reset".len() as u32);
        let labels = completion_labels(&request(
            &mut server,
            "textDocument/completion",
            position_params(&top_uri, reset),
        ));
        assert_eq!(labels, vec!["reset"], "{labels:?}");
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
        let completion = capabilities
            .get("completionProvider")
            .expect("completion capability");
        assert_eq!(completion.get_bool("resolveProvider"), Some(false));
        assert!(completion.get("triggerCharacters").is_some());
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
            "textDocument/formatting",
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
        let reply = server
            .handle_message_text(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#);
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

    // -- diagnostics -------------------------------------------------------

    /// A file that does not parse: the design database reports a `parse`
    /// diagnostic and the server has to publish it as a Monaco/LSP diagnostic.
    const BROKEN: &str = "module broken(input logic a;\n    assign a = 1'b0;\nendmodule\n";
    const CLEAN: &str = "module clean(input logic a);\n    assign a = a;\nendmodule\n";

    #[test]
    fn a_malformed_document_publishes_diagnostics() {
        let mut server = LspServer::new();
        initialize(&mut server);
        let uri = "wave-insight://source/broken.sv";
        did_open(&mut server, uri, BROKEN);

        let notifications = published(take_notifications(&mut server));
        assert_eq!(notifications.len(), 1, "{notifications:?}");
        let notification = &notifications[0];
        let params = notification.get("params").expect("notification params");
        assert_eq!(params.get_str("uri"), Some(uri));
        assert_eq!(params.get_i64("version"), Some(1));

        let diagnostics = params.get_array("diagnostics").expect("diagnostics array");
        assert!(
            !diagnostics.is_empty(),
            "expected a parse diagnostic: {params:?}"
        );
        let first = &diagnostics[0];
        let (start_line, start_character, end_line, end_character) = diagnostic_range(first);
        assert!(start_line >= 0 && end_line >= 0, "negative line: {first:?}");
        assert!(
            (end_line, end_character) >= (start_line, start_character),
            "range ends before it starts: {first:?}"
        );
        assert!(end_character < 500, "range runs past the buffer: {first:?}");
        assert_eq!(
            first.get_i64("severity"),
            Some(1),
            "parse errors are errors: {first:?}"
        );
        assert_eq!(first.get_str("source"), Some("wave-insight"));
        assert_eq!(first.get_str("code"), Some("parse"));
        assert!(
            !first.get_str("message").unwrap_or_default().is_empty(),
            "diagnostic message is empty: {first:?}"
        );

        // The old reply contract is untouched: notifications are not replies.
        assert!(
            server.pending_notifications() == 0,
            "queue was drained above"
        );
    }

    #[test]
    fn a_clean_document_publishes_an_empty_diagnostics_array() {
        let mut server = LspServer::new();
        initialize(&mut server);
        let uri = "wave-insight://source/clean.sv";
        did_open(&mut server, uri, CLEAN);

        let notifications = published(take_notifications(&mut server));
        assert_eq!(notifications.len(), 1, "{notifications:?}");
        let params = notifications[0].get("params").expect("params");
        assert_eq!(params.get_str("uri"), Some(uri));
        assert_eq!(params.get_i64("version"), Some(1));
        assert_eq!(
            params.get_array("diagnostics").map(<[Json]>::len),
            Some(0),
            "a clean file must clear the markers: {params:?}"
        );
    }

    #[test]
    fn fixing_a_document_replaces_its_diagnostics_and_version() {
        let mut server = LspServer::new();
        initialize(&mut server);
        let uri = "wave-insight://source/broken.sv";
        did_open(&mut server, uri, BROKEN);
        let first = published(take_notifications(&mut server));
        assert_eq!(first.len(), 1);
        assert!(first[0]
            .get("params")
            .and_then(|params| params.get_array("diagnostics"))
            .is_some_and(|diagnostics| !diagnostics.is_empty()));

        notify(
            &mut server,
            "textDocument/didChange",
            Json::object([
                (
                    "textDocument",
                    Json::object([("uri", Json::string(uri)), ("version", Json::int(2))]),
                ),
                (
                    "contentChanges",
                    Json::Array(vec![Json::object([("text", Json::string(CLEAN))])]),
                ),
            ]),
        );
        let second = published(take_notifications(&mut server));
        assert_eq!(second.len(), 1, "{second:?}");
        let params = second[0].get("params").expect("params");
        assert_eq!(params.get_str("uri"), Some(uri));
        assert_eq!(params.get_i64("version"), Some(2));
        assert_eq!(
            params.get_array("diagnostics").map(<[Json]>::len),
            Some(0),
            "the fixed file must publish an empty array: {params:?}"
        );
    }

    #[test]
    fn closing_a_document_clears_its_diagnostics() {
        let mut server = LspServer::new();
        initialize(&mut server);
        let uri = "wave-insight://source/broken.sv";
        did_open(&mut server, uri, BROKEN);
        let first = published(take_notifications(&mut server));
        assert_eq!(first.len(), 1);

        notify(
            &mut server,
            "textDocument/didClose",
            Json::object([("textDocument", Json::object([("uri", Json::string(uri))]))]),
        );
        let cleared = published(take_notifications(&mut server));
        assert_eq!(cleared.len(), 1, "{cleared:?}");
        let params = cleared[0].get("params").expect("params");
        assert_eq!(params.get_str("uri"), Some(uri));
        assert_eq!(params.get_i64("version"), Some(1));
        assert_eq!(params.get_array("diagnostics").map(<[Json]>::len), Some(0));
    }

    #[test]
    fn a_preloaded_database_and_document_publish_elaboration_diagnostics() {
        let absolute = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root")
            .join("wave-insight-lib/tests/data/link_errors.sv");
        let db = DesignDb::load_files([absolute.clone()]);
        let mut server = LspServer::with_db(db);
        initialize(&mut server);
        server.set_top(Some("width_top".to_owned()));
        let uri = uri::to_uri(&absolute);
        let text = std::fs::read_to_string(&absolute).expect("read");
        did_open(&mut server, &uri, &text);
        let notifications = published(take_notifications(&mut server));
        assert_eq!(notifications.len(), 1, "{notifications:?}");
        let diagnostics = notifications[0]
            .get("params")
            .and_then(|params| params.get_array("diagnostics"))
            .expect("diagnostics");
        assert!(
            diagnostics
                .iter()
                .any(|diagnostic| diagnostic.get_str("code") == Some("width-mismatch")),
            "the elaboration diagnostic of the preloaded design was not published: {diagnostics:?}"
        );
    }

    // -- elaboration diagnostics merged into publishDiagnostics ------------

    #[test]
    fn top_module_change_republishes_elaboration_diagnostics() {
        let mut server = LspServer::with_db(DesignDb::load_files([fixture("link_errors.sv")]));
        initialize(&mut server);
        let uri = open_fixture(&mut server, "link_errors.sv");
        // The automatic top is the first uninstantiated module (link_top), so
        // the first publication carries its unresolved names.
        let _first = published(take_notifications(&mut server));

        server.set_top(Some("width_top".to_owned()));
        let notifications = published(take_notifications(&mut server));
        assert_eq!(notifications.len(), 1, "{notifications:?}");
        let params = notifications[0].get("params").expect("params");
        assert_eq!(params.get_str("uri"), Some(uri.as_str()));
        let diagnostics = params.get_array("diagnostics").expect("diagnostics array");

        let width: Vec<&Json> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.get_str("code") == Some("width-mismatch"))
            .collect();
        assert_eq!(
            width.len(),
            1,
            "expected one width mismatch: {diagnostics:?}"
        );
        let diagnostic = width[0];
        assert_eq!(diagnostic.get_i64("severity"), Some(2));
        assert_eq!(diagnostic.get_str("source"), Some("wave-insight"));
        assert!(
            diagnostic
                .get_str("message")
                .unwrap_or_default()
                .contains("4 bits"),
            "{diagnostic:?}"
        );
        // The range points at the port connection in the opened document.
        let text = fixture_text("link_errors.sv");
        let (start_line, start_character, end_line, end_character) = diagnostic_range(diagnostic);
        let expected = position_of(&text, "u_short");
        assert_eq!(start_line, i64::from(expected.line), "{diagnostic:?}");
        assert!(start_character >= 0 && end_line >= start_line);
        assert!(end_character < 200);
    }

    #[test]
    fn unresolved_names_are_published_for_the_document() {
        let mut server = LspServer::with_db(DesignDb::load_files([fixture("link_errors.sv")]));
        initialize(&mut server);
        // The top is configured before the file is opened, like a host that
        // knows the root from the waveform view.
        server.set_top(Some("link_top".to_owned()));
        let uri = open_fixture(&mut server, "link_errors.sv");
        let notifications = published(take_notifications(&mut server));
        assert_eq!(notifications.len(), 1, "{notifications:?}");
        let params = notifications[0].get("params").expect("params");
        assert_eq!(params.get_str("uri"), Some(uri.as_str()));
        let diagnostics = params.get_array("diagnostics").expect("diagnostics");
        let unresolved: Vec<&str> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.get_str("code") == Some("unresolved-name"))
            .filter_map(|diagnostic| diagnostic.get_str("message"))
            .collect();
        assert!(
            unresolved
                .iter()
                .any(|message| message.contains("missing_load")),
            "{unresolved:?}"
        );
        assert!(
            unresolved
                .iter()
                .any(|message| message.contains("missing_driver")),
            "{unresolved:?}"
        );
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.get_i64("severity") == Some(2)),
            "elaboration diagnostics are warnings: {diagnostics:?}"
        );
    }

    #[test]
    fn diagnostics_are_attributed_to_their_own_document() {
        let mut server = LspServer::with_db(DesignDb::load_files([fixture("link_errors.sv")]));
        initialize(&mut server);
        server.set_top(Some("link_top".to_owned()));

        // Two documents in one server: a parse error and an elaboration warning.
        let broken_uri = uri::to_uri(&fixture("malformed.sv"));
        did_open(&mut server, &broken_uri, &fixture_text("malformed.sv"));
        let link_uri = open_fixture(&mut server, "link_errors.sv");
        let notifications = published(take_notifications(&mut server));
        assert_eq!(notifications.len(), 2, "{notifications:?}");

        let codes = |uri: &str| -> Vec<String> {
            let notification = notifications
                .iter()
                .find(|notification| {
                    notification
                        .get("params")
                        .and_then(|params| params.get_str("uri"))
                        == Some(uri)
                })
                .unwrap_or_else(|| panic!("no diagnostics for {uri}"));
            notification
                .get("params")
                .and_then(|params| params.get_array("diagnostics"))
                .expect("diagnostics")
                .iter()
                .filter_map(|diagnostic| diagnostic.get_str("code").map(str::to_owned))
                .collect()
        };
        assert!(
            codes(&broken_uri).iter().any(|code| code == "parse"),
            "{:?}",
            codes(&broken_uri)
        );
        assert!(
            !codes(&link_uri).iter().any(|code| code == "parse"),
            "the clean file must not inherit another document's parse error: {:?}",
            codes(&link_uri)
        );
        assert!(
            codes(&link_uri)
                .iter()
                .any(|code| code == "unresolved-name"),
            "{:?}",
            codes(&link_uri)
        );
    }

    #[test]
    fn a_clean_top_publishes_an_empty_array_after_the_top_is_set() {
        let mut server =
            LspServer::with_db(DesignDb::load_files([fixture("top.sv"), fixture("cpu.sv")]));
        initialize(&mut server);
        server.set_top(Some("top".to_owned()));
        let uri = open_fixture(&mut server, "top.sv");
        let notifications = published(take_notifications(&mut server));
        assert_eq!(notifications.len(), 1, "{notifications:?}");
        let params = notifications[0].get("params").expect("params");
        assert_eq!(params.get_str("uri"), Some(uri.as_str()));
        assert_eq!(
            params.get_array("diagnostics").map(<[Json]>::len),
            Some(0),
            "{params:?}"
        );
    }
}
