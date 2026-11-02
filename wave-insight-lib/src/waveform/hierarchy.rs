//! Format independent view of the variable hierarchy.
//!
//! `wellen` exposes a flat arena of scopes and variables linked by handles.
//! The viewer wants paths (`top.u_cpu.pc`), widths, index ranges and a tree it
//! can walk, so we resolve everything once when the file is opened. Resolving
//! the hierarchy is cheap compared to reading values: it is the file header,
//! not the body.
//!
//! The resolved form is deliberately compact: names and variable types are
//! interned (`Arc<str>`), every signal in a scope shares that scope's
//! `Arc<[String]>` components, and the full path is built lazily in a
//! `OnceLock<String>` on first `path()`.  There is no path index map: a lookup
//! walks the scope tree instead (see [`HierarchyView::signal_by_path`]).  The
//! accounting lives in [`HierarchyStats`] and [`HierarchyView::memory_stats`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use wellen::{Hierarchy, ItemRef, ScopeRef, SignalEncoding, SignalRef, VarRef};

/// What kind of payload a variable carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SignalKind {
    /// A two/four/nine state bit vector of [`SignalInfo::width`] bits.
    BitVector,
    /// An event without a payload (width 0).
    Event,
    /// A 64 bit floating point value.
    Real,
    /// A variable length string.
    String,
}

impl SignalKind {
    /// Derive from wellen's encoding information.
    ///
    /// `SignalEncoding::BitVector(0)` is how wellen represents events.
    pub fn from_encoding(encoding: SignalEncoding) -> Self {
        match encoding {
            SignalEncoding::BitVector(0) => SignalKind::Event,
            SignalEncoding::BitVector(_) => SignalKind::BitVector,
            SignalEncoding::Real => SignalKind::Real,
            SignalEncoding::String => SignalKind::String,
        }
    }

    /// `true` for anything that is shown as a bit pattern.
    pub fn is_bit_vector(self) -> bool {
        matches!(self, SignalKind::BitVector)
    }
}

/// One variable of the design.
///
/// Variables are addressed by [`SignalInfo::index`] in all value queries. The
/// index is stable for the lifetime of the [`super::Waveform`].
#[derive(Clone, Debug)]
pub struct SignalInfo {
    /// Dense index, valid as a value query argument.
    index: usize,
    /// Name inside its scope, e.g. `pc`.  Interned: the same local name in many
    /// scopes shares one allocation.
    name: Arc<str>,
    /// Path components of the enclosing scope, empty for top level variables.
    /// Shared by every signal declared in the same scope.
    scope: Arc<[String]>,
    /// Full dotted path, e.g. `top.u_cpu.pc`, built on first use only.  100k
    /// signals no longer pre-allocate 100k full path strings (and no scope
    /// path is stored per signal: it is rebuilt from the shared components).
    path: OnceLock<String>,
    /// What the values look like.
    kind: SignalKind,
    /// Width in bits for [`SignalKind::BitVector`], `0` otherwise.
    width: u32,
    /// Declared index range, e.g. `[7:0]` becomes `Some((7, 0))`.
    index_range: Option<(i64, i64)>,
    /// wellen's variable type, e.g. `"Wire"`, `"Logic"`, `"Parameter"`.
    /// Interned: there are only a handful of distinct types.
    var_type: Arc<str>,
    /// Handle into wellen's hierarchy, used to request the values.
    pub(crate) signal_ref: SignalRef,
}

impl SignalInfo {
    /// Dense index, use it for [`super::Waveform::changes_in_window`] etc.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Name inside its scope, e.g. `pc`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Full dotted path, e.g. `top.u_cpu.pc`.
    ///
    /// This is the key the wave view uses to match a row against the dump (the
    /// VCD identifier code is not preserved by wellen).
    pub fn path(&self) -> &str {
        let scope = Arc::clone(&self.scope);
        let name = Arc::clone(&self.name);
        self.path
            .get_or_init(|| {
                if scope.is_empty() {
                    name.to_string()
                } else {
                    let mut path = String::with_capacity(
                        scope
                            .iter()
                            .map(|component| component.len() + 1)
                            .sum::<usize>()
                            + name.len(),
                    );
                    for component in scope.iter() {
                        path.push_str(component);
                        path.push('.');
                    }
                    path.push_str(&name);
                    path
                }
            })
            .as_str()
    }

    /// Path components of the enclosing scope, e.g. `["top", "u_cpu"]`.
    pub fn scope(&self) -> &[String] {
        &self.scope
    }

