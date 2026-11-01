//! SystemVerilog front end: preprocess + parse one or more files into a
//! [`crate::design::DesignDb`].
//!
//! This module is the *only* place that knows about `sv_parser`.  Everything
//! it produces is expressed with the crate local types in
//! [`crate::design::types`], so the parser dependency can be replaced without
//! changing a single public signature.
//!
//! # Generate
//!
//! Generate regions, constant generate-for loops and generate-if/case are
//! unrolled here with the existing constant evaluator and the module's default
//! parameter values.  Every iteration becomes ordinary signals/instances tagged
//! with its generate-block path (`g[2]`), so the elaborator, the source spans
//! and the driver/load tables work unchanged.  Non-constant bounds and
//! conditions stay warning diagnostics.  Limitation: the unroll happens at
//! parse time, so a per-instance parameter override that changes a generate
//! bound does not reshape the design (arrayed-instance bounds are re-evaluated
//! per instance during elaboration and do honour overrides).
//!
//! Robustness rules for this layer:
//!
//! * malformed input should not panic - parser errors, preprocessing errors and
//!   (best effort) a panic inside the third party parser become [`Diagnostic`]s.
//!   The best effort part needs unwinding: the shipped release profile sets
//!   `panic = "abort"`, so a parser panic aborts the app/webview there;
//! * every declaration remembers its source span;
//! * constructs outside the supported subset are skipped with a warning
//!   diagnostic instead of aborting the file.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use sv_parser::{
    parse_sv, parse_sv_str, AlwaysConstruct, AnsiPortDeclaration, CaseGenerateConstruct,
    CaseGenerateItem, ConditionalGenerateConstruct, ConstantParamExpression, ContinuousAssign,
    DataDeclaration, DataType, DataTypeOrImplicit, Defines, FinalConstruct, GenerateBlock,
    GenerateItem, GenerateRegion, GenvarIteration, HierarchicalIdentifier, IfGenerateConstruct,
    InitialConstruct, InputDeclaration, IntegerAtomType, IntegerVectorType, Iter,
    ListOfParameterAssignments, ListOfPortConnections, LocalParameterDeclaration, Locate,
    LoopGenerateConstruct, ModuleAnsiHeader, ModuleCommonItem, ModuleDeclaration,
    ModuleInstantiation, ModuleItem, ModuleNonansiHeader, NamedPortConnection, NetDeclaration,
    NetPortType, NodeEvent, NonPortModuleItem, OutputDeclaration, PackageOrGenerateItemDeclaration,
    PackedDimension, ParameterDeclaration, ParameterPortDeclaration, ParameterPortList,
    PortDeclaration, PortDirection, PsOrHierarchicalNetIdentifier, RefNode, RefNodes, SyntaxTree,
    UnpackedDimension, VarDataType, VariableDimension, VariablePortType,
};

/// Upper bound on the number of iterations one generate loop may unroll, so a
/// runaway or malicious bound cannot hang the parser.
const MAX_GENERATE_ITERATIONS: usize = 4096;

use super::const_eval::{dim_width, eval_const, ParamEnv};
use super::types::{
    Diagnostic, DiagnosticKind, Direction, InstanceDecl, ModuleDef, NameRef, ParamAssignment,
    ParamDef, PortConnectionDecl, PortDef, RangeDim, SignalDef, SignalKind, SignalUsage, Span,
    UsageKind, UsageRole,
};

/// Result of parsing one file.
pub(crate) struct ParsedFile {
    pub modules: Vec<ModuleDef>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parse `path` from disk (or `source` when given, which is how tests and
/// editor buffers get parsed without touching the file system).
pub(crate) fn parse_file(
    path: &Path,
    source: Option<&str>,
    include_paths: &[PathBuf],
) -> ParsedFile {
    let mut locator = Locator::new(path.to_path_buf());
    if let Some(source) = source {
        locator.register(path, source);
    }

    let parse_result = catch_unwind(AssertUnwindSafe(|| {
        let defines: Defines = Defines::new();
        match source {
            Some(source) => parse_sv_str(source, path, &defines, include_paths, false, false),
            None => parse_sv(path, &defines, include_paths, false, false),
        }
    }));

    let tree = match parse_result {
        Ok(Ok((tree, _defines))) => tree,
        Ok(Err(error)) => {
            return ParsedFile {
                modules: Vec::new(),
                diagnostics: vec![error_diagnostic(&mut locator, path, error)],
            };
        }
        Err(_) => {
            return ParsedFile {
                modules: Vec::new(),
                diagnostics: vec![Diagnostic::error(
                    DiagnosticKind::ParserPanic,
                    format!(
                        "the SystemVerilog parser panicked while reading {}; the file was skipped",
                        path.display()
                    ),
                    Some(Span::point(path.to_path_buf(), 1, 1)),
                )],
            };
        }
    };

    let extracted = catch_unwind(AssertUnwindSafe(|| {
        let extractor = Extractor {
            tree: &tree,
            locator,
            diagnostics: RefCell::new(Vec::new()),
            generate_scope: Vec::new(),
            next_genblk: 1,
        };
        extractor.run()
    }));

    match extracted {
        Ok(parsed) => parsed,
        Err(_) => ParsedFile {
            modules: Vec::new(),
            diagnostics: vec![Diagnostic::error(
                DiagnosticKind::ParserPanic,
                format!(
                    "internal error while reading {}; the file was skipped",
                    path.display()
                ),
                Some(Span::point(path.to_path_buf(), 1, 1)),
            )],
        },
    }
}

fn error_diagnostic(locator: &mut Locator, path: &Path, error: sv_parser::Error) -> Diagnostic {
    let kind = match &error {
        sv_parser::Error::Parse(_) => DiagnosticKind::Parse,
        sv_parser::Error::Preprocess(_) | sv_parser::Error::Include { .. } => {
            DiagnosticKind::Preprocess
        }
        sv_parser::Error::Io(_) | sv_parser::Error::File { .. } | sv_parser::Error::ReadUtf8(_) => {
            DiagnosticKind::Io
        }
        _ => DiagnosticKind::Preprocess,
    };
    let origin = match &error {
        sv_parser::Error::Parse(origin) | sv_parser::Error::Preprocess(origin) => origin.clone(),
        _ => None,
    };
    let span = origin.map(|(file, offset)| {
        let (line, col) = locator.pos(&file, offset);
        Span::point(file, line, col)
    });
    let span = span.or_else(|| Some(Span::point(path.to_path_buf(), 1, 1)));
    Diagnostic::error(kind, error.to_string(), span)
}

// ---------------------------------------------------------------------------
// Source position tracking
// ---------------------------------------------------------------------------

struct Locator {
    fallback: PathBuf,
    files: RefCell<HashMap<PathBuf, LineIndex>>,
}

struct LineIndex {
    text: String,
    /// Byte offset of the first character of every line.
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: String) -> Self {
        let mut starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(index + 1);
            }
        }
        LineIndex { text, starts }
    }

    fn line_col(&self, offset: usize) -> (u32, u32) {
        if self.text.is_empty() || self.starts.is_empty() {
            return (1, 1);
        }
        let mut offset = offset.min(self.text.len());
        while offset > 0 && !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|start| *start <= offset);
        let line_start = self.starts[line.saturating_sub(1)];
        let col = self.text[line_start..offset].chars().count() + 1;
        (line.max(1) as u32, col as u32)
    }
}

impl Locator {
    fn new(fallback: PathBuf) -> Self {
        Locator {
            fallback,
            files: RefCell::new(HashMap::new()),
        }
    }

