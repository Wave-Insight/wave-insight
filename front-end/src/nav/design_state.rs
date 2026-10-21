//! The front end side of the design database.
//!
//! The desktop build receives a serialized [`DesignDb`] from the core, the wasm
//! build parses the files the user dropped. Either way this module owns the
//! database plus the elaborated design and turns the low level queries into the
//! few owned, render ready answers the Yew components need.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use wave_insight_lib::design::{DesignDb, ElaboratedDesign, FlatSignal, Span, UsageKind};

use super::tree::{ModuleRow, SidebarTree, SignalRow};

/// One clickable source position shown in the inspector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceRow {
    /// `file:line:col`, i.e. what [`Span`]'s `Display` produces.
    pub label: String,
    /// What kind of reference this is (`assign`, `port`, ...).
    pub detail: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

impl SourceRow {
    pub fn of(span: &Span, detail: impl Into<String>) -> Self {
        Self {
            label: span.to_string(),
            detail: detail.into(),
            file: span.file.display().to_string(),
            line: span.start_line,
            col: span.start_col,
        }
    }

    /// [`crate::nav::Goto`] target, i.e. `(file, line, col)`.
    pub fn target(&self) -> (String, u32, u32) {
        (self.file.clone(), self.line, self.col)
    }
}

/// Everything the inspector shows for one signal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalDetails {
    pub path: String,
    pub width: Option<u32>,
    /// `input`, `output`, ... when the signal is a port.
    pub direction: Option<String>,
    pub kind: String,
    pub declaration: Option<SourceRow>,
    pub drivers: Vec<SourceRow>,
    pub loads: Vec<SourceRow>,
}

/// Parsed sources plus the design elaborated from them.
pub struct DesignState {
    db: DesignDb,
    design: Option<ElaboratedDesign>,
    /// The module that was elaborated, if any.
    top: Option<String>,
    /// The waveform hierarchy names its root scope (usually the testbench); when
    /// the same module was loaded from source it is the best guess for "top".
    top_hint: Option<String>,
    /// Bumped on every rebuild; the components use it to notice new content
    /// behind an unchanged [`crate::nav::SharedDesign`] handle.
    version: u64,
}

impl DesignState {
    pub fn new() -> Self {
        Self {
            db: DesignDb::new(),
            design: None,
            top: None,
            top_hint: None,
            version: 0,
        }
    }

    /// Parse one source file (in memory) and re-elaborate.
    ///
    /// The file name is kept verbatim: every span the queries return refers to
    /// it, so the editor tabs must use the same spelling.
    pub fn add_source(&mut self, file: &str, text: &str) {
        // `add_source` returns `false` only for a hard parse failure; the
        // modules it managed to recover are still worth elaborating.
        self.db.add_source(file, text);
        self.rebuild();
    }

    /// Tell the database which module the waveform was dumped from.
    pub fn set_top_hint(&mut self, hint: Option<String>) {
        if self.top_hint != hint {
            self.top_hint = hint;
            if !self.db.modules().is_empty() {
                self.rebuild();
            }
        }
    }