    /// What kind of payload the values have.
    pub fn kind(&self) -> SignalKind {
        self.kind
    }

    /// Width in bits; `0` for events, reals and strings.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Declared index range, e.g. `Some((7, 0))` for `$var wire 8 "\ data [7:0]`.
    pub fn index_range(&self) -> Option<(i64, i64)> {
        self.index_range
    }

    /// `"[7:0]"` if the file declares an index range.
    pub fn range_string(&self) -> Option<String> {
        self.index_range.map(|(msb, lsb)| format!("[{msb}:{lsb}]"))
    }

    /// wellen's variable type name, e.g. `"Wire"` or `"Parameter"`.
    pub fn var_type(&self) -> &str {
        &self.var_type
    }

    /// `true` if this variable holds a bit vector.
    pub fn is_bit_vector(&self) -> bool {
        self.kind.is_bit_vector()
    }

    /// `true` if this variable is one bit wide.
    pub fn is_1bit(&self) -> bool {
        self.kind.is_bit_vector() && self.width == 1
    }
}

/// A scope of the design, with the variables and child scopes it declares.
#[derive(Clone, Debug, Default)]
pub struct ScopeNode {
    /// Name inside the parent scope, empty for the virtual root; interned.
    name: Arc<str>,
    /// Full dotted path, empty for the virtual root; shared with the signals
    /// declared in this scope.
    path: Arc<str>,
    /// Path components of this scope (same as the signals' `scope` field).
    components: Arc<[String]>,
    /// Indices of the variables declared directly in this scope.
    signals: Vec<usize>,
    /// Child scopes in declaration order.
    children: Vec<ScopeNode>,
}

impl ScopeNode {
    /// Name inside the parent scope, empty for the virtual root.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Full dotted path, empty for the virtual root.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Variables declared directly in this scope.
    pub fn signals(&self) -> &[usize] {
        &self.signals
    }

    /// Path components of this scope (empty for the virtual root).
    pub fn components(&self) -> &[String] {
        &self.components
    }

    /// Child scopes in declaration order.
    pub fn children(&self) -> &[ScopeNode] {
        &self.children
    }

    /// Find a child scope by its local name.
    pub fn child(&self, name: &str) -> Option<&ScopeNode> {
        self.children.iter().find(|child| &*child.name == name)
    }

    /// Find a scope by full path, e.g. `"top.u_cpu"`. The empty path is the root.
    pub fn find(&self, path: &str) -> Option<&ScopeNode> {
        if &*self.path == path {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(path))
    }

    /// Recursively visit this scope and all child scopes (depth first).
    pub fn walk(&self, visit: &mut impl FnMut(&ScopeNode)) {
        visit(self);
        for child in &self.children {
            child.walk(visit);
        }
    }

    /// Total number of variables in this scope and all child scopes.
    pub fn total_signals(&self) -> usize {
        let mut count = 0usize;
        self.walk(&mut |scope| count += scope.signals.len());
        count
    }
}

/// Heap accounting of the resolved hierarchy, by category.
///
/// This is the library's own accounting (no allocator hooks): byte lengths of
/// the strings, struct sizes and container capacities.  It is meant for a
/// memory_stats()-style report and for regression tests that assert interning
/// shrinks the hierarchy, not for exact RSS accounting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HierarchyStats {
    /// Number of scopes, including the virtual root.
    pub scopes: usize,
    /// Number of variables.
    pub signals: usize,
    /// size_of of the scope tree and the signal vector.
    pub struct_bytes: usize,
    /// Bytes of the variable/scope names and variable types.
    pub name_bytes: usize,
    /// Bytes of the full-path strings (signal paths, scope paths, lookup keys).
    pub path_bytes: usize,
    /// Bytes of the per-signal scope component vectors.
    pub scope_component_bytes: usize,
    /// Approximate bytes of the path/alias lookup structures.
    pub index_bytes: usize,
    /// Unique interned strings (0 when interning is not used).
    pub interned_strings: usize,
    /// Bytes of the unique interned string allocations.
    pub interned_bytes: usize,
}

impl HierarchyStats {
    /// Sum of every category.
    pub fn total_bytes(&self) -> usize {
        self.struct_bytes
            + self.name_bytes
            + self.path_bytes
            + self.scope_component_bytes
            + self.index_bytes
    }
}