    fn register(&mut self, path: &Path, text: &str) {
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), LineIndex::new(text.to_string()));
    }

    fn pos(&self, file: &Path, offset: usize) -> (u32, u32) {
        if !self.files.borrow().contains_key(file) {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            self.files
                .borrow_mut()
                .insert(file.to_path_buf(), LineIndex::new(text));
        }
        self.files
            .borrow()
            .get(file)
            .map(|index| index.line_col(offset))
            .unwrap_or((1, 1))
    }

    fn span_of_locates(&self, tree: &SyntaxTree, first: &Locate, last: &Locate) -> Span {
        let start_origin = tree
            .get_origin(first)
            .map(|(file, offset)| (file.clone(), offset));
        let end_origin = tree
            .get_origin(last)
            .map(|(file, offset)| (file.clone(), offset));
        match (start_origin, end_origin) {
            (Some((file, start_offset)), Some((end_file, end_offset))) if file == end_file => {
                let (start_line, start_col) = self.pos(&file, start_offset);
                let (end_line, end_col) = self.pos(&file, end_offset + last.len);
                Span {
                    file,
                    start_line,
                    start_col,
                    end_line,
                    end_col,
                }
            }
            (Some((file, start_offset)), _) => {
                let (line, col) = self.pos(&file, start_offset);
                Span::point(file, line, col)
            }
            (None, _) => {
                // Tokens produced by a macro expansion have no origin; fall
                // back to the preprocessed line number, which is still useful.
                let file = self.fallback.clone();
                let line = first.line.max(1);
                Span::point(file, line, 1)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Extraction
// ---------------------------------------------------------------------------

/// Everything needed to build one [`ModuleDef`] while walking a module body.
struct ModuleBuilder {
    name: String,
    span: Span,
    parameters: Vec<ParamDef>,
    env: ParamEnv,
    genvars: Vec<String>,
    /// ANSI ports in header order, or the final ports of a non-ANSI module.
    ports: Vec<PortDef>,
    /// Non-ANSI: port names in header order.
    port_order: Vec<(String, Span)>,
    /// Non-ANSI: port definitions found in `input`/`output` declarations.
    port_map: HashMap<String, PortDef>,
    signals: Vec<SignalDef>,
    signal_names: HashSet<String>,
    instances: Vec<InstanceDecl>,
    usages: Vec<SignalUsage>,
}

impl ModuleBuilder {
    fn new(name: String, span: Span) -> Self {
        ModuleBuilder {
            name,
            span,
            parameters: Vec::new(),
            env: ParamEnv::new(),
            genvars: Vec::new(),
            ports: Vec::new(),
            port_order: Vec::new(),
            port_map: HashMap::new(),
            signals: Vec::new(),
            signal_names: HashSet::new(),
            instances: Vec::new(),
            usages: Vec::new(),
        }
    }

    fn push_param(&mut self, param: ParamDef) {
        if let Some(default) = &param.default {
            if let Some(value) = eval_const(default, &self.env) {
                self.env.insert(param.name.clone(), value);
            }
        }
        self.parameters.push(param);
    }
    fn finish_ports(&mut self, diagnostics: &mut Vec<Diagnostic>) {
        // ANSI modules already have their ports in header order; only classic
        // non-ANSI modules need to be assembled from header names + body
        // declarations.
        if self.port_order.is_empty() && self.port_map.is_empty() {
            return;
        }
        let mut ordered = Vec::new();
        for (name, span) in std::mem::take(&mut self.port_order) {
            match self.port_map.remove(&name) {
                Some(mut port) => {
                    port.header_span = Some(span);
                    ordered.push(port);
                }
                None => {
                    diagnostics.push(Diagnostic::error(
                        DiagnosticKind::MissingPortDeclaration,
                        format!(
                            "port `{}` is listed in the module header but has no direction declaration",
                            name
                        ),
                        Some(span.clone()),
                    ));
                    ordered.push(PortDef {
                        name,
                        direction: Direction::Unknown,
                        kind: SignalKind::Other,
                        packed: Vec::new(),
                        unpacked: Vec::new(),
                        width: None,
                        span: span.clone(),
                        header_span: Some(span),
                    });
                }
            }
        }
        // Declarations for names that were not in the header are illegal but
        // still worth keeping: the UI can show them instead of silently
        // dropping the signal.
        let mut extras: Vec<PortDef> = self.port_map.drain().map(|(_, port)| port).collect();
        extras.sort_by(|a, b| {
            (a.span.start_line, a.span.start_col).cmp(&(b.span.start_line, b.span.start_col))
        });
        for port in extras {
            diagnostics.push(Diagnostic::warning(
                DiagnosticKind::MissingPortDeclaration,
                format!(
                    "port `{}` is declared but not listed in the module header",
                    port.name
                ),
                Some(port.span.clone()),
            ));
            ordered.push(port);
        }
        self.ports = ordered;
    }

    fn finish(self) -> ModuleDef {
        ModuleDef {
            name: self.name,
            span: self.span,
            parameters: self.parameters,
            genvars: self.genvars,
            ports: self.ports,
            signals: self.signals,
            instances: self.instances,
            usages: self.usages,
        }
    }
}

struct Extractor<'a> {
    tree: &'a SyntaxTree,
    locator: Locator,
    diagnostics: RefCell<Vec<Diagnostic>>,
    /// Generate-block path of the item being walked, outermost component first
    /// (e.g. ["g[2]"]).  Empty while walking the module body itself.
    generate_scope: Vec<String>,
    /// Next implicit name for an anonymous generate block (genblk1, ...).
    next_genblk: usize,
}

impl<'a> Extractor<'a> {
    fn run(mut self) -> ParsedFile {
        let mut modules = Vec::new();
        let tree: &'a SyntaxTree = self.tree;
        for event in tree.into_iter().event() {
            let NodeEvent::Enter(node) = event else {
                continue;
            };
            if let RefNode::Description(description) = node {
                match description {
                    sv_parser::Description::ModuleDeclaration(module) => {
                        if let Some(module) = self.extract_module(module.as_ref()) {
                            modules.push(module);
                        }
                    }
                    sv_parser::Description::InterfaceDeclaration(_) => {
                        self.unsupported("interface declaration", self.span_of(description))
                    }
                    sv_parser::Description::ProgramDeclaration(_) => {
                        self.unsupported("program declaration", self.span_of(description))
                    }
                    sv_parser::Description::UdpDeclaration(_) => {
                        self.unsupported("user defined primitive", self.span_of(description))
                    }
                    sv_parser::Description::InterfaceClassDeclaration(_) => {
                        self.unsupported("interface class declaration", self.span_of(description))
                    }
                    sv_parser::Description::PackageDeclaration(_) => {
                        self.unsupported("package declaration", self.span_of(description))
                    }
                    sv_parser::Description::ConfigDeclaration(_) => {
                        self.unsupported("config declaration", self.span_of(description))
                    }
                    sv_parser::Description::BindDirective(_) => {
                        self.unsupported("bind directive", self.span_of(description))
                    }
                    sv_parser::Description::ResetallCompilerDirective(_) => {}
                    sv_parser::Description::PackageItem(_) => {}
                }
            }
        }
        ParsedFile {
            modules,
            diagnostics: self.diagnostics.into_inner(),
        }
    }

    // -- generic helpers ---------------------------------------------------

    /// Concatenated source text of all non-whitespace tokens of `node`.
    fn text_of<'n>(&self, node: impl Into<RefNode<'n>>) -> String {
        let nodes = RefNodes(vec![node.into()]);
        let mut text = String::new();
        let mut skip = 0usize;
        for event in Iter::new(nodes).event() {
            match event {
                NodeEvent::Enter(RefNode::WhiteSpace(_)) => skip += 1,
                NodeEvent::Leave(RefNode::WhiteSpace(_)) => skip = skip.saturating_sub(1),
                NodeEvent::Enter(RefNode::Locate(locate)) if skip == 0 => {
                    if let Some(part) = self.tree.get_str(locate) {
                        text.push_str(part);
                    }
                }
                _ => {}
            }
        }
        text
    }

    fn span_of<'n>(&self, node: impl Into<RefNode<'n>>) -> Span {
        let nodes = RefNodes(vec![node.into()]);
        let mut first: Option<&'n Locate> = None;
        let mut last: Option<&'n Locate> = None;
        let mut skip = 0usize;
        for event in Iter::new(nodes).event() {
            match event {
                NodeEvent::Enter(RefNode::WhiteSpace(_)) => skip += 1,
                NodeEvent::Leave(RefNode::WhiteSpace(_)) => skip = skip.saturating_sub(1),
                NodeEvent::Enter(RefNode::Locate(locate)) if skip == 0 => {
                    if first.is_none() {
                        first = Some(locate);
                    }
                    last = Some(locate);
                }
                _ => {}
            }
        }
        match (first, last) {
            (Some(first), Some(last)) => {
                let tree = self.tree;
                self.locator.span_of_locates(tree, first, last)
            }
            _ => Span::point(self.locator.fallback.clone(), 1, 1),
        }
    }

    fn identifier_text(&self, identifier: &sv_parser::Identifier) -> String {
        self.text_of(identifier).trim().to_string()
    }

    fn hierarchical_name(&self, identifier: &HierarchicalIdentifier) -> String {
        let mut parts = Vec::new();
        for (id, _select, _dot) in &identifier.nodes.1 {
            parts.push(self.identifier_text(id));
        }
        parts.push(self.identifier_text(&identifier.nodes.2));
        parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(".")
    }

    fn unsupported(&self, what: &str, span: Span) {
        self.diagnostics.borrow_mut().push(Diagnostic::warning(
            DiagnosticKind::UnsupportedConstruct,
            format!("unsupported construct: {}; ignored", what),
            Some(span),
        ));
    }

    fn duplicate(&self, what: &str, name: &str, span: Span) {
        self.diagnostics.borrow_mut().push(Diagnostic::error(
            DiagnosticKind::DuplicateDeclaration,
            format!("duplicate {} `{}`", what, name),
            Some(span),
        ));
    }

    // -- module level ------------------------------------------------------

    fn extract_module(&mut self, declaration: &ModuleDeclaration) -> Option<ModuleDef> {
        match declaration {
            ModuleDeclaration::Ansi(module) => self.extract_ansi_module(module),
            ModuleDeclaration::Nonansi(module) => self.extract_nonansi_module(module),
            ModuleDeclaration::Wildcard(module) => {
                self.unsupported("wildcard module declaration", self.span_of(module.as_ref()));
                None
            }
            ModuleDeclaration::ExternNonansi(module) => {
                self.unsupported("extern module declaration", self.span_of(module.as_ref()));
                None
            }
            ModuleDeclaration::ExternAnsi(module) => {
                self.unsupported("extern module declaration", self.span_of(module.as_ref()));
                None
            }
        }
    }

    fn extract_ansi_module(
        &mut self,
        module: &sv_parser::ModuleDeclarationAnsi,
    ) -> Option<ModuleDef> {
        let header: &ModuleAnsiHeader = &module.nodes.0;
        let name = self.text_of(&header.nodes.3).trim().to_string();
        let span = self.span_of(&header.nodes.3);
        let mut builder = ModuleBuilder::new(name, span);
        self.generate_scope.clear();
        self.next_genblk = 1;
        self.extract_parameter_port_list(&mut builder, header.nodes.5.as_ref());
        self.extract_ansi_ports_header(&mut builder, header);
        for item in &module.nodes.2 {
            self.walk_non_port_item(&mut builder, item);
        }
        builder.finish_ports(&mut self.diagnostics.borrow_mut());
        self.check_duplicate_ports(&builder);
        Some(builder.finish())
    }

    fn extract_nonansi_module(
        &mut self,
        module: &sv_parser::ModuleDeclarationNonansi,
    ) -> Option<ModuleDef> {
        let header: &ModuleNonansiHeader = &module.nodes.0;
        let name = self.text_of(&header.nodes.3).trim().to_string();
        let span = self.span_of(&header.nodes.3);
        let mut builder = ModuleBuilder::new(name, span);
        self.generate_scope.clear();
        self.next_genblk = 1;
        self.extract_parameter_port_list(&mut builder, header.nodes.5.as_ref());
        self.collect_port_list_names(&mut builder, &header.nodes.6);
        for item in &module.nodes.2 {
            match item {
                ModuleItem::PortDeclaration(port) => {
                    self.handle_port_declaration(&mut builder, &port.0)
                }
                ModuleItem::NonPortModuleItem(item) => self.walk_non_port_item(&mut builder, item),
            }
        }
        builder.finish_ports(&mut self.diagnostics.borrow_mut());
        self.check_duplicate_ports(&builder);
        Some(builder.finish())
    }

    fn check_duplicate_ports(&mut self, builder: &ModuleBuilder) {
        let mut seen = HashSet::new();
        for port in &builder.ports {
            if !seen.insert(port.name.clone()) {
                self.duplicate("port", &port.name, port.span.clone());
            }
        }
        let mut seen = HashSet::new();
        for signal in &builder.signals {
            if !seen.insert(signal.name.clone()) {
                self.duplicate("signal", &signal.name, signal.span.clone());
            }
        }
        let mut seen = HashSet::new();
        for param in &builder.parameters {
            if !seen.insert(param.name.clone()) {
                self.duplicate("parameter", &param.name, param.span.clone());
            }
        }
        let mut seen = HashSet::new();
        for instance in &builder.instances {
            if !seen.insert(instance.name.clone()) {
                self.duplicate("instance", &instance.name, instance.span.clone());
            }
        }
    }

    fn collect_port_list_names(
        &mut self,
        builder: &mut ModuleBuilder,
        list: &sv_parser::ListOfPorts,
    ) {
        let ports = &list.nodes.0.nodes.1;
        for port in ports.contents() {
            match port {
                sv_parser::Port::NonNamed(port) => {
                    if let Some(expression) = &port.nodes.0 {
                        match expression {
                            sv_parser::PortExpression::PortReference(reference) => {
                                let name = self.text_of(&reference.nodes.0).trim().to_string();
                                let span = self.span_of(&reference.nodes.0);
                                builder.port_order.push((name, span));
                            }
                            sv_parser::PortExpression::Brace(_) => {
                                self.unsupported(
                                    "multiple port expressions in a non-ANSI port list",
                                    self.span_of(port.as_ref()),
                                );
                            }
                        }
                    }
                }
                sv_parser::Port::Named(port) => {
                    let name = self.text_of(&port.nodes.1).trim().to_string();
                    let span = self.span_of(&port.nodes.1);
                    if let Some(expression) = &port.nodes.2.nodes.1 {
                        let expression_text = self.text_of(expression).trim().to_string();
                        if expression_text != name {
                            self.diagnostics.borrow_mut().push(Diagnostic::warning(
                                DiagnosticKind::UnsupportedConstruct,
                                format!(
                                    "explicit port expression `{}` in the module header is ignored; the port is named `{}`",
                                    expression_text, name
                                ),
                                Some(self.span_of(expression)),
                            ));
                        }
                    }
                    builder.port_order.push((name, span));
                }
            }
        }
    }

    // -- parameters --------------------------------------------------------

    fn extract_parameter_port_list(
        &mut self,
        builder: &mut ModuleBuilder,
        list: Option<&ParameterPortList>,
    ) {
        let Some(list) = list else {
            return;
        };
        match list {
            ParameterPortList::Assignment(assignment) => {
                for item in assignment.nodes.1.nodes.1 .0.nodes.0.contents() {
                    self.push_param_assignment(builder, item, false);
                }
                for (_comma, declaration) in assignment.nodes.1.nodes.1 .1.iter() {
                    self.push_parameter_port_declaration(builder, declaration);
                }
            }
            ParameterPortList::Declaration(declaration) => {
                for item in declaration.nodes.1.nodes.1.contents() {
                    self.push_parameter_port_declaration(builder, item);
                }
            }
            ParameterPortList::Empty(_) => {}
        }
    }

    fn push_parameter_port_declaration(
        &mut self,
        builder: &mut ModuleBuilder,
        declaration: &ParameterPortDeclaration,
    ) {
        match declaration {
            ParameterPortDeclaration::ParameterDeclaration(parameter) => {
                self.push_parameter_declaration(builder, parameter, false)
            }
            ParameterPortDeclaration::LocalParameterDeclaration(parameter) => {
                self.push_local_parameter_declaration(builder, parameter)
            }
            ParameterPortDeclaration::ParamList(list) => {
                for item in list.nodes.1.nodes.0.contents() {
                    self.push_param_assignment(builder, item, false);
                }
            }
            ParameterPortDeclaration::TypeList(list) => {
                for item in list.nodes.1.nodes.0.contents() {
                    let name = self.text_of(&item.nodes.0).trim().to_string();
                    let span = self.span_of(&item.nodes.0);
                    builder.push_param(ParamDef {
                        name,
                        default: None,
                        is_local: false,
                        span,
                    });
                }
                self.unsupported("type parameter", self.span_of(declaration));
            }
        }
    }

    fn push_parameter_declaration(
        &mut self,
        builder: &mut ModuleBuilder,
        declaration: &ParameterDeclaration,
        is_local: bool,
    ) {
        match declaration {
            ParameterDeclaration::Param(parameter) => {
                for item in parameter.nodes.2.nodes.0.contents() {
                    self.push_param_assignment(builder, item, is_local);
                }
            }
            ParameterDeclaration::Type(_) => {
                self.unsupported("type parameter", self.span_of(declaration))
            }
        }
    }

    fn push_local_parameter_declaration(
        &mut self,
        builder: &mut ModuleBuilder,
        declaration: &LocalParameterDeclaration,
    ) {
        match declaration {
            LocalParameterDeclaration::Param(parameter) => {
                for item in parameter.nodes.2.nodes.0.contents() {
                    self.push_param_assignment(builder, item, true);
                }
            }
            LocalParameterDeclaration::Type(_) => {
                self.unsupported("type parameter", self.span_of(declaration))
            }
        }
    }

    fn push_param_assignment(
        &mut self,
        builder: &mut ModuleBuilder,
        assignment: &sv_parser::ParamAssignment,
        is_local: bool,
    ) {
        let name = self.text_of(&assignment.nodes.0).trim().to_string();
        if name.is_empty() {
            return;
        }
        let span = self.span_of(&assignment.nodes.0);
        let default = assignment.nodes.2.as_ref().map(
            |(_symbol, value): &(sv_parser::Symbol, ConstantParamExpression)| {
                self.text_of(value).trim().to_string()
            },
        );
        builder.push_param(ParamDef {
            name,
            default,
            is_local,
            span,
        });
    }

    // -- ports -------------------------------------------------------------

    fn extract_ansi_ports_header(
        &mut self,
        builder: &mut ModuleBuilder,
        header: &ModuleAnsiHeader,
    ) {
        let Some(list) = &header.nodes.6 else {
            return;
        };
        let Some(ports) = &list.nodes.0.nodes.1 else {
            return;
        };
        let mut state: Option<AnsiPortState> = None;
        for (_attributes, port) in ports.contents() {
            match port {
                AnsiPortDeclaration::Net(port) => {
                    let name = self.text_of(&port.nodes.1).trim().to_string();
                    let span = self.span_of(&port.nodes.1);
                    let unpacked = self.unpacked_dims(&port.nodes.2);
                    let header_info = match &port.nodes.0 {
                        Some(sv_parser::NetPortHeaderOrInterfacePortHeader::NetPortHeader(
                            header,
                        )) => {
                            let direction = header.nodes.0.as_ref().map(|d| self.direction_of(d));
                            let info = self.type_info_net_port_type(&header.nodes.1);
                            Some((direction, info))
                        }
                        Some(
                            sv_parser::NetPortHeaderOrInterfacePortHeader::InterfacePortHeader(_),
                        ) => {
                            self.unsupported("interface port", span.clone());
                            None
                        }
                        None => None,
                    };
                    self.push_ansi_port(builder, name, span, header_info, unpacked, &mut state);
                }
                AnsiPortDeclaration::Variable(port) => {
                    let name = self.text_of(&port.nodes.1).trim().to_string();
                    let span = self.span_of(&port.nodes.1);
                    let unpacked = self.variable_unpacked_dims(&port.nodes.2);
                    let header_info = port.nodes.0.as_ref().map(|header| {
                        let direction = header.nodes.0.as_ref().map(|d| self.direction_of(d));
                        let info = self.type_info_variable_port_type(&header.nodes.1);
                        (direction, info)
                    });
                    self.push_ansi_port(builder, name, span, header_info, unpacked, &mut state);
                }
                AnsiPortDeclaration::Paren(port) => {
                    let name = self.text_of(&port.nodes.2).trim().to_string();
                    let span = self.span_of(&port.nodes.2);
                    self.unsupported("explicit port expression", span.clone());
                    let header_info = port.nodes.0.as_ref().map(|direction| {
                        let direction = self.direction_of(direction);
                        (Some(direction), TypeInfo::scalar())
                    });
                    self.push_ansi_port(builder, name, span, header_info, Vec::new(), &mut state);
                }
            }
        }
    }

    fn push_ansi_port(
        &mut self,
        builder: &mut ModuleBuilder,
        name: String,
        span: Span,
        header_info: Option<(Option<Direction>, TypeInfo)>,
        unpacked: Vec<RangeDim>,
        state: &mut Option<AnsiPortState>,
    ) {
        if name.is_empty() {
            return;
        }
        let (direction, info) = match header_info {
            Some((direction, info)) => {
                let direction = direction.unwrap_or_else(|| {
                    state
                        .as_ref()
                        .map(|s| s.direction)
                        .unwrap_or(Direction::Unknown)
                });
                let state = state.get_or_insert_with(|| AnsiPortState {
                    direction,
                    info: info.clone(),
                });
                state.direction = direction;
                state.info = info.clone();
                (direction, info)
            }
            None => match state {
                Some(state) => (state.direction, state.info.clone()),
                None => {
                    self.diagnostics.borrow_mut().push(Diagnostic::warning(
                        DiagnosticKind::UnsupportedConstruct,
                        format!(
                            "port `{}` has no direction and no preceding port to inherit it from",
                            name
                        ),
                        Some(span.clone()),
                    ));
                    (Direction::Unknown, TypeInfo::scalar())
                }
            },
        };
        let width = type_width(&info, &builder.env);
        let port = PortDef {
            name: name.clone(),
            direction,
            kind: info.kind.unwrap_or(SignalKind::Other),
            packed: info.packed.clone(),
            unpacked,
            width,
            span,
            header_span: None,
        };
        if builder.ports.iter().any(|existing| existing.name == name) {
            self.duplicate("port", &name, port.span.clone());
            return;
        }
        builder.ports.push(port);
    }

    fn direction_of(&self, direction: &PortDirection) -> Direction {
        match direction {
            PortDirection::Input(_) => Direction::Input,
            PortDirection::Output(_) => Direction::Output,
            PortDirection::Inout(_) => Direction::Inout,
            PortDirection::Ref(_) => Direction::Ref,
        }
    }

    fn handle_port_declaration(
        &mut self,
        builder: &mut ModuleBuilder,
        declaration: &PortDeclaration,
    ) {
        match declaration {
            PortDeclaration::Input(declaration) => match &declaration.nodes.1 {
                InputDeclaration::Net(net) => {
                    let info = self.type_info_net_port_type(&net.nodes.1);
                    self.push_port_names(
                        builder,
                        Direction::Input,
                        &info,
                        PortNameList::Port(&net.nodes.2),
                    );
                }
                InputDeclaration::Variable(variable) => {
                    let info = self.type_info_variable_port_type(&variable.nodes.1);
                    self.push_port_names(
                        builder,
                        Direction::Input,
                        &info,
                        PortNameList::Variable(&variable.nodes.2),
                    );
                }
            },
            PortDeclaration::Output(declaration) => match &declaration.nodes.1 {
                OutputDeclaration::Net(net) => {
                    let info = self.type_info_net_port_type(&net.nodes.1);
                    self.push_port_names(
                        builder,
                        Direction::Output,
                        &info,
                        PortNameList::Port(&net.nodes.2),
                    );
                }
                OutputDeclaration::Variable(variable) => {
                    let info = self.type_info_variable_port_type(&variable.nodes.1);
                    self.push_port_names(
                        builder,
                        Direction::Output,
                        &info,
                        PortNameList::VariablePort(&variable.nodes.2),
                    );
                }
            },
            PortDeclaration::Inout(declaration) => {
                let inout = &declaration.nodes.1;
                let info = self.type_info_net_port_type(&inout.nodes.1);
                self.push_port_names(
                    builder,
                    Direction::Inout,
                    &info,
                    PortNameList::Port(&inout.nodes.2),
                );
            }
            PortDeclaration::Ref(declaration) => {
                let reference = &declaration.nodes.1;
                let info = self.type_info_variable_port_type(&reference.nodes.1);
                self.push_port_names(
                    builder,
                    Direction::Ref,
                    &info,
                    PortNameList::Variable(&reference.nodes.2),
                );
            }
            PortDeclaration::Interface(_) => {
                self.unsupported("interface port declaration", self.span_of(declaration))
            }
        }
    }

    fn push_port_names(
        &mut self,
        builder: &mut ModuleBuilder,
        direction: Direction,
        info: &TypeInfo,
        names: PortNameList<'_>,
    ) {
        let width = type_width(info, &builder.env);
        match names {
            PortNameList::Port(list) => {
                for (identifier, dimensions) in list.nodes.0.contents() {
                    let name = self.text_of(identifier).trim().to_string();
                    let span = self.span_of(identifier);
                    let unpacked = self.unpacked_dims(dimensions);
                    self.insert_port(builder, direction, info, name, span, unpacked, width);
                }
            }
            PortNameList::Variable(list) => {
                for (identifier, dimensions) in list.nodes.0.contents() {
                    let name = self.text_of(identifier).trim().to_string();
                    let span = self.span_of(identifier);
                    let unpacked = self.variable_unpacked_dims(dimensions);
                    self.insert_port(builder, direction, info, name, span, unpacked, width);
                }
            }
            PortNameList::VariablePort(list) => {
                for (identifier, dimensions, _default) in list.nodes.0.contents() {
                    let name = self.text_of(identifier).trim().to_string();
                    let span = self.span_of(identifier);
                    let unpacked = self.variable_unpacked_dims(dimensions);
                    self.insert_port(builder, direction, info, name, span, unpacked, width);
                }
            }
        }
    }

    //one argument per piece of the declaration being recorded (direction, type,
    //name, span, defaults); grouping them into a struct would hide that
    #[allow(clippy::too_many_arguments)]
    fn insert_port(
        &mut self,
        builder: &mut ModuleBuilder,
        direction: Direction,
        info: &TypeInfo,
        name: String,
        span: Span,
        unpacked: Vec<RangeDim>,
        width: Option<u32>,
    ) {
        if name.is_empty() {
            return;
        }
        if builder.port_map.contains_key(&name) {
            self.duplicate("port", &name, span);
            return;
        }
        builder.port_map.insert(
            name.clone(),
            PortDef {
                name,
                direction,
                kind: info.kind.unwrap_or(SignalKind::Other),
                packed: info.packed.clone(),
                unpacked,
                width,
                span,
                header_span: None,
            },
        );
    }

    // -- module items ------------------------------------------------------

    fn walk_non_port_item(&mut self, builder: &mut ModuleBuilder, item: &NonPortModuleItem) {
        match item {
            NonPortModuleItem::ModuleOrGenerateItem(item) => {
                self.walk_module_or_generate_item(builder, item)
            }
            NonPortModuleItem::GenerateRegion(region) => self.walk_generate_region(builder, region),
            NonPortModuleItem::SpecifyBlock(block) => {
                self.unsupported("specify block", self.span_of(block.as_ref()))
            }
            NonPortModuleItem::Specparam(_) => {
                self.unsupported("specparam declaration", self.span_of(item))
            }
            NonPortModuleItem::ProgramDeclaration(_) => {
                self.unsupported("nested program declaration", self.span_of(item))
            }
            NonPortModuleItem::ModuleDeclaration(_) => {
                self.unsupported("nested module declaration", self.span_of(item))
            }
            NonPortModuleItem::InterfaceDeclaration(_) => {
                self.unsupported("interface declaration", self.span_of(item))
            }
            NonPortModuleItem::TimeunitsDeclaration(_) => {}
        }
    }

    fn walk_module_or_generate_item(
        &mut self,
        builder: &mut ModuleBuilder,
        item: &sv_parser::ModuleOrGenerateItem,
    ) {
        match item {
            sv_parser::ModuleOrGenerateItem::Module(module) => {
                self.handle_module_instantiation(builder, &module.nodes.1)
            }
            sv_parser::ModuleOrGenerateItem::ModuleItem(item) => {
                self.walk_common_item(builder, &item.nodes.1)
            }
            sv_parser::ModuleOrGenerateItem::Parameter(parameter) => {
                self.unsupported("defparam", self.span_of(&parameter.nodes.1))
            }
            sv_parser::ModuleOrGenerateItem::Gate(_) => {
                self.unsupported("gate instantiation", self.span_of(item))
            }
            sv_parser::ModuleOrGenerateItem::Udp(_) => {
                self.unsupported("user defined primitive instantiation", self.span_of(item))
            }
        }
    }

    // -- generate ----------------------------------------------------------

    fn walk_generate_region(&mut self, builder: &mut ModuleBuilder, region: &GenerateRegion) {
        for item in &region.nodes.1 {
            self.walk_generate_item(builder, item);
        }
    }

    fn walk_generate_item(&mut self, builder: &mut ModuleBuilder, item: &GenerateItem) {
        match item {
            GenerateItem::ModuleOrGenerateItem(item) => {
                self.walk_module_or_generate_item(builder, item)
            }
            GenerateItem::InterfaceOrGenerateItem(_) => {
                self.unsupported("interface item in a generate block", self.span_of(item))
            }
            GenerateItem::CheckerOrGenerateItem(_) => {
                self.unsupported("checker item in a generate block", self.span_of(item))
            }
        }
    }

    /// Label of a named generate block: "begin : name", "name : begin" or
    /// "end : name".  None for an anonymous block.
    fn generate_block_label(&mut self, block: &GenerateBlock) -> Option<String> {
        let GenerateBlock::Multiple(block) = block else {
            return None;
        };
        let candidates = [
            block.nodes.0.as_ref().map(|pair| &pair.0),
            block.nodes.2.as_ref().map(|pair| &pair.1),
            block.nodes.5.as_ref().map(|pair| &pair.1),
        ];
        for identifier in candidates.into_iter().flatten() {
            let label = self.text_of(identifier).trim().to_string();
            if !label.is_empty() {
                return Some(label);
            }
        }
        None
    }

    /// Next implicit generate-block name (genblk1, genblk2, ... per module).
    fn anonymous_generate_label(&mut self) -> String {
        let label = format!("genblk{}", self.next_genblk);
        self.next_genblk += 1;
        label
    }

    fn walk_generate_block(&mut self, builder: &mut ModuleBuilder, block: &GenerateBlock) {
        let label = self
            .generate_block_label(block)
            .unwrap_or_else(|| self.anonymous_generate_label());
        self.generate_scope.push(label);
        self.walk_generate_block_items(builder, block);
        self.generate_scope.pop();
    }

    fn walk_generate_block_items(&mut self, builder: &mut ModuleBuilder, block: &GenerateBlock) {
        match block {
            GenerateBlock::GenerateItem(item) => self.walk_generate_item(builder, item),
            GenerateBlock::Multiple(block) => {
                for item in &block.nodes.3 {
                    self.walk_generate_item(builder, item);
                }
            }
        }
    }

    /// Unroll a constant generate-for: every iteration walks the body with the
    /// genvar bound in the module's parameter environment, so the declarations
    /// and instances it contains get distinct paths like "g[2].u".
    fn walk_generate_loop(
        &mut self,
        builder: &mut ModuleBuilder,
        generate: &LoopGenerateConstruct,
    ) {
        let initialization = &generate.nodes.1.nodes.1 .0;
        let genvar = self.text_of(&initialization.nodes.1).trim().to_string();
        if genvar.is_empty() {
            return;
        }
        if !builder.genvars.contains(&genvar) {
            builder.genvars.push(genvar.clone());
        }
        let initialization = self.text_of(&initialization.nodes.3).trim().to_string();
        let condition = self
            .text_of(&generate.nodes.1.nodes.1 .2)
            .trim()
            .to_string();
        let iteration = &generate.nodes.1.nodes.1 .4;
        let body = &generate.nodes.2;
        let label = self
            .generate_block_label(body)
            .unwrap_or_else(|| self.anonymous_generate_label());

        let Some(mut value) = eval_const(&initialization, &builder.env) else {
            self.unsupported(
                "generate loop with a non-constant initial value",
                self.span_of(generate),
            );
            return;
        };
        let previous = builder.env.get(&genvar).copied();
        let mut iterations = 0usize;
        loop {
            if iterations >= MAX_GENERATE_ITERATIONS {
                self.diagnostics.borrow_mut().push(Diagnostic::warning(
                    DiagnosticKind::RecursionLimit,
                    format!(
                        "generate loop over the genvar exceeds {} iterations; the remaining ones were not elaborated",
                        MAX_GENERATE_ITERATIONS
                    ),
                    Some(self.span_of(generate)),
                ));
                break;
            }
            builder.env.insert(genvar.clone(), value);
            let Some(condition_value) = eval_const(&condition, &builder.env) else {
                self.unsupported(
                    "generate loop with a non-constant condition",
                    self.span_of(generate),
                );
                break;
            };
            if condition_value == 0 {
                break;
            }
            iterations += 1;
            self.generate_scope.push(format!("{label}[{value}]"));
            self.walk_generate_block_items(builder, body);
            self.generate_scope.pop();
            let Some(next) = self.generate_iteration_value(iteration, value, &builder.env) else {
                self.unsupported(
                    "generate loop step the constant evaluator cannot apply",
                    self.span_of(generate),
                );
                break;
            };
            if next == value {
                self.unsupported(
                    "generate loop step does not change the genvar",
                    self.span_of(generate),
                );
                break;
            }
            value = next;
        }
        match previous {
            Some(previous) => {
                builder.env.insert(genvar, previous);
            }
            None => {
                builder.env.remove(&genvar);
            }
        }
    }

    /// One iteration step of a generate loop: ++, -- or "genvar = expression".
    fn generate_iteration_value(
        &mut self,
        iteration: &GenvarIteration,
        current: i64,
        env: &ParamEnv,
    ) -> Option<i64> {
        match iteration {
            GenvarIteration::Suffix(suffix) => {
                let operator = self.text_of(&suffix.nodes.1).trim().to_string();
                match operator.as_str() {
                    "++" => current.checked_add(1),
                    "--" => current.checked_sub(1),
                    _ => None,
                }
            }
            GenvarIteration::Prefix(prefix) => {
                let operator = self.text_of(&prefix.nodes.0).trim().to_string();
                match operator.as_str() {
                    "++" => current.checked_add(1),
                    "--" => current.checked_sub(1),
                    _ => None,
                }
            }
            GenvarIteration::Assignment(assignment) => {
                let operator = self.text_of(&assignment.nodes.1).trim().to_string();
                if operator != "=" {
                    return None;
                }
                let expression = self.text_of(&assignment.nodes.2).trim().to_string();
                eval_const(&expression, env)
            }
        }
    }

    fn walk_generate_conditional(
        &mut self,
        builder: &mut ModuleBuilder,
        generate: &ConditionalGenerateConstruct,
    ) {
        match generate {
            ConditionalGenerateConstruct::If(if_generate) => {
                self.walk_generate_if(builder, if_generate)
            }
            ConditionalGenerateConstruct::Case(case_generate) => {
                self.walk_generate_case(builder, case_generate)
            }
        }
    }

    fn walk_generate_if(&mut self, builder: &mut ModuleBuilder, generate: &IfGenerateConstruct) {
        let condition = self.text_of(&generate.nodes.1.nodes.1).trim().to_string();
        let Some(value) = eval_const(&condition, &builder.env) else {
            self.unsupported(
                "generate-if with a non-constant condition",
                self.span_of(generate),
            );
            return;
        };
        if value != 0 {
            self.walk_generate_block(builder, &generate.nodes.2);
        } else if let Some((_keyword, block)) = &generate.nodes.3 {
            self.walk_generate_block(builder, block);
        }
    }

    fn walk_generate_case(
        &mut self,
        builder: &mut ModuleBuilder,
        generate: &CaseGenerateConstruct,
    ) {
        let selector = self.text_of(&generate.nodes.1.nodes.1).trim().to_string();
        let Some(selector) = eval_const(&selector, &builder.env) else {
            self.unsupported(
                "generate-case with a non-constant selector",
                self.span_of(generate),
            );
            return;
        };
        let mut default = None;
        for item in &generate.nodes.2 {
            match item {
                CaseGenerateItem::Nondefault(item) => {
                    let mut matched = false;
                    for (_symbol, label) in &item.nodes.0.nodes.1 {
                        let text = self.text_of(label).trim().to_string();
                        if eval_const(&text, &builder.env) == Some(selector) {
                            matched = true;
                            break;
                        }
                    }
                    if matched {
                        self.walk_generate_block(builder, &item.nodes.2);
                        return;
                    }
                }
                CaseGenerateItem::Default(item) => default = Some(&item.nodes.2),
            }
        }
        if let Some(block) = default {
            self.walk_generate_block(builder, block);
        }
    }

    fn walk_common_item(&mut self, builder: &mut ModuleBuilder, item: &ModuleCommonItem) {
        match item {
            ModuleCommonItem::ModuleOrGenerateItemDeclaration(declaration) => {
                match declaration.as_ref() {
                    sv_parser::ModuleOrGenerateItemDeclaration::PackageOrGenerateItemDeclaration(
                        declaration,
                    ) => self.walk_declaration(builder, declaration.as_ref()),
                    sv_parser::ModuleOrGenerateItemDeclaration::GenvarDeclaration(declaration) => {
                        // The declaration itself stays a warning (it is a
                        // constant, not a signal), but the names are recorded so
                        // driver/load diagnostics do not treat them as unknown.
                        for identifier in declaration.nodes.1.nodes.0.contents() {
                            let name = self.text_of(identifier).trim().to_string();
                            if !name.is_empty() && !builder.genvars.contains(&name) {
                                builder.genvars.push(name);
                            }
                        }
                        self.unsupported("genvar declaration", self.span_of(item))
                    }
                    sv_parser::ModuleOrGenerateItemDeclaration::ClockingDeclaration(_) => {
                        self.unsupported("clocking declaration", self.span_of(item))
                    }
                    sv_parser::ModuleOrGenerateItemDeclaration::Clocking(_) => {
                        self.unsupported("clocking block", self.span_of(item))
                    }
                    sv_parser::ModuleOrGenerateItemDeclaration::Disable(_) => {
                        self.unsupported("disable statement", self.span_of(item))
                    }
                }
            }
            ModuleCommonItem::ContinuousAssign(assign) => {
                self.handle_continuous_assign(builder, assign)
            }
            ModuleCommonItem::AlwaysConstruct(always) => self.handle_procedural(builder, always),
            ModuleCommonItem::InitialConstruct(initial) => self.handle_initial(builder, initial),
            ModuleCommonItem::FinalConstruct(final_construct) => {
                self.handle_final(builder, final_construct)
            }
            ModuleCommonItem::LoopGenerateConstruct(generate) => {
                self.walk_generate_loop(builder, generate)
            }
            ModuleCommonItem::ConditionalGenerateConstruct(generate) => {
                self.walk_generate_conditional(builder, generate)
            }
            ModuleCommonItem::NetAlias(_) => self.unsupported("net alias", self.span_of(item)),
            ModuleCommonItem::AssertionItem(_) => {
                self.unsupported("assertion item", self.span_of(item))
            }
            ModuleCommonItem::BindDirective(_) => {
                self.unsupported("bind directive", self.span_of(item))
            }
            ModuleCommonItem::InterfaceInstantiation(_) => {
                self.unsupported("interface instantiation", self.span_of(item))
            }
            ModuleCommonItem::ProgramInstantiation(_) => {
                self.unsupported("program instantiation", self.span_of(item))
            }
            ModuleCommonItem::ElaborationSystemTask(_) => {}
        }
    }

    fn walk_declaration(
        &mut self,
        builder: &mut ModuleBuilder,
        declaration: &PackageOrGenerateItemDeclaration,
    ) {
        match declaration {
            PackageOrGenerateItemDeclaration::NetDeclaration(net) => {
                self.handle_net_declaration(builder, net)
            }
            PackageOrGenerateItemDeclaration::DataDeclaration(data) => {
                self.handle_data_declaration(builder, data)
            }
            PackageOrGenerateItemDeclaration::ParameterDeclaration(parameter) => {
                if self.generate_scope.is_empty() {
                    self.push_parameter_declaration(builder, &parameter.0, false)
                } else {
                    // A parameter/localparam inside a generate block is a
                    // constant local to that block, not a module parameter: bind
                    // it in the environment used for widths and loop bounds.
                    self.bind_generate_parameter(builder, &parameter.0, false)
                }
            }
            PackageOrGenerateItemDeclaration::LocalParameterDeclaration(parameter) => {
                if self.generate_scope.is_empty() {
                    self.push_local_parameter_declaration(builder, &parameter.0)
                } else {
                    self.bind_generate_local_parameter(builder, &parameter.0)
                }
            }
            PackageOrGenerateItemDeclaration::Empty(_) => {}
            PackageOrGenerateItemDeclaration::TaskDeclaration(_) => {
                self.unsupported("task declaration", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::FunctionDeclaration(_) => {
                self.unsupported("function declaration", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::CheckerDeclaration(_) => {
                self.unsupported("checker declaration", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::DpiImportExport(_) => {
                self.unsupported("DPI declaration", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::ExternConstraintDeclaration(_) => {
                self.unsupported("extern constraint declaration", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::ClassDeclaration(_) => {
                self.unsupported("class declaration", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::InterfaceClassDeclaration(_) => {
                self.unsupported("interface class declaration", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::ClassConstructorDeclaration(_) => {
                self.unsupported("class constructor", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::CovergroupDeclaration(_) => {
                self.unsupported("covergroup declaration", self.span_of(declaration))
            }
            PackageOrGenerateItemDeclaration::AssertionItemDeclaration(_) => {
                self.unsupported("assertion item", self.span_of(declaration))
            }
        }
    }

    /// Bind the parameters of a generate-local parameter declaration in the
    /// module environment (they are not part of the module's parameter list).
    fn bind_generate_parameter(
        &mut self,
        builder: &mut ModuleBuilder,
        declaration: &ParameterDeclaration,
        is_local: bool,
    ) {
        match declaration {
            ParameterDeclaration::Param(parameter) => {
                for assignment in parameter.nodes.2.nodes.0.contents() {
                    self.bind_generate_parameter_assignment(builder, assignment);
                }
            }
            ParameterDeclaration::Type(_) => self.unsupported(
                "type parameter inside a generate block",
                self.span_of(declaration),
            ),
        }
        let _ = is_local;
    }

    fn bind_generate_local_parameter(
        &mut self,
        builder: &mut ModuleBuilder,
        declaration: &LocalParameterDeclaration,
    ) {
        match declaration {
            LocalParameterDeclaration::Param(parameter) => {
                for assignment in parameter.nodes.2.nodes.0.contents() {
                    self.bind_generate_parameter_assignment(builder, assignment);
                }
            }
            LocalParameterDeclaration::Type(_) => self.unsupported(
                "type parameter inside a generate block",
                self.span_of(declaration),
            ),
        }
    }

    fn bind_generate_parameter_assignment(
        &mut self,
        builder: &mut ModuleBuilder,
        assignment: &sv_parser::ParamAssignment,
    ) {
        let name = self.text_of(&assignment.nodes.0).trim().to_string();
        if name.is_empty() {
            return;
        }
        let default = assignment.nodes.2.as_ref().map(
            |(_symbol, value): &(sv_parser::Symbol, ConstantParamExpression)| {
                self.text_of(value).trim().to_string()
            },
        );
        if let Some(default) = default {
            if let Some(value) = eval_const(&default, &builder.env) {
                builder.env.insert(name, value);
            }
        }
    }

    fn handle_net_declaration(&mut self, builder: &mut ModuleBuilder, net: &NetDeclaration) {
        match net {
            NetDeclaration::NetType(declaration) => {
                let info = self
                    .type_info_data_type_or_implicit(&declaration.nodes.3, Some(SignalKind::Wire));
                for assignment in declaration.nodes.5.nodes.0.contents() {
                    let name = self.text_of(&assignment.nodes.0).trim().to_string();
                    let span = self.span_of(&assignment.nodes.0);
                    let unpacked = self.unpacked_dims(&assignment.nodes.1);
                    let width = type_width(&info, &builder.env);
                    self.insert_signal(
                        builder,
                        info.kind.unwrap_or(SignalKind::Wire),
                        &info.packed,
                        unpacked,
                        width,
                        name,
                        span,
                    );
                    if let Some((_equals, expression)) = &assignment.nodes.2 {
                        self.add_loads(builder, expression, UsageKind::Declaration);
                    }
                }
            }
            NetDeclaration::NetTypeIdentifier(declaration) => {
                for assignment in declaration.nodes.2.nodes.0.contents() {
                    let name = self.text_of(&assignment.nodes.0).trim().to_string();
                    let span = self.span_of(&assignment.nodes.0);
                    let unpacked = self.unpacked_dims(&assignment.nodes.1);
                    self.insert_signal(builder, SignalKind::Other, &[], unpacked, None, name, span);
                    if let Some((_equals, expression)) = &assignment.nodes.2 {
                        self.add_loads(builder, expression, UsageKind::Declaration);
                    }
                }
            }
            NetDeclaration::Interconnect(_) => {
                self.unsupported("interconnect declaration", self.span_of(net))
            }
        }
    }

    fn handle_data_declaration(&mut self, builder: &mut ModuleBuilder, data: &DataDeclaration) {
        match data {
            DataDeclaration::Variable(declaration) => {
                let mut info = self
                    .type_info_data_type_or_implicit(&declaration.nodes.3, Some(SignalKind::Reg));
                if declaration.nodes.1.is_some() && info.kind.is_none() {
                    info.kind = Some(SignalKind::Reg);
                }
                for assignment in declaration.nodes.4.nodes.0.contents() {
                    match assignment {
                        sv_parser::VariableDeclAssignment::Variable(variable) => {
                            let name = self.text_of(&variable.nodes.0).trim().to_string();
                            let span = self.span_of(&variable.nodes.0);
                            let unpacked = self.variable_unpacked_dims(&variable.nodes.1);
                            let width = type_width(&info, &builder.env);
                            self.insert_signal(
                                builder,
                                info.kind.unwrap_or(SignalKind::Other),
                                &info.packed,
                                unpacked,
                                width,
                                name,
                                span,
                            );
                            if let Some((_equals, expression)) = &variable.nodes.2 {
                                self.add_loads(builder, expression, UsageKind::Declaration);
                            }
                        }
                        sv_parser::VariableDeclAssignment::DynamicArray(_) => {
                            self.unsupported("dynamic array declaration", self.span_of(assignment))
                        }
                        sv_parser::VariableDeclAssignment::Class(_) => {
                            self.unsupported("class variable declaration", self.span_of(assignment))
                        }
                    }
                }
            }
            DataDeclaration::TypeDeclaration(_) => {
                self.unsupported("type declaration", self.span_of(data))
            }
            DataDeclaration::PackageImportDeclaration(_) => {}
            DataDeclaration::NetTypeDeclaration(_) => {
                self.unsupported("nettype declaration", self.span_of(data))
            }
        }
    }

    //same as insert_port: the arguments are the declaration, not a bag
    #[allow(clippy::too_many_arguments)]
    fn insert_signal(
        &mut self,
        builder: &mut ModuleBuilder,
        kind: SignalKind,
        packed: &[RangeDim],
        unpacked: Vec<RangeDim>,
        width: Option<u32>,
        name: String,
        span: Span,
    ) {
        if name.is_empty() {
            return;
        }
        // A net declaration may repeat a port name (classic non-ANSI style);
        // merge the extra type information into the port instead of adding a
        // second signal.
        if let Some(port) = builder.ports.iter_mut().find(|port| port.name == name) {
            if port.kind == SignalKind::Other {
                port.kind = kind;
            }
            if port.width.is_none() {
                port.width = width;
            }
            return;
        }
        if let Some(port) = builder.port_map.get_mut(&name) {
            if port.kind == SignalKind::Other {
                port.kind = kind;
            }
            if port.width.is_none() {
                port.width = width;
            }
            return;
        }
        if builder.signal_names.contains(&name) {
            self.duplicate("signal", &name, span);
            return;
        }
        builder.signal_names.insert(name.clone());
        builder.signals.push(SignalDef {
            name,
            kind,
            packed: packed.to_vec(),
            unpacked,
            width,
            span,
            scope: self.generate_scope.clone(),
        });
    }

    // -- instances ---------------------------------------------------------

    fn handle_module_instantiation(
        &mut self,
        builder: &mut ModuleBuilder,
        instantiation: &ModuleInstantiation,
    ) {
        let module_name = self.text_of(&instantiation.nodes.0).trim().to_string();
        let module_span = self.span_of(&instantiation.nodes.0);
        let parameters = self.extract_parameter_assignments(instantiation.nodes.1.as_ref());
        for instance in instantiation.nodes.2.contents() {
            let name_of_instance = &instance.nodes.0;
            let name = self.text_of(&name_of_instance.nodes.0).trim().to_string();
            if name.is_empty() {
                continue;
            }
            let span = self.span_of(&name_of_instance.nodes.0);
            // Unpacked dimensions of an arrayed instance (u[3:0]) are kept so
            // the elaborator can expand one instance per element.
            let array = self.unpacked_dims(&name_of_instance.nodes.1);
            let connections = self.extract_port_connections(&instance.nodes.1.nodes.1);
            builder.instances.push(InstanceDecl {
                name,
                module: module_name.clone(),
                span,
                module_span: module_span.clone(),
                parameters: parameters.clone(),
                connections,
                scope: self.generate_scope.clone(),
                array,
            });
        }
    }

    fn extract_parameter_assignments(
        &mut self,
        assignment: Option<&sv_parser::ParameterValueAssignment>,
    ) -> Vec<ParamAssignment> {
        let Some(assignment) = assignment else {
            return Vec::new();
        };
        let Some(list) = &assignment.nodes.1.nodes.1 else {
            return Vec::new();
        };
        let mut result = Vec::new();
        match list {
            ListOfParameterAssignments::Ordered(ordered) => {
                for item in ordered.nodes.0.contents() {
                    let expression = &item.nodes.0;
                    result.push(ParamAssignment {
                        name: None,
                        value: Some(self.text_of(expression).trim().to_string()),
                        span: self.span_of(expression),
                    });
                }
            }
            ListOfParameterAssignments::Named(named) => {
                for item in named.nodes.0.contents() {
                    let name = self.text_of(&item.nodes.1).trim().to_string();
                    let value = item
                        .nodes
                        .2
                        .nodes
                        .1
                        .as_ref()
                        .map(|expression| self.text_of(expression).trim().to_string());
                    result.push(ParamAssignment {
                        name: Some(name),
                        value,
                        span: self.span_of(item),
                    });
                }
            }
        }
        result
    }

    fn extract_port_connections(
        &mut self,
        list: &Option<ListOfPortConnections>,
    ) -> Vec<PortConnectionDecl> {
        let Some(list) = list else {
            return Vec::new();
        };
        let mut result = Vec::new();
        match list {
            ListOfPortConnections::Ordered(ordered) => {
                for connection in ordered.nodes.0.contents() {
                    match &connection.nodes.1 {
                        Some(expression) => {
                            let names = self.collect_primary_names(expression);
                            result.push(PortConnectionDecl {
                                port: None,
                                expression: Some(self.text_of(expression).trim().to_string()),
                                span: self.span_of(expression),
                                names,
                            });
                        }
                        None => result.push(PortConnectionDecl {
                            port: None,
                            expression: None,
                            span: self.span_of(connection),
                            names: Vec::new(),
                        }),
                    }
                }
            }
            ListOfPortConnections::Named(named) => {
                for connection in named.nodes.0.contents() {
                    match connection {
                        NamedPortConnection::Identifier(connection) => {
                            let port = self.text_of(&connection.nodes.2).trim().to_string();
                            let port_span = self.span_of(&connection.nodes.2);
                            match &connection.nodes.3 {
                                Some(paren) => match &paren.nodes.1 {
                                    Some(expression) => {
                                        let names = self.collect_primary_names(expression);
                                        // The span of a named connection covers
                                        // `.port(expression)` (port name .. end of
                                        // the connected expression), not just the
                                        // expression: a cursor on the port name is
                                        // also part of the connection and has to
                                        // resolve to a driver/load answer.
                                        let expression_span = self.span_of(expression);
                                        let span = Span::new(
                                            port_span.file.clone(),
                                            port_span.start_line,
                                            port_span.start_col,
                                            expression_span.end_line,
                                            expression_span.end_col,
                                        );
                                        result.push(PortConnectionDecl {
                                            port: Some(port),
                                            expression: Some(
                                                self.text_of(expression).trim().to_string(),
                                            ),
                                            span,
                                            names,
                                        });
                                    }
                                    None => result.push(PortConnectionDecl {
                                        port: Some(port),
                                        expression: None,
                                        span: port_span,
                                        names: Vec::new(),
                                    }),
                                },
                                None => {
                                    // `.a` means "connect the signal with the same name".
                                    result.push(PortConnectionDecl {
                                        port: Some(port.clone()),
                                        expression: None,
                                        span: port_span.clone(),
                                        names: vec![NameRef {
                                            name: port,
                                            span: port_span,
                                            bits: None,
                                        }],
                                    });
                                }
                            }
                        }
                        NamedPortConnection::Asterisk(_) => {
                            result.push(PortConnectionDecl {
                                port: Some("*".to_string()),
                                expression: None,
                                span: self.span_of(connection),
                                names: Vec::new(),
                            });
                        }
                    }
                }
            }
        }
        result
    }

    // -- drivers and loads -------------------------------------------------

    fn handle_continuous_assign(&mut self, builder: &mut ModuleBuilder, assign: &ContinuousAssign) {
        match assign {
            ContinuousAssign::Net(assign) => {
                for assignment in assign.nodes.3.nodes.0.contents() {
                    let lvalues = self.collect_lvalues(&assignment.nodes.0);
                    self.push_usages(
                        builder,
                        lvalues,
                        UsageRole::Driver,
                        UsageKind::ContinuousAssign,
                    );
                    self.add_loads(builder, &assignment.nodes.2, UsageKind::ContinuousAssign);
                }
            }
            ContinuousAssign::Variable(assign) => {
                for assignment in assign.nodes.2.nodes.0.contents() {
                    let lvalues = self.collect_lvalues(&assignment.nodes.0);
                    self.push_usages(
                        builder,
                        lvalues,
                        UsageRole::Driver,
                        UsageKind::ContinuousAssign,
                    );
                    self.add_loads(builder, &assignment.nodes.2, UsageKind::ContinuousAssign);
                }
            }
        }
    }

    fn handle_procedural(&mut self, builder: &mut ModuleBuilder, always: &AlwaysConstruct) {
        self.collect_procedural_usages(builder, always)
    }

    fn handle_initial(&mut self, builder: &mut ModuleBuilder, initial: &InitialConstruct) {
        self.collect_procedural_usages(builder, initial)
    }

    fn handle_final(&mut self, builder: &mut ModuleBuilder, final_construct: &FinalConstruct) {
        self.collect_procedural_usages(builder, final_construct)
    }

    /// Collect drivers (left hand sides) and loads (every other identifier)
    /// from an `always`/`initial`/`final` construct.
    fn collect_procedural_usages<'n, T>(&mut self, builder: &mut ModuleBuilder, node: &'n T)
    where
        &'n T: Into<RefNode<'n>>,
    {
        let node: RefNode<'n> = node.into();
        // Event controls (`@(posedge clk)`) are reads, but deserve their own
        // reference kind so the UI can tell them apart from data loads.
        let mut event_spans: HashSet<Span> = HashSet::new();
        {
            let nodes = Iter::new(RefNodes(vec![node.clone()]));
            for item in nodes {
                if let RefNode::EventControl(event) = item {
                    let names = self.collect_primary_names(event);
                    for name in names {
                        event_spans.insert(name.span.clone());
                        builder.usages.push(SignalUsage {
                            name: name.name,
                            span: name.span,
                            role: UsageRole::Load,
                            kind: UsageKind::EventControl,
                            bits: name.bits,
                            scope: self.generate_scope.clone(),
                        });
                    }
                }
            }
        }
        let lvalues = self.collect_lvalues(node.clone());
        self.push_usages(
            builder,
            lvalues,
            UsageRole::Driver,
            UsageKind::ProceduralAssign,
        );
        let loads = self.collect_primary_names(node);
        for name in loads {
            if event_spans.contains(&name.span) {
                continue;
            }
            builder.usages.push(SignalUsage {
                name: name.name,
                span: name.span,
                role: UsageRole::Load,
                kind: UsageKind::ProceduralAssign,
                bits: name.bits,
                scope: self.generate_scope.clone(),
            });
        }
    }

    fn add_loads<'n, T>(&mut self, builder: &mut ModuleBuilder, node: &'n T, kind: UsageKind)
    where
        &'n T: Into<RefNode<'n>>,
    {
        let names = self.collect_primary_names(node);
        self.push_usages(builder, names, UsageRole::Load, kind);
    }

    fn push_usages(
        &mut self,
        builder: &mut ModuleBuilder,
        names: Vec<NameRef>,
        role: UsageRole,
        kind: UsageKind,
    ) {
        for name in names {
            builder.usages.push(SignalUsage {
                name: name.name,
                span: name.span,
                role,
                kind,
                bits: name.bits,
                scope: self.generate_scope.clone(),
            });
        }
    }

    /// Text of a select node as written (`"[3:0]"`, `"[i]"`), or `None` when the
    /// identifier is not bit-selected (`q`, `q[3]` -> "[3]").
    fn select_text<'n>(&self, node: impl Into<RefNode<'n>>) -> Option<String> {
        let text = self.text_of(node).trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    fn collect_primary_names<'n>(&mut self, node: impl Into<RefNode<'n>>) -> Vec<NameRef> {
        let mut names = Vec::new();
        let nodes = RefNodes(vec![node.into()]);
        for item in Iter::new(nodes) {
            if let RefNode::PrimaryHierarchical(primary) = item {
                let identifier = &primary.nodes.1;
                let name = self.hierarchical_name(identifier);
                if !name.is_empty() {
                    names.push(NameRef {
                        name,
                        span: self.span_of(identifier),
                        bits: self.select_text(&primary.nodes.2),
                    });
                }
            }
        }
        names
    }

    fn collect_lvalues<'n>(&mut self, node: impl Into<RefNode<'n>>) -> Vec<NameRef> {
        let mut names = Vec::new();
        let nodes = RefNodes(vec![node.into()]);
        for item in Iter::new(nodes) {
            match item {
                RefNode::NetLvalueIdentifier(lvalue) => {
                    if let Some(name) = self.net_lvalue_name(&lvalue.nodes.0) {
                        names.push(NameRef {
                            name,
                            span: self.span_of(lvalue),
                            bits: self.select_text(&lvalue.nodes.1),
                        });
                    }
                }
                RefNode::VariableLvalueIdentifier(lvalue) => {
                    let identifier = &lvalue.nodes.1.nodes.0;
                    let name = self.hierarchical_name(identifier);
                    if !name.is_empty() {
                        let bits = self.select_text(&lvalue.nodes.2);
                        names.push(NameRef {
                            name,
                            span: self.span_of(identifier),
                            bits,
                        });
                    }
                }
                RefNode::NonrangeVariableLvalue(lvalue) => {
                    let identifier = &lvalue.nodes.1.nodes.0;
                    let name = self.hierarchical_name(identifier);
                    if !name.is_empty() {
                        let bits = self.select_text(&lvalue.nodes.2);
                        names.push(NameRef {
                            name,
                            span: self.span_of(identifier),
                            bits,
                        });
                    }
                }
                RefNode::BlockingAssignmentHierarchicalVariable(lvalue) => {
                    let identifier = &lvalue.nodes.1.nodes.0;
                    let name = self.hierarchical_name(identifier);
                    if !name.is_empty() {
                        let bits = self.select_text(&lvalue.nodes.2);
                        names.push(NameRef {
                            name,
                            span: self.span_of(identifier),
                            bits,
                        });
                    }
                }
                _ => {}
            }
        }
        names
    }

    fn net_lvalue_name(&self, lvalue: &PsOrHierarchicalNetIdentifier) -> Option<String> {
        match lvalue {
            PsOrHierarchicalNetIdentifier::PackageScope(scoped) => {
                Some(self.identifier_text(&scoped.nodes.1.nodes.0))
            }
            PsOrHierarchicalNetIdentifier::HierarchicalNetIdentifier(hierarchical) => {
                let name = self.hierarchical_name(&hierarchical.nodes.0);
                if name.is_empty() {
                    None
                } else {
                    Some(name)
                }
            }
        }
    }

    // -- type information --------------------------------------------------

    fn packed_dims(&mut self, dimensions: &[PackedDimension], owner: Span) -> Vec<RangeDim> {
        let mut result = Vec::new();
        for dimension in dimensions {
            match dimension {
                PackedDimension::Range(range) => {
                    let constant_range = &range.nodes.0.nodes.1;
                    result.push(RangeDim::new(
                        self.text_of(&constant_range.nodes.0).trim().to_string(),
                        self.text_of(&constant_range.nodes.2).trim().to_string(),
                    ));
                }
                PackedDimension::UnsizedDimension(_) => {
                    self.unsupported("unsized packed dimension", owner.clone())
                }
            }
        }
        result
    }

    fn unpacked_dims(&mut self, dimensions: &[UnpackedDimension]) -> Vec<RangeDim> {
        let mut result = Vec::new();
        for dimension in dimensions {
            match dimension {
                UnpackedDimension::Range(range) => {
                    let constant_range = &range.nodes.0.nodes.1;
                    result.push(RangeDim::new(
                        self.text_of(&constant_range.nodes.0).trim().to_string(),
                        self.text_of(&constant_range.nodes.2).trim().to_string(),
                    ));
                }
                UnpackedDimension::Expression(_) => {
                    self.unsupported("sized unpacked dimension", self.span_of(dimension))
                }
            }
        }
        result
    }

    fn variable_unpacked_dims(&mut self, dimensions: &[VariableDimension]) -> Vec<RangeDim> {
        let mut unpacked = Vec::new();
        for dimension in dimensions {
            match dimension {
                VariableDimension::UnpackedDimension(dimension) => {
                    let mut collected = self.unpacked_dims(std::slice::from_ref(dimension));
                    unpacked.append(&mut collected);
                }
                VariableDimension::UnsizedDimension(_)
                | VariableDimension::AssociativeDimension(_)
                | VariableDimension::QueueDimension(_) => {
                    self.unsupported("dynamic array dimension", self.span_of(dimension))
                }
            }
        }
        unpacked
    }

    fn type_info_data_type(&mut self, data_type: &DataType) -> TypeInfo {
        let span = self.span_of(data_type);
        match data_type {
            DataType::Vector(vector) => {
                let kind = match &vector.nodes.0 {
                    IntegerVectorType::Bit(_) => SignalKind::Bit,
                    IntegerVectorType::Logic(_) => SignalKind::Logic,
                    IntegerVectorType::Reg(_) => SignalKind::Reg,
                };
                let packed = self.packed_dims(&vector.nodes.2, span);
                TypeInfo::with_kind(kind, packed)
            }
            DataType::Atom(atom) => {
                let width = match &atom.nodes.0 {
                    IntegerAtomType::Byte(_) => 8,
                    IntegerAtomType::Shortint(_) => 16,
                    IntegerAtomType::Int(_) => 32,
                    IntegerAtomType::Longint(_) => 64,
                    IntegerAtomType::Integer(_) => 32,
                    IntegerAtomType::Time(_) => 64,
                };
                TypeInfo {
                    kind: Some(SignalKind::Integer),
                    packed: Vec::new(),
                    fixed_width: Some(width),
                }
            }
            DataType::NonIntegerType(_) => TypeInfo {
                kind: Some(SignalKind::Real),
                packed: Vec::new(),
                fixed_width: None,
            },
            DataType::StructUnion(_) => {
                self.unsupported("struct/union data type", span);
                TypeInfo::unknown()
            }
            DataType::Enum(_) => {
                self.unsupported("enum data type", span);
                TypeInfo::unknown()
            }
            DataType::Type(_) | DataType::TypeReference(_) => {
                self.unsupported("user defined data type", span);
                TypeInfo::unknown()
            }
            DataType::String(_)
            | DataType::Chandle(_)
            | DataType::Virtual(_)
            | DataType::ClassType(_)
            | DataType::Event(_)
            | DataType::PsCovergroupIdentifier(_) => {
                self.unsupported("non synthesizable data type", span);
                TypeInfo {
                    kind: Some(SignalKind::Other),
                    packed: Vec::new(),
                    fixed_width: None,
                }
            }
        }
    }

    fn type_info_data_type_or_implicit(
        &mut self,
        data_type: &DataTypeOrImplicit,
        default_kind: Option<SignalKind>,
    ) -> TypeInfo {
        match data_type {
            DataTypeOrImplicit::DataType(data_type) => {
                let mut info = self.type_info_data_type(data_type);
                if info.kind.is_none() {
                    info.kind = default_kind;
                }
                info
            }
            DataTypeOrImplicit::ImplicitDataType(implicit) => {
                let packed = self.packed_dims(&implicit.nodes.1, self.span_of(data_type));
                TypeInfo::with_kind(default_kind.unwrap_or(SignalKind::Wire), packed)
            }
        }
    }

    fn type_info_net_port_type(&mut self, net_type: &NetPortType) -> TypeInfo {
        match net_type {
            NetPortType::DataType(data_type) => {
                self.type_info_data_type_or_implicit(&data_type.nodes.1, Some(SignalKind::Wire))
            }
            NetPortType::NetTypeIdentifier(_) => TypeInfo::unknown(),
            NetPortType::Interconnect(_) => {
                self.unsupported("interconnect port type", self.span_of(net_type));
                TypeInfo::unknown()
            }
        }
    }

    fn type_info_variable_port_type(&mut self, variable_type: &VariablePortType) -> TypeInfo {
        match &variable_type.nodes.0 {
            VarDataType::DataType(data_type) => self.type_info_data_type(data_type),
            VarDataType::Var(var) => {
                self.type_info_data_type_or_implicit(&var.nodes.1, Some(SignalKind::Reg))
            }
        }
    }
}

/// Type information shared by ports and declarations.
#[derive(Clone, Debug)]
struct TypeInfo {
    kind: Option<SignalKind>,
    packed: Vec<RangeDim>,
    fixed_width: Option<u32>,
}

impl TypeInfo {
    fn scalar() -> Self {
        TypeInfo {
            kind: None,
            packed: Vec::new(),
            fixed_width: Some(1),
        }
    }

    fn with_kind(kind: SignalKind, packed: Vec<RangeDim>) -> Self {
        let fixed_width = if packed.is_empty() { Some(1) } else { None };
        TypeInfo {
            kind: Some(kind),
            packed,
            fixed_width,
        }
    }

    fn unknown() -> Self {
        TypeInfo {
            kind: None,
            packed: Vec::new(),
            fixed_width: None,
        }
    }
}

#[derive(Clone)]
struct AnsiPortState {
    direction: Direction,
    info: TypeInfo,
}

enum PortNameList<'a> {
    Port(&'a sv_parser::ListOfPortIdentifiers),
    Variable(&'a sv_parser::ListOfVariableIdentifiers),
    VariablePort(&'a sv_parser::ListOfVariablePortIdentifiers),
}

fn type_width(info: &TypeInfo, env: &ParamEnv) -> Option<u32> {
    if info.packed.is_empty() {
        return info.fixed_width;
    }
    let mut width: u32 = 1;
    for dimension in &info.packed {
        width = width.checked_mul(dim_width(&dimension.msb, &dimension.lsb, env)?)?;
    }
    Some(width)
}
