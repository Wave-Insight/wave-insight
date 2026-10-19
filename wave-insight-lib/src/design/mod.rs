//! SystemVerilog design database: source front end + elaboration.
//!
//! This module replaces the hand written prototype scanner.  It is organised as
//! the reference elaborator is: a *front end* that turns source files into
//! module definitions ([`ModuleDef`]), and an *elaborator* that instantiates a
//! chosen top module into an instance tree plus a flat signal table
//! ([`ElaboratedDesign`]).  The two layers meet in [`DesignDb`], which is what
//! the waveform UI and the future language server talk to.
//!
//! # What is supported
//!
//! * modules with ANSI or non-ANSI headers, including `input a, b;`
//!   declarations that the old scanner mis-parsed;
//! * multiple files (each file is preprocessed and parsed separately; include
//!   paths can be configured with [`DesignDb::with_include_paths`]);
//! * ports with direction, packed range and width, connected by *name* or by
//!   *position*, including `.*`;
//! * parameters and localparams with default values, overridden per instance
//!   both by name and by position;
//! * packed vectors (`[7:0]`, `[WIDTH-1:0]`) and unpacked dimensions, with
//!   widths recomputed with each instance's parameter values;
//! * nets and variables (`wire`, `tri`, `reg`, `logic`, `bit`, integer types,
//!   `real`);
//! * instances, `assign`, `always` / `always_comb` / `always_ff` /
//!   `always_latch` (plus best effort `initial` / `final`) for driver and load
//!   extraction, and port connections as driver/load references;
//! * source spans for every declaration, instance and driver/load reference;
//! * queries: module by name, signal by hierarchical path, declaration of a
//!   signal, drivers/loads of a signal, "what is at this source position",
//!   [`DesignDb::signals_at`] for every instance covering a position, and
//!   [`DesignDb::item_at_in_instance`] for instance-scoped hover;
//! * incremental editing: [`DesignDb::update_source`] reparses one file's
//!   buffer without dropping the rest of the design;
//! * the whole public model is `serde`-serializable, so it can cross the
//!   wasm boundary to the Yew UI (lookup indexes are rebuilt lazily and are
//!   not part of the payload).
//!
//! # What degrades to diagnostics
//!
//! Everything below is skipped with a [`Diagnostic`] (severity
//! `Warning`/`Error`) instead of aborting the file; it never panics:
//!
//! * `generate` regions, loops and conditionals (their contents are not
//!   elaborated);
//! * `typedef`, user defined types, structs/unions/enums, type parameters;
//! * interfaces, interface ports, modports, classes and programs;
//! * packages (package bodies are not analysed; `import` declarations are
//!   ignored), tasks/functions/checkers/DPI declarations (their bodies are not
//!   inspected, calls inside expressions show up as loads);
//! * `defparam`, gate/UDP instantiations, `nettype`, `interconnect`,
//!   `alias`, clocking blocks, `specify` blocks, specparams, assertions,
//!   `bind`, arrayed instances, dynamic arrays and unsized dimensions;
//! * constant expressions outside a small integer subset (integer literals,
//!   parameter references, `+ - * / % << >> **`, comparisons, parentheses,
//!   `$clog2`).  Unsupported width expressions yield `width: None` and an
//!   `unresolved-width` diagnostic.
//!
//! Hierarchical names are resolved relative to the current instance, then
//! absolutely, then in the parent scope; names that resolve to parameters,
//! instance names or functions are simply ignored by the driver/load tables.
//!
//! # Example
//!
//! ```no_run
//! use wave_insight_lib::design::DesignDb;
//!
//! let db = DesignDb::load_files(["tests/data/top.sv", "tests/data/cpu.sv"]);
//! assert!(db.diagnostics().is_empty());
//! let top = db.module("top").expect("top module");
//! assert_eq!(top.ports[0].name, "clk");
//!
//! let design = db.elaborate("top");
//! let pc = design.signal("top.u_cpu.pc").expect("pc is elaborated");
//! assert_eq!(pc.width, Some(8));
//! println!("pc is declared at {}", pc.declaration);
//! for driver in &pc.drivers {
//!     println!("driven at {}", driver.span);
//! }
//! ```

mod const_eval;
mod elaborate;
mod frontend;
mod types;

pub use types::*;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use frontend::ParsedFile;