/// The resolved variable hierarchy of a waveform file.
#[derive(Clone, Debug)]
pub struct HierarchyView {
    root: ScopeNode,
    signals: Vec<SignalInfo>,
    /// wellen signal handle index -> every variable index that uses it, first
    /// variable first.
    ///
    /// Two variables can alias one signal (the same VCD identifier code); the
    /// value stream reports the signal once, so the caller needs all variables
    /// to report the change against the row that was actually requested.
    var_of_signal: HashMap<u32, Vec<usize>>,
}

impl HierarchyView {
    /// Heap accounting of the hierarchy (see [HierarchyStats]).
    pub fn memory_stats(&self) -> HierarchyStats {
        let mut stats = HierarchyStats {
            signals: self.signals.len(),
            ..HierarchyStats::default()
        };
        let mut scopes = 0usize;
        let mut unique: HashSet<(usize, usize)> = HashSet::new();
        self.root.walk(&mut |node| {
            scopes += 1;
            if unique.insert((node.name.len(), node.name.as_ptr() as usize)) {
                stats.interned_strings += 1;
                stats.interned_bytes += node.name.len();
            }
            stats.path_bytes += node.path.len();
            stats.scope_component_bytes += std::mem::size_of::<String>() * node.components.len()
                + node.components.iter().map(String::len).sum::<usize>();
            stats.index_bytes += node.children.capacity() * std::mem::size_of::<ScopeNode>();
            stats.index_bytes += node.signals.capacity() * std::mem::size_of::<usize>();
        });
        stats.scopes = scopes;
        stats.struct_bytes = self.signals.len() * std::mem::size_of::<SignalInfo>()
            + scopes * std::mem::size_of::<ScopeNode>();
        for signal in &self.signals {
            if unique.insert((signal.name.len(), signal.name.as_ptr() as usize)) {
                stats.interned_strings += 1;
                stats.interned_bytes += signal.name.len();
            }
            if unique.insert((signal.var_type.len(), signal.var_type.as_ptr() as usize)) {
                stats.interned_strings += 1;
                stats.interned_bytes += signal.var_type.len();
            }
            if let Some(path) = signal.path.get() {
                stats.path_bytes += path.len();
            }
        }
        stats.index_bytes += self.var_of_signal.len()
            * (std::mem::size_of::<u32>() + std::mem::size_of::<Vec<usize>>());
        stats
    }

    /// The virtual root; its children are the top level scopes of the file.
    pub fn root(&self) -> &ScopeNode {
        &self.root
    }

    /// All variables in depth first declaration order.
    pub fn signals(&self) -> &[SignalInfo] {
        &self.signals
    }

    /// Number of variables.
    pub fn len(&self) -> usize {
        self.signals.len()
    }

    /// `true` if the file declares no variables at all.
    pub fn is_empty(&self) -> bool {
        self.signals.is_empty()
    }

    /// Look a variable up by its dense index.
    pub fn signal(&self, index: usize) -> Option<&SignalInfo> {
        self.signals.get(index)
    }

    /// Look a variable up by its full path (`top.u_cpu.pc`).
    ///
    /// The hierarchy no longer stores every full path (that was one allocation
    /// per signal): the path is walked segment by segment through the scope tree
    /// instead, with a linear scan for the variable name in its scope.  Lookups
    /// are O(path depth + signals of the scope), the documented trade-off for
    /// the memory win.
    pub fn signal_by_path(&self, path: &str) -> Option<&SignalInfo> {
        if path.is_empty() {
            return None;
        }
        let mut node = &self.root;
        let mut parts = path.split('.');
        let mut current = parts.next()?;
        loop {
            let Some(next) = parts.next() else {
                let index = node
                    .signals
                    .iter()
                    .copied()
                    .find(|&index| self.signals[index].name() == current)?;
                return self.signals.get(index);
            };
            node = node.children.iter().find(|child| child.name() == current)?;
            current = next;
        }
    }

    /// Map a decoded wellen signal handle back to every variable that aliases
    /// it (the first variable first).
    pub(crate) fn variables_of_signal(&self, signal: SignalRef) -> Option<&[usize]> {
        self.var_of_signal
            .get(&(signal.index() as u32))
            .map(Vec::as_slice)
    }

