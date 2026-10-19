//! Public data model of the design database.
//!
//! Everything a consumer (the waveform UI, the language server) needs is
//! expressed with these types; the underlying SystemVerilog parser is an
//! implementation detail of [`crate::design::frontend`] and never appears in
//! a public signature.  That is deliberate: the parser can be swapped (or the
//! version bumped) without touching the query API.

use std::fmt;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Source positions
// ---------------------------------------------------------------------------

/// A half-open source range in one file.
///
/// `start_line` / `start_col` are 1-based, `end_line` / `end_col` point just
/// past the last character of the construct (also 1-based, exclusive).
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Span {
    pub file: PathBuf,
    pub start_line: u32,
    pub start_col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

impl Span {
    pub fn new(
        file: impl Into<PathBuf>,
        start_line: u32,
        start_col: u32,
        end_line: u32,
        end_col: u32,
    ) -> Self {
        Span {
            file: file.into(),
            start_line,
            start_col,
            end_line,
            end_col,
        }
    }

    /// Point span used as a last resort when no source origin is available.
    pub fn point(file: impl Into<PathBuf>, line: u32, col: u32) -> Self {
        Span {
            file: file.into(),
            start_line: line,
            start_col: col,
            end_line: line,
            end_col: col,
        }
    }

    pub fn file(&self) -> &Path {
        &self.file
    }

    /// True when `(line, col)` lies inside the span.  Both ends are inclusive
    /// for the purpose of the query, which is what a "what is under the
    /// cursor" query wants.
    pub fn contains(&self, line: u32, col: u32) -> bool {
        if line < self.start_line || line > self.end_line {
            return false;
        }
        if line == self.start_line && col < self.start_col {
            return false;
        }
        if line == self.end_line && col > self.end_col {
            return false;
        }
        true
    }

    /// Ordering key used to pick the innermost construct when several spans
    /// contain the same source position.
    pub fn size_key(&self) -> u64 {
        let lines = self.end_line.saturating_sub(self.start_line) as u64;
        let cols = self.end_col.saturating_sub(self.start_col) as u64;
        lines.saturating_mul(1_000_000).saturating_add(cols)
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.file.display(),
            self.start_line,
            self.start_col
        )
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

/// Direction of a module port.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    Input,
    Output,
    Inout,
    Ref,
    Unknown,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Input => "input",
            Direction::Output => "output",
            Direction::Inout => "inout",
            Direction::Ref => "ref",
            Direction::Unknown => "unknown",
        }
    }

    pub fn is_output_like(self) -> bool {
        matches!(self, Direction::Output | Direction::Inout)
    }
}

/// Coarse declaration kind of a signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SignalKind {
    Wire,
    Reg,
    Logic,
    Bit,
    Integer,
    Real,
    Other,
}

impl SignalKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SignalKind::Wire => "wire",
            SignalKind::Reg => "reg",
            SignalKind::Logic => "logic",
            SignalKind::Bit => "bit",
            SignalKind::Integer => "integer",
            SignalKind::Real => "real",
            SignalKind::Other => "other",
        }
    }
}

/// One packed or unpacked range, kept as source text so that instance specific
/// parameter overrides can be applied during elaboration.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct RangeDim {
    pub msb: String,
    pub lsb: String,
}

impl RangeDim {
    pub fn new(msb: impl Into<String>, lsb: impl Into<String>) -> Self {
        RangeDim {
            msb: msb.into(),
            lsb: lsb.into(),
        }
    }

    pub fn text(&self) -> String {
        format!("[{}:{}]", self.msb, self.lsb)
    }
}

impl fmt::Display for RangeDim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}:{}]", self.msb, self.lsb)
    }
}

/// A module port.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PortDef {
    pub name: String,
    pub direction: Direction,
    pub kind: SignalKind,
    /// Packed dimensions, outermost first.  Empty for a scalar.
    pub packed: Vec<RangeDim>,
    /// Unpacked (array) dimensions, outermost first.
    pub unpacked: Vec<RangeDim>,
    /// Width with the module's default parameter values applied.  `None` when
    /// the range uses a construct this front end cannot evaluate.
    pub width: Option<u32>,
    /// Span of the port name in the declaration that carries its direction.
    pub span: Span,
    /// For non-ANSI modules: span of the port name in the module header, where
    /// the port is listed but not yet declared with a direction.
    pub header_span: Option<Span>,
}