/// The design database: parsed module definitions plus the diagnostics
/// collected while reading them.
///
/// A `DesignDb` never panics on malformed input.  Check
/// [`DesignDb::diagnostics`] to find out what was skipped or repaired.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct DesignDb {
    files: Vec<PathBuf>,
    modules: Vec<ModuleDef>,
    /// Everything that was skipped, repaired or could not be resolved.
    pub diagnostics: Vec<Diagnostic>,
    /// Lookup acceleration.  Not transported; [`DesignDb::module`] falls back
    /// to a scan when a deserialized database has no index, and the mutating
    /// entry points rebuild it before use.
    #[serde(skip)]
    module_index: HashMap<String, usize>,
    include_paths: Vec<PathBuf>,
    /// Module names contributed by each parsed file, used by
    /// [`DesignDb::update_source`] to replace one file's results.
    #[serde(skip)]
    file_modules: HashMap<PathBuf, Vec<String>>,
}

/// Two databases are equal when their transported data is equal; the derived
/// lookup caches are deliberately ignored (they are rebuilt on demand).
impl PartialEq for DesignDb {
    fn eq(&self, other: &Self) -> bool {
        self.files == other.files
            && self.modules == other.modules
            && self.diagnostics == other.diagnostics
            && self.include_paths == other.include_paths
    }
}

impl DesignDb {
    /// An empty database.
    pub fn new() -> Self {
        DesignDb::default()
    }

    /// An empty database whose files may use `` `include `` with the given
    /// search paths.
    pub fn with_include_paths(include_paths: Vec<PathBuf>) -> Self {
        DesignDb {
            include_paths,
            ..DesignDb::new()
        }
    }