    /// Recompute the elaborated design from the current database.
    pub fn rebuild(&mut self) {
        self.top = self.pick_top();
        self.design = self
            .top
            .as_ref()
            .map(|top| self.db.elaborate(top.as_str()));
        self.version = self.version.wrapping_add(1);
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// The elaborated top module name.
    pub fn top(&self) -> Option<&str> {
        self.top.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.db.modules().is_empty()
    }

    /// Number of problems the parser/elaborator reported.
    pub fn diagnostic_count(&self) -> usize {
        self.db.diagnostics().len() + self.design.as_ref().map(|d| d.diagnostics().len()).unwrap_or(0)
    }

    /// The flat signal table entry for a hierarchical path.
    ///
    /// Accepts the exact elaborated path as well as a path from the waveform
    /// hierarchy that spells the root scope differently; see [`Self::resolve_path`].
    pub fn flat_signal(&self, path: &str) -> Option<&FlatSignal> {
        self.resolve_path(path)
    }

    /// Resolve a signal path against the elaborated design.
    ///
    /// An exact hierarchical path always wins. The dump and the sources can spell
    /// the root scope differently (`tb_top.u_cpu.pc` in the VCD vs
    /// `top.u_cpu.pc` in the sources); the ordered rules are documented on
    /// [`super::resolve_candidates`]. `None` when the answer would be ambiguous:
    /// guessing between two instances would navigate to the wrong declaration.
    pub fn resolve_path(&self, path: &str) -> Option<&FlatSignal> {
        let mut candidates = self.resolve_candidates(path).into_iter();
        let first = candidates.next()?;
        if candidates.next().is_some() {
            return None;
        }
        Some(first)
    }

    /// Every signal a path can mean, at the most specific interpretation that
    /// matches at all (exact path first, then the root-replacement/suffix rules of
    /// [`super::resolve_candidates`]).
    ///
    /// Callers that navigate must not pick one when this returns several; the UI
    /// says how many candidates matched instead of guessing.
    pub fn resolve_candidates(&self, path: &str) -> Vec<&FlatSignal> {
        let Some(design) = self.design.as_ref() else {
            return Vec::new();
        };
        super::resolve_candidates(
            path,
            design.signals().iter(),
            |signal| signal.path.as_str(),
            |signal| signal.local_name.as_str(),
        )
    }

    /// Where a hierarchical path is declared.
    pub fn declaration(&self, path: &str) -> Option<&Span> {
        self.resolve_path(path).map(|signal| &signal.declaration)
    }

    /// Anything the inspector needs for one signal.
    pub fn signal_details(&self, path: &str) -> Option<SignalDetails> {
        let signal = self.resolve_path(path)?;
        Some(SignalDetails {
            path: signal.path.clone(),
            width: signal.width,
            direction: signal.direction.map(|d| d.as_str().to_string()),
            kind: signal.kind.as_str().to_string(),
            declaration: Some(SourceRow::of(&signal.declaration, "declaration")),
            drivers: signal
                .drivers
                .iter()
                .map(|driver| SourceRow::of(&driver.span, usage_label(driver.kind)))
                .collect(),
            loads: signal
                .loads
                .iter()
                .map(|load| SourceRow::of(&load.span, usage_label(load.kind)))
                .collect(),
        })
    }

    /// Every hierarchical signal whose declaration or usage covers `file:line:col`.
    pub fn signals_at(&self, file: &str, line: u32, col: u32) -> Vec<String> {
        match &self.design {
            Some(design) => self.db.signals_at(design, Path::new(file), line, col),
            None => Vec::new(),
        }
    }

    /// Fallback lookup: every elaborated signal whose local name is `word`.
    ///
    /// Used when the clicked position is not covered by any span (a comment, a
    /// module that is not part of the elaborated hierarchy, ...), so clicking a
    /// name in the source still does something useful.
    pub fn signals_matching(&self, word: &str) -> Vec<String> {
        let Some(design) = &self.design else {
            return Vec::new();
        };
        if word.is_empty() {
            return Vec::new();
        }
        design
            .signals()
            .iter()
            .filter(|signal| signal.local_name == word)
            .map(|signal| signal.path.clone())
            .collect()
    }

    /// Build the sidebar tree of the elaborated design.
    ///
    /// The instance hierarchy becomes the module tree and every elaborated
    /// signal becomes a row, so the same list works before a dump is loaded.
    /// Paths are the row keys, so a row can be looked up in the dump later.
    pub fn sidebar_tree(&self) -> Option<SidebarTree> {
        let design = self.design.as_ref()?;
        if design.instances().is_empty() {
            return None;
        }

        let mut by_instance: HashMap<&str, Vec<&FlatSignal>> = HashMap::new();
        for signal in design.signals() {
            by_instance
                .entry(signal.instance.as_str())
                .or_default()
                .push(signal);
        }

        Some(SidebarTree::new(build_row(design, &by_instance, design.root())))
    }

    /// Which module to elaborate.
    ///
    /// Preference: the waveform's top scope, then a module literally called
    /// `top`, then the only module nobody instantiates. Picking "the biggest
    /// root" is the last resort; a design without any root (a cycle) falls back
    /// to the first module so evidence still shows up in the UI.
    fn pick_top(&self) -> Option<String> {
        if let Some(hint) = &self.top_hint {
            if self.db.module(hint).is_some() {
                return Some(hint.clone());
            }
        }
        let modules = self.db.modules();
        if modules.is_empty() {
            return None;
        }

        let instantiated: BTreeSet<&str> = modules
            .iter()
            .flat_map(|module| module.instances.iter().map(|inst| inst.module.as_str()))
            .collect();
        let mut roots: Vec<&str> = modules
            .iter()
            .filter(|module| !instantiated.contains(module.name.as_str()))
            .map(|module| module.name.as_str())
            .collect();
        if roots.is_empty() {
            return Some(modules[0].name.clone());
        }
        if let Some(top) = roots.iter().find(|name| **name == "top") {
            return Some((*top).to_string());
        }
        // no hint: the root with the most declarations is nearly always the top
        roots.sort_by_key(|name| {
            let module = self.db.module(name);
            let size = module
                .map(|module| module.signals.len() + module.ports.len() + module.instances.len())
                .unwrap_or(0);
            (std::cmp::Reverse(size), *name)
        });
        roots.first().map(|name| (*name).to_string())
    }
}

impl Default for DesignState {
    fn default() -> Self {
        Self::new()
    }
}

fn build_row(
    design: &ElaboratedDesign,
    by_instance: &HashMap<&str, Vec<&FlatSignal>>,
    index: usize,
) -> ModuleRow {
    let Some(instance) = design.instances().get(index) else {
        return ModuleRow {
            name: String::new(),
            path: String::new(),
            signals: Vec::new(),
            children: Vec::new(),
        };
    };
    let signals = by_instance
        .get(instance.path.as_str())
        .into_iter()
        .flatten()
        .map(|signal| SignalRow {
            name: signal.local_name.clone(),
            path: signal.path.clone(),
            width: signal.width.unwrap_or(1).max(1) as usize,
        })
        .collect();
    let children = instance
        .children
        .iter()
        .map(|&child| build_row(design, by_instance, child))
        .collect();
    ModuleRow {
        name: instance.name.clone(),
        path: instance.path.clone(),
        signals,
        children,
    }
}

fn usage_label(kind: UsageKind) -> &'static str {
    match kind {
        UsageKind::ContinuousAssign => "assign",
        UsageKind::ProceduralAssign => "procedural",
        UsageKind::PortConnection => "port",
        UsageKind::EventControl => "event",
        UsageKind::Declaration => "declaration",
        UsageKind::Unknown => "reference",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design() -> DesignState {
        let mut state = DesignState::new();
        state.add_source(
            "top.sv",
            "module top;\n  wire clk;\n  child u_c(.clk(clk));\nendmodule\n\
             module child(input clk);\nendmodule\n",
        );
        state
    }

    #[test]
    fn resolve_prefers_exact_then_hierarchical_suffix() {
        let state = design();
        assert_eq!(
            state.resolve_path("top.clk").map(|signal| signal.path.as_str()),
            Some("top.clk")
        );
        assert_eq!(
            state.resolve_path("top.u_c.clk").map(|signal| signal.path.as_str()),
            Some("top.u_c.clk")
        );
        // the dump can have an extra testbench level above the elaborated top
        assert_eq!(
            state.resolve_path("tb_top.clk").map(|signal| signal.path.as_str()),
            Some("top.clk")
        );
        assert_eq!(
            state.resolve_path("tb_top.u_c.clk").map(|signal| signal.path.as_str()),
            Some("top.u_c.clk")
        );
        assert!(state.resolve_path("tb_top.nope").is_none());
    }

    #[test]
    fn ambiguous_paths_do_not_resolve() {
        let state = design();
        // `clk` exists in both modules: a local-name-only lookup must not guess
        assert!(state.resolve_path("clk").is_none());
        assert_eq!(state.resolve_candidates("clk").len(), 2);
        assert!(state.declaration("clk").is_none());
        assert!(state.signal_details("clk").is_none());
    }
}