/// A net or variable declared inside a module body.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SignalDef {
    pub name: String,
    pub kind: SignalKind,
    pub packed: Vec<RangeDim>,
    pub unpacked: Vec<RangeDim>,
    /// Width with the module's default parameter values applied.
    pub width: Option<u32>,
    pub span: Span,
}

/// A `parameter` or `localparam` declaration.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamDef {
    pub name: String,
    pub default: Option<String>,
    pub is_local: bool,
    pub span: Span,
}

/// A parameter assignment on an instance (`#(.W(8))` or `#(8)`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamAssignment {
    /// `None` for positional assignments.
    pub name: Option<String>,
    /// Value expression source text; `None` for `.W()`.
    pub value: Option<String>,
    pub span: Span,
}

/// A single name reference inside an expression, with its own span.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct NameRef {
    /// Hierarchical name as written; elements of a select are stripped, e.g.
    /// `u.pc[3]` yields `u.pc`.
    pub name: String,
    pub span: Span,
}

/// A port connection in an instance declaration.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PortConnectionDecl {
    /// `None` for a positional connection, `Some("*")` for `.*`.
    pub port: Option<String>,
    /// Connection expression source text.  `None` for `.a`, `.a()` and empty
    /// positional slots.
    pub expression: Option<String>,
    /// Span of the connection expression, or of the port name for implicit
    /// connections.
    pub span: Span,
    /// Signal names referenced by the connection expression.
    pub names: Vec<NameRef>,
}

/// An instantiation (`sub u_sub (...);`) as written in a module body.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InstanceDecl {
    pub name: String,
    pub module: String,
    /// Span of the instance name.
    pub span: Span,
    /// Span of the instantiated module identifier.
    pub module_span: Span,
    pub parameters: Vec<ParamAssignment>,
    pub connections: Vec<PortConnectionDecl>,
}

/// Role of a signal reference collected from the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum UsageRole {
    Driver,
    Load,
}

/// What kind of construct produced a driver/load reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum UsageKind {
    ContinuousAssign,
    ProceduralAssign,
    PortConnection,
    EventControl,
    Declaration,
    Unknown,
}

/// A reference to a module local signal, collected while parsing.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SignalUsage {
    /// Local name as written; may be hierarchical (`u.pc`).
    pub name: String,
    pub span: Span,
    pub role: UsageRole,
    pub kind: UsageKind,
}

/// One parsed module definition.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModuleDef {
    pub name: String,
    /// Span of the module name in its declaration.
    pub span: Span,
    pub parameters: Vec<ParamDef>,
    pub ports: Vec<PortDef>,
    /// Nets and variables declared in the body.  Ports are not repeated here.
    pub signals: Vec<SignalDef>,
    pub instances: Vec<InstanceDecl>,
    /// Driver/load references written inside the module body.  They are
    /// resolved to hierarchical paths per elaborated instance.
    pub usages: Vec<SignalUsage>,
}

impl ModuleDef {
    pub fn port(&self, name: &str) -> Option<&PortDef> {
        self.ports.iter().find(|p| p.name == name)
    }

    pub fn signal(&self, name: &str) -> Option<&SignalDef> {
        self.signals.iter().find(|s| s.name == name)
    }

    pub fn parameter(&self, name: &str) -> Option<&ParamDef> {
        self.parameters.iter().find(|p| p.name == name)
    }
}

// ---------------------------------------------------------------------------
// Elaboration
// ---------------------------------------------------------------------------

/// One parameter of an elaborated instance, with its final value when it can
/// be evaluated to an integer.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParamValue {
    pub name: String,
    /// Source text of the value (override or default).
    pub text: Option<String>,
    pub value: Option<i64>,
    pub is_local: bool,
}

/// An elaborated port binding of one instance.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PortBinding {
    pub port: String,
    pub direction: Direction,
    pub width: Option<u32>,
    /// `None` when the port is unconnected.
    pub connection: Option<Connection>,
}