    /// Resolve the hierarchy of a freshly read header.
    ///
    /// Returns an error instead of recursing without a bound when a file
    /// declares an absurdly deep scope nesting (the builder is recursive, and a
    /// stack overflow cannot be turned into an error afterwards).
    pub(crate) fn build(hierarchy: &Hierarchy) -> std::result::Result<Self, String> {
        let mut builder = Builder {
            signals: Vec::with_capacity(hierarchy.all_vars().count()),
            var_of_signal: HashMap::new(),
            interner: HashMap::new(),
        };
        let root = builder.build_scope(
            hierarchy,
            None,
            Arc::from(""),
            Arc::<[String]>::from(Vec::<String>::new()),
            0,
        )?;
        Ok(Self {
            root,
            signals: builder.signals,
            var_of_signal: builder.var_of_signal,
        })
    }
}

/// Upper bound on the scope nesting depth the builder recurses into.
const MAX_SCOPE_DEPTH: usize = 256;

struct Builder {
    signals: Vec<SignalInfo>,
    var_of_signal: HashMap<u32, Vec<usize>>,
    /// The same scope/variable/type names repeat thousands of times; intern
    /// them so one allocation serves every occurrence.
    interner: HashMap<String, Arc<str>>,
}

impl Builder {
    /// Intern a name: the first occurrence allocates, the rest clone the Arc.
    fn intern(&mut self, text: &str) -> Arc<str> {
        if let Some(existing) = self.interner.get(text) {
            return Arc::clone(existing);
        }
        let string: Arc<str> = Arc::from(text);
        self.interner.insert(text.to_owned(), Arc::clone(&string));
        string
    }
}

impl Builder {
    /// `scope == None` means the virtual root, whose items are the top level
    /// scopes and the variables declared outside of any scope.
    fn build_scope(
        &mut self,
        hierarchy: &Hierarchy,
        scope: Option<ScopeRef>,
        path: Arc<str>,
        components: Arc<[String]>,
        depth: usize,
    ) -> std::result::Result<ScopeNode, String> {
        if depth > MAX_SCOPE_DEPTH {
            return Err(format!(
                "scope hierarchy is nested deeper than {MAX_SCOPE_DEPTH} levels"
            ));
        }
        let name = match scope {
            Some(scope) => self.intern(hierarchy[scope].name(hierarchy)),
            None => self.intern(""),
        };
        let mut node = ScopeNode {
            name,
            path,
            components,
            signals: Vec::new(),
            children: Vec::new(),
        };

        let items: Vec<ItemRef> = match scope {
            None => hierarchy.items().collect(),
            Some(scope) => hierarchy[scope].items(hierarchy).collect(),
        };

        for item in items {
            match item {
                ItemRef::Var(var) => {
                    let index = self.add_var(hierarchy, var, &node.components);
                    node.signals.push(index);
                }
                ItemRef::Scope(child) => {
                    let child_name = self.intern(hierarchy[child].name(hierarchy));
                    let child_path: Arc<str> = if node.path.is_empty() {
                        Arc::clone(&child_name)
                    } else {
                        Arc::from(format!("{}.{}", node.path, child_name))
                    };
                    let mut child_components: Vec<String> =
                        node.components.iter().cloned().collect();
                    child_components.push(child_name.to_string());
                    let child_components: Arc<[String]> = Arc::from(child_components);
                    node.children.push(self.build_scope(
                        hierarchy,
                        Some(child),
                        child_path,
                        child_components,
                        depth + 1,
                    )?);
                }
            }
        }

        Ok(node)
    }

    fn add_var(&mut self, hierarchy: &Hierarchy, var_ref: VarRef, scope: &Arc<[String]>) -> usize {
        let var = &hierarchy[var_ref];
        let signal_ref = var.signal_ref();

        //wellen tells us the encoding per signal; fall back to the declared
        //length for files without encoding information
        let (kind, width) = match hierarchy.get_signal_tpe(signal_ref) {
            Some(encoding) => (
                SignalKind::from_encoding(encoding),
                encoding.length().unwrap_or(0),
            ),
            None => match var.length(hierarchy) {
                Some(0) => (SignalKind::Event, 0),
                Some(width) => (SignalKind::BitVector, width),
                None => (SignalKind::Event, 0),
            },
        };

        let name = self.intern(var.name(hierarchy));
        let var_type = self.intern(&format!("{:?}", var.var_type()));
        let index = self.signals.len();
        let info = SignalInfo {
            index,
            name,
            scope: Arc::clone(scope),
            path: OnceLock::new(),
            kind,
            width,
            index_range: var.index().map(|range| (range.msb(), range.lsb())),
            var_type,
            signal_ref,
        };

        self.var_of_signal
            .entry(signal_ref.index() as u32)
            .or_default()
            .push(index);
        self.signals.push(info);
        index
    }
}