    /// Load and parse several `.sv` / `.v` files.
    pub fn load_files<I, P>(paths: I) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut db = DesignDb::new();
        for path in paths {
            db.add_file(path);
        }
        db
    }

    /// Load and parse several files, with include search paths.
    pub fn load_files_with_includes<I, P>(paths: I, include_paths: Vec<PathBuf>) -> Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let mut db = DesignDb::with_include_paths(include_paths);
        for path in paths {
            db.add_file(path);
        }
        db
    }

    /// Parse one file from disk and add its modules to the database.
    ///
    /// Returns `true` when the file was parsed (even if individual constructs
    /// were skipped with diagnostics).
    pub fn add_file(&mut self, path: impl AsRef<Path>) -> bool {
        let path = path.as_ref().to_path_buf();
        if self.files.iter().any(|known| known == &path) {
            return false;
        }
        self.ensure_indexes();
        self.files.push(path.clone());
        let parsed = frontend::parse_file(&path, None, &self.include_paths);
        self.integrate(&path, parsed)
    }

    /// Parse in-memory source as if it lived at `path`.
    ///
    /// This is what an editor buffer uses; it is also convenient in tests.
    /// Re-using a path that is already loaded replaces its previous modules.
    pub fn add_source(&mut self, path: impl AsRef<Path>, source: &str) -> bool {
        let path = path.as_ref().to_path_buf();
        self.ensure_indexes();
        if !self.files.iter().any(|known| known == &path) {
            self.files.push(path.clone());
        }
        let parsed = frontend::parse_file(&path, Some(source), &self.include_paths);
        self.integrate(&path, parsed)
    }

    /// Replace one file's parse results from an in-memory buffer, keeping all
    /// other files intact.
    ///
    /// This is the editor "save / did change" entry point: modules previously
    /// contributed by `path` are removed, its stale diagnostics are cleared,
    /// and the new source is parsed into the same database.  An unknown path is
    /// added like [`DesignDb::add_source`].
    ///
    /// Returns `true` when the new source parsed (individual constructs may
    /// still have produced non-fatal diagnostics).
    ///
    /// Duplicate module names across files keep first-wins semantics: if the
    /// updated file owned a name, that name is released, but a module that was
    /// skipped as a duplicate is only picked up when its own file is updated
    /// again.
    pub fn update_source(&mut self, path: impl AsRef<Path>, source: &str) -> bool {
        self.add_source(path, source)
    }

    /// Rebuild the lookup maps after deserialization (they are not part of the
    /// transported payload).
    fn ensure_indexes(&mut self) {
        if self.module_index.len() != self.modules.len() {
            self.module_index = self
                .modules
                .iter()
                .enumerate()
                .map(|(index, module)| (module.name.clone(), index))
                .collect();
        }
        if self.file_modules.is_empty() && !self.modules.is_empty() {
            for module in &self.modules {
                self.file_modules
                    .entry(module.span.file.clone())
                    .or_default()
                    .push(module.name.clone());
            }
        }
    }

    fn integrate(&mut self, path: &Path, parsed: ParsedFile) -> bool {
        // Drop the previous generation of this file first so an update cannot
        // leave duplicated or renamed modules behind.
        self.remove_file(path);

        let parsed_ok = !parsed.diagnostics.iter().any(|diagnostic| {
            matches!(
                diagnostic.kind,
                DiagnosticKind::Parse
                    | DiagnosticKind::Preprocess
                    | DiagnosticKind::Io
                    | DiagnosticKind::ParserPanic
            )
        });
        self.diagnostics.extend(parsed.diagnostics);
        let mut names = Vec::new();
        for module in parsed.modules {
            if let Some(&index) = self.module_index.get(&module.name) {
                let previous = &self.modules[index];
                self.diagnostics.push(Diagnostic::error(
                    DiagnosticKind::DuplicateDeclaration,
                    format!(
                        "module `{}` is already defined at {}:{}",
                        module.name, previous.span.file.display(), previous.span.start_line
                    ),
                    Some(module.span.clone()),
                ));
                continue;
            }
            self.module_index
                .insert(module.name.clone(), self.modules.len());
            names.push(module.name.clone());
            self.modules.push(module);
        }
        self.file_modules.insert(path.to_path_buf(), names);
        parsed_ok
    }

    /// Remove modules and diagnostics previously produced by `path`.
    fn remove_file(&mut self, path: &Path) {
        if let Some(names) = self.file_modules.remove(path) {
            let names: std::collections::HashSet<String> = names.into_iter().collect();
            self.modules
                .retain(|module| !names.contains(module.name.as_str()));
            self.module_index = self
                .modules
                .iter()
                .enumerate()
                .map(|(index, module)| (module.name.clone(), index))
                .collect();
        }
        self.diagnostics.retain(|diagnostic| {
            diagnostic
                .span
                .as_ref()
                .map(|span| span.file.as_path() != path)
                .unwrap_or(true)
        });
    }

    /// The files added so far, in insertion order.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// All module definitions, in file/insertion order.
    pub fn modules(&self) -> &[ModuleDef] {
        &self.modules
    }

    /// Find a module definition by name.
    pub fn module(&self, name: &str) -> Option<&ModuleDef> {
        if let Some(index) = self.module_index.get(name) {
            return self.modules.get(*index);
        }
        // Deserialized databases have no index; fall back to a scan.
        self.modules.iter().find(|module| module.name == name)
    }

    /// Everything that was skipped, repaired or could not be resolved.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
    /// Elaborate `top` into an instance tree, a flat signal table and
    /// driver/load relations.
    ///
    /// Elaboration problems are reported in [`ElaboratedDesign::diagnostics`];
    /// the module database is not modified.
    pub fn elaborate(&self, top: &str) -> ElaboratedDesign {
        elaborate::elaborate(&self.modules, top)
    }

    /// Where a signal (by hierarchical path) is declared.
    pub fn declaration_of<'a>(
        &self,
        design: &'a ElaboratedDesign,
        signal_path: &str,
    ) -> Option<&'a Span> {
        design.declaration(signal_path)
    }

    /// Answer "what is at `file:line:col`?".
    ///
    /// `line` and `col` are 1-based.  The innermost span containing the
    /// position wins, so a cursor on a signal reference inside an `assign`
    /// reports [`SourceItem::Load`]/[`SourceItem::Driver`] rather than the
    /// surrounding module.
    ///
    /// A module that is instantiated several times shares one source range;
    /// positions inside it resolve to the first elaborated instance.  Use
    /// [`ElaboratedDesign::signals`] for per-instance data.
    ///
    /// `file` is matched exactly as stored: pass the same path spelling that
    /// was given to [`DesignDb::add_file`]/[`DesignDb::add_source`] (the LSP
    /// layer normalises to absolute paths before calling).
    pub fn item_at(
        &self,
        design: &ElaboratedDesign,
        file: impl AsRef<Path>,
        line: u32,
        col: u32,
    ) -> Option<SourceItem> {
        self.find_item_at(design, file.as_ref(), line, col, None)
    }

    /// Like [`DesignDb::item_at`], but anchored to one elaborated instance.
    ///
    /// When a module body is instantiated several times, every instance shares
    /// the same source text.  This query restricts the answer to the module
    /// body (and child instantiations) of `instance_path`, so hover/definition
    /// inside `top.u_cpu` does not accidentally report `top.u_cpu16`.
    ///
    /// Returns `None` when `instance_path` is not present in `design`.
    ///
    /// `file` is matched exactly as stored (see [`DesignDb::item_at`]).
    pub fn item_at_in_instance(
        &self,
        design: &ElaboratedDesign,
        instance_path: &str,
        file: impl AsRef<Path>,
        line: u32,
        col: u32,
    ) -> Option<SourceItem> {
        let index = design
            .instances()
            .iter()
            .position(|instance| instance.path == instance_path)?;
        self.find_item_at(design, file.as_ref(), line, col, Some(index))
    }

    /// Every hierarchical signal path whose declaration or driver/load
    /// reference covers `file:line:col`, across all elaborated instances.
    ///
    /// This is the "click a line, add the right signal" query: a position in a
    /// module body that is instantiated twice yields both hierarchical paths,
    /// and a port connection yields the parent signal and the child port it
    /// drives or reads.
    ///
    /// `file` is matched exactly as stored (same spelling as
    /// [`DesignDb::add_file`]/[`DesignDb::add_source`]).
    pub fn signals_at(
        &self,
        design: &ElaboratedDesign,
        file: impl AsRef<Path>,
        line: u32,
        col: u32,
    ) -> Vec<String> {
        let file = file.as_ref();
        let mut paths = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for signal in design.signals() {
            let covers = |span: &Span| span.file.as_path() == file && span.contains(line, col);
            let hit = covers(&signal.declaration)
                || signal.drivers.iter().any(|driver| covers(&driver.span))
                || signal.loads.iter().any(|load| covers(&load.span));
            if hit && seen.insert(signal.path.clone()) {
                paths.push(signal.path.clone());
            }
        }
        paths
    }

    fn find_item_at(
        &self,
        design: &ElaboratedDesign,
        file: &Path,
        line: u32,
        col: u32,
        scope: Option<usize>,
    ) -> Option<SourceItem> {
        let scope_instance = scope.map(|index| &design.instances()[index]);
        let mut best: Option<(u64, SourceItem)> = None;
        let mut consider = |span: &Span, item: SourceItem| {
            if span.file.as_path() != file || !span.contains(line, col) {
                return;
            }
            let size = span.size_key();
            let replace = match &best {
                Some((best_size, _)) => size < *best_size,
                None => true,
            };
            if replace {
                best = Some((size, item));
            }
        };

        // Module level declarations first (also covers modules that are not
        // part of the elaborated hierarchy).  On equal-sized spans this makes
        // a port declaration answer `Port` instead of `Signal`.
        for module in &self.modules {
            if let Some(scope) = scope_instance {
                if module.name != scope.module {
                    continue;
                }
            }
            consider(
                &module.span,
                SourceItem::Module {
                    name: module.name.clone(),
                    span: module.span.clone(),
                },
            );
            for port in &module.ports {
                consider(
                    &port.span,
                    SourceItem::Port {
                        module: module.name.clone(),
                        name: port.name.clone(),
                        span: port.span.clone(),
                    },
                );
                if let Some(header_span) = &port.header_span {
                    consider(
                        header_span,
                        SourceItem::Port {
                            module: module.name.clone(),
                            name: port.name.clone(),
                            span: header_span.clone(),
                        },
                    );
                }
            }
            for signal in &module.signals {
                consider(
                    &signal.span,
                    SourceItem::Signal {
                        module: module.name.clone(),
                        name: signal.name.clone(),
                        span: signal.span.clone(),
                    },
                );
            }
        }

        // Elaborated instances and signal references: they are the most
        // specific answers for expression positions.  With a scope, only the
        // scoped instance itself and its direct children are considered, so
        // source inside a child instance is not attributed to siblings.
        for (index, instance) in design.instances().iter().enumerate() {
            if let Some(scope_index) = scope {
                let is_self = index == scope_index;
                let is_child = instance.parent == Some(scope_index);
                if !is_self && !is_child {
                    continue;
                }
            }
            consider(
                &instance.module_span,
                SourceItem::Module {
                    name: instance.module.clone(),
                    span: instance.module_span.clone(),
                },
            );
            consider(
                &instance.span,
                SourceItem::Instance {
                    path: instance.path.clone(),
                    module: instance.module.clone(),
                    span: instance.span.clone(),
                },
            );
        }
        for signal in design.signals() {
            if let Some(scope) = scope_instance {
                if signal.instance != scope.path {
                    continue;
                }
            }
            let name = signal.local_name.clone();
            let module = signal.module.clone();
            consider(
                &signal.declaration,
                SourceItem::Signal {
                    module,
                    name,
                    span: signal.declaration.clone(),
                },
            );
            for driver in &signal.drivers {
                consider(
                    &driver.span,
                    SourceItem::Driver {
                        signal: signal.path.clone(),
                        span: driver.span.clone(),
                        kind: driver.kind,
                    },
                );
            }
            for load in &signal.loads {
                consider(
                    &load.span,
                    SourceItem::Load {
                        signal: signal.path.clone(),
                        span: load.span.clone(),
                        kind: load.kind,
                    },
                );
            }
        }

        best.map(|(_, item)| item)
    }
}