/// A connection expression on an elaborated instance, with its names resolved
/// to hierarchical paths in the *parent* scope.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    pub text: String,
    pub span: Span,
    /// Raw references as written in the parent scope.
    pub names: Vec<NameRef>,
    /// Resolved parent signal paths.
    pub signals: Vec<String>,
}

/// One node of the elaborated instance tree.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InstanceNode {
    /// Hierarchical instance path, e.g. `top.u_cpu`.
    pub path: String,
    /// Leaf instance name.
    pub name: String,
    /// Module definition name.
    pub module: String,
    /// Index into [`ElaboratedDesign::instances`], `None` for the top.
    pub parent: Option<usize>,
    /// Indices into [`ElaboratedDesign::instances`].
    pub children: Vec<usize>,
    pub parameters: Vec<ParamValue>,
    pub ports: Vec<PortBinding>,
    /// Span of the instance name in the parent module.
    pub span: Span,
    /// Span of the instantiated module's declaration name.
    pub module_span: Span,
    /// Index of the instantiation statement in the parent module's
    /// [`ModuleDef::instances`]; `None` for the top instance.
    pub decl_index: Option<usize>,
}

impl InstanceNode {
    pub fn parameter(&self, name: &str) -> Option<&ParamValue> {
        self.parameters.iter().find(|p| p.name == name)
    }

    pub fn port(&self, name: &str) -> Option<&PortBinding> {
        self.ports.iter().find(|p| p.port == name)
    }
}

/// A resolved driver or load reference.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SignalReference {
    pub span: Span,
    pub kind: UsageKind,
}

/// A flat signal table entry, keyed by hierarchical path.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FlatSignal {
    pub path: String,
    /// Signal name inside its module.
    pub local_name: String,
    /// Instance that owns the signal (hierarchical instance path).
    pub instance: String,
    pub module: String,
    /// `Some` for ports.
    pub direction: Option<Direction>,
    pub kind: SignalKind,
    pub width: Option<u32>,
    /// Declaration span, i.e. where this signal is declared.
    pub declaration: Span,
    pub drivers: Vec<SignalReference>,
    pub loads: Vec<SignalReference>,
}

/// Result of elaborating one top module.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ElaboratedDesign {
    pub(crate) top: String,
    pub(crate) root: usize,
    pub(crate) instances: Vec<InstanceNode>,
    pub(crate) signals: Vec<FlatSignal>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// Lookup acceleration rebuilt by the elaborator.  Skipped when the model
    /// is transported over the wasm/JSON boundary; [`ElaboratedDesign::signal`]
    /// falls back to a scan when it is empty.
    #[serde(skip)]
    pub(crate) signal_index: std::collections::HashMap<String, usize>,
}

/// Two elaborated designs are equal when their transported data is equal; the
/// derived lookup cache is deliberately ignored (it is rebuilt lazily).
impl PartialEq for ElaboratedDesign {
    fn eq(&self, other: &Self) -> bool {
        self.top == other.top
            && self.root == other.root
            && self.instances == other.instances
            && self.signals == other.signals
            && self.diagnostics == other.diagnostics
    }
}

impl ElaboratedDesign {
    /// Name of the top module.
    pub fn top(&self) -> &str {
        &self.top
    }

    /// Index of the root instance (always `0` when elaboration succeeded).
    pub fn root(&self) -> usize {
        self.root
    }

    /// The instance tree, in depth-first creation order.  Index 0 is the top.
    pub fn instances(&self) -> &[InstanceNode] {
        &self.instances
    }

    /// All elaborated signals, in depth-first creation order.
    pub fn signals(&self) -> &[FlatSignal] {
        &self.signals
    }

    /// Look up an instance by hierarchical path (`top.u_cpu`).
    pub fn instance(&self, path: &str) -> Option<&InstanceNode> {
        self.instances.iter().find(|i| i.path == path)
    }

    /// Look up a signal by hierarchical path (`top.u_cpu.pc`).
    pub fn signal(&self, path: &str) -> Option<&FlatSignal> {
        match self.signal_index.get(path) {
            Some(index) => self.signals.get(*index),
            // The index is not serialized; a deserialized design falls back
            // to a scan over the flat table.
            None => self.signals.iter().find(|signal| signal.path == path),
        }
    }

