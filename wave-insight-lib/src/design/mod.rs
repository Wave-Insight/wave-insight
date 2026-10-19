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
//!   signal, drivers/loads of a signal, and "what is at this source position".
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
#[derive(Clone, Debug, Default)]
pub struct DesignDb {
    files: Vec<PathBuf>,
    modules: Vec<ModuleDef>,
    /// Everything that was skipped, repaired or could not be resolved.
    pub diagnostics: Vec<Diagnostic>,
    module_index: HashMap<String, usize>,
    include_paths: Vec<PathBuf>,
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
        self.files.push(path.clone());
        let parsed = frontend::parse_file(&path, None, &self.include_paths);
        self.integrate(parsed)
    }

    /// Parse in-memory source as if it lived at `path`.
    ///
    /// This is what an editor buffer uses; it is also convenient in tests.
    pub fn add_source(&mut self, path: impl AsRef<Path>, source: &str) -> bool {
        let path = path.as_ref().to_path_buf();
        if !self.files.iter().any(|known| known == &path) {
            self.files.push(path.clone());
        }
        let parsed = frontend::parse_file(&path, Some(source), &self.include_paths);
        self.integrate(parsed)
    }

    fn integrate(&mut self, parsed: ParsedFile) -> bool {
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
            self.modules.push(module);
        }
        parsed_ok
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
        self.module_index
            .get(name)
            .map(|index| &self.modules[*index])
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
    pub fn item_at(
        &self,
        design: &ElaboratedDesign,
        file: impl AsRef<Path>,
        line: u32,
        col: u32,
    ) -> Option<SourceItem> {
        let file = file.as_ref();
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
        // specific answers for expression positions.
        for instance in design.instances() {
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
