//! Format independent view of the variable hierarchy.
//!
//! `wellen` exposes a flat arena of scopes and variables linked by handles.
//! The viewer wants paths (`top.u_cpu.pc`), widths, index ranges and a tree it
//! can walk, so we resolve everything once when the file is opened and keep it
//! as plain owned data. Resolving the hierarchy is cheap compared to reading
//! values: it is the file header, not the body.

use std::collections::HashMap;

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
    /// Name inside its scope, e.g. `pc`.
    name: String,
    /// Full dotted path, e.g. `top.u_cpu.pc`.
    path: String,
    /// Path components of the enclosing scope, empty for top level variables.
    scope: Vec<String>,
    /// What the values look like.
    kind: SignalKind,
    /// Width in bits for [`SignalKind::BitVector`], `0` otherwise.
    width: u32,
    /// Declared index range, e.g. `[7:0]` becomes `Some((7, 0))`.
    index_range: Option<(i64, i64)>,
    /// wellen's variable type, e.g. `"Wire"`, `"Logic"`, `"Parameter"`.
    var_type: String,
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
    /// This is also the key the legacy [`crate::data_struct::ModuleValue`]
    /// adapter uses, replacing the VCD identifier code.
    pub fn path(&self) -> &str {
        &self.path
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
        self.index_range
            .map(|(msb, lsb)| format!("[{msb}:{lsb}]"))
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
    /// Name inside the parent scope, empty for the virtual root.
    name: String,
    /// Full dotted path, empty for the virtual root.
    path: String,
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

    /// Child scopes in declaration order.
    pub fn children(&self) -> &[ScopeNode] {
        &self.children
    }

    /// Find a child scope by its local name.
    pub fn child(&self, name: &str) -> Option<&ScopeNode> {
        self.children.iter().find(|child| child.name == name)
    }

    /// Find a scope by full path, e.g. `"top.u_cpu"`. The empty path is the root.
    pub fn find(&self, path: &str) -> Option<&ScopeNode> {
        if self.path == path {
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

/// The resolved variable hierarchy of a waveform file.
#[derive(Clone, Debug)]
pub struct HierarchyView {
    root: ScopeNode,
    signals: Vec<SignalInfo>,
    by_path: HashMap<String, usize>,
    /// wellen signal handle index -> first variable index that uses it.
    ///
    /// Two variables can alias one signal (e.g. a bus and an alias of it); the
    /// value stream reports the signal, we map it back to a variable.
    var_of_signal: HashMap<u32, usize>,
}

impl HierarchyView {
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
    pub fn signal_by_path(&self, path: &str) -> Option<&SignalInfo> {
        self.by_path.get(path).and_then(|&i| self.signals.get(i))
    }

    /// Map a decoded wellen signal handle back to a variable index.
    pub(crate) fn variable_of_signal(&self, signal: SignalRef) -> Option<usize> {
        self.var_of_signal.get(&(signal.index() as u32)).copied()
    }

    /// Resolve the hierarchy of a freshly read header.
    pub(crate) fn build(hierarchy: &Hierarchy) -> Self {
        let mut builder = Builder {
            signals: Vec::with_capacity(hierarchy.all_vars().count()),
            by_path: HashMap::new(),
            var_of_signal: HashMap::new(),
        };
        let root = builder.build_scope(hierarchy, None, "");
        Self {
            root,
            signals: builder.signals,
            by_path: builder.by_path,
            var_of_signal: builder.var_of_signal,
        }
    }
}

struct Builder {
    signals: Vec<SignalInfo>,
    by_path: HashMap<String, usize>,
    var_of_signal: HashMap<u32, usize>,
}

impl Builder {
    /// `scope == None` means the virtual root, whose items are the top level
    /// scopes and the variables declared outside of any scope.
    fn build_scope(
        &mut self,
        hierarchy: &Hierarchy,
        scope: Option<ScopeRef>,
        path: &str,
    ) -> ScopeNode {
        let mut node = ScopeNode {
            name: scope.map(|s| hierarchy[s].name(hierarchy).to_string()).unwrap_or_default(),
            path: path.to_string(),
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
                    let index = self.add_var(hierarchy, var, path);
                    node.signals.push(index);
                }
                ItemRef::Scope(child) => {
                    let child_name = hierarchy[child].name(hierarchy);
                    let child_path = join_path(path, child_name);
                    node.children
                        .push(self.build_scope(hierarchy, Some(child), &child_path));
                }
            }
        }

        node
    }

    fn add_var(&mut self, hierarchy: &Hierarchy, var_ref: VarRef, scope_path: &str) -> usize {
        let var = &hierarchy[var_ref];
        let signal_ref = var.signal_ref();

        //wellen tells us the encoding per signal; fall back to the declared
        //length for files without encoding information
        let (kind, width) = match hierarchy.get_signal_tpe(signal_ref) {
            Some(encoding) => (SignalKind::from_encoding(encoding), encoding.length().unwrap_or(0)),
            None => match var.length(hierarchy) {
                Some(0) => (SignalKind::Event, 0),
                Some(width) => (SignalKind::BitVector, width),
                None => (SignalKind::Event, 0),
            },
        };

        let name = var.name(hierarchy).to_string();
        let index = self.signals.len();
        let info = SignalInfo {
            index,
            path: join_path(scope_path, &name),
            name,
            scope: scope_path
                .split('.')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect(),
            kind,
            width,
            index_range: var.index().map(|range| (range.msb(), range.lsb())),
            var_type: format!("{:?}", var.var_type()),
            signal_ref,
        };

        self.by_path.insert(info.path.clone(), index);
        self.var_of_signal
            .entry(signal_ref.index() as u32)
            .or_insert(index);
        self.signals.push(info);
        index
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}