    /// Where the signal is declared.
    pub fn declaration(&self, path: &str) -> Option<&Span> {
        self.signal(path).map(|s| &s.declaration)
    }

    /// Spans that drive the signal.
    pub fn drivers(&self, path: &str) -> &[SignalReference] {
        self.signal(path)
            .map(|s| s.drivers.as_slice())
            .unwrap_or(&[])
    }

    /// Spans that read the signal.
    pub fn loads(&self, path: &str) -> &[SignalReference] {
        self.signal(path).map(|s| s.loads.as_slice()).unwrap_or(&[])
    }

    /// Diagnostics produced while elaborating (unresolved modules, unknown
    /// ports, recursion limits, ...).
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

// ---------------------------------------------------------------------------
// Source queries
// ---------------------------------------------------------------------------

/// Answer to "what is at this source position".
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SourceItem {
    Module {
        name: String,
        span: Span,
    },
    Port {
        module: String,
        name: String,
        span: Span,
    },
    Signal {
        module: String,
        name: String,
        span: Span,
    },
    Instance {
        path: String,
        module: String,
        span: Span,
    },
    /// A place where a signal is driven.
    Driver {
        signal: String,
        span: Span,
        kind: UsageKind,
    },
    /// A place where a signal is read.
    Load {
        signal: String,
        span: Span,
        kind: UsageKind,
    },
}

impl SourceItem {
    pub fn span(&self) -> &Span {
        match self {
            SourceItem::Module { span, .. }
            | SourceItem::Port { span, .. }
            | SourceItem::Signal { span, .. }
            | SourceItem::Instance { span, .. }
            | SourceItem::Driver { span, .. }
            | SourceItem::Load { span, .. } => span,
        }
    }
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DiagnosticKind {
    Io,
    Preprocess,
    Parse,
    ParserPanic,
    UnsupportedConstruct,
    UnresolvedModule,
    DuplicateDeclaration,
    MissingPortDeclaration,
    UnresolvedWidth,
    UnknownPort,
    RecursionLimit,
    Other,
}

impl DiagnosticKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DiagnosticKind::Io => "io",
            DiagnosticKind::Preprocess => "preprocess",
            DiagnosticKind::Parse => "parse",
            DiagnosticKind::ParserPanic => "parser-panic",
            DiagnosticKind::UnsupportedConstruct => "unsupported-construct",
            DiagnosticKind::UnresolvedModule => "unresolved-module",
            DiagnosticKind::DuplicateDeclaration => "duplicate-declaration",
            DiagnosticKind::MissingPortDeclaration => "missing-port-declaration",
            DiagnosticKind::UnresolvedWidth => "unresolved-width",
            DiagnosticKind::UnknownPort => "unknown-port",
            DiagnosticKind::RecursionLimit => "recursion-limit",
            DiagnosticKind::Other => "other",
        }
    }
}

/// A non fatal problem found while reading or elaborating the design.
///
/// Malformed input never panics: everything that can go wrong becomes one of
/// these instead.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub kind: DiagnosticKind,
    pub message: String,
    pub span: Option<Span>,
}

impl Diagnostic {
    pub fn new(
        severity: Severity,
        kind: DiagnosticKind,
        message: impl Into<String>,
        span: Option<Span>,
    ) -> Self {
        Diagnostic {
            severity,
            kind,
            message: message.into(),
            span,
        }
    }

    pub fn error(kind: DiagnosticKind, message: impl Into<String>, span: Option<Span>) -> Self {
        Diagnostic::new(Severity::Error, kind, message, span)
    }

    pub fn warning(kind: DiagnosticKind, message: impl Into<String>, span: Option<Span>) -> Self {
        Diagnostic::new(Severity::Warning, kind, message, span)
    }

    pub fn info(kind: DiagnosticKind, message: impl Into<String>, span: Option<Span>) -> Self {
        Diagnostic::new(Severity::Info, kind, message, span)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let severity = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        };
        match &self.span {
            Some(span) => write!(
                f,
                "{}[{}] at {}: {}",
                severity,
                self.kind.as_str(),
                span,
                self.message
            ),
            None => write!(f, "{}[{}]: {}", severity, self.kind.as_str(), self.message),
        }
    }
}
