//! The sidebar's module tree.
//!
//! Two things can fill the sidebar: the scope tree of an opened dump
//! ([`SidebarTree::from_hierarchy`]) or the instance tree of the elaborated
//! design ([`DesignState::sidebar_tree`](super::DesignState::sidebar_tree)).
//! Both produce this one owned shape, so the tree component does not care which
//! of the two it is showing, and the old `Module` type could go away.

use std::fmt;
use std::rc::Rc;

use wave_insight_lib::waveform::HierarchyView;

/// One signal in the sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignalRow {
    /// Name inside its scope, e.g. `pc`.
    pub name: String,
    /// Full hierarchical path, e.g. `top.u_cpu.pc`; also the row key.
    pub path: String,
    /// Declared width in bits.
    pub width: usize,
}

impl SignalRow {
    /// The name the wave view shows, with the index range for buses.
    pub fn display_name(&self) -> String {
        if self.width <= 1 {
            self.name.clone()
        } else {
            format!("{}[{}:0]", self.name, self.width - 1)
        }
    }
}

/// One scope of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleRow {
    /// Name inside the parent scope.
    pub name: String,
    /// Full dotted path, empty for the virtual root.
    pub path: String,
    /// Signals declared directly in this scope.
    pub signals: Vec<SignalRow>,
    /// Child scopes in declaration order.
    pub children: Vec<ModuleRow>,
}

impl ModuleRow {
    /// Find a scope by full path; the empty path is the root.
    pub fn find(&self, path: &str) -> Option<&ModuleRow> {
        if self.path == path {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(path))
    }

    /// The scope a fresh sidebar should open on: descend through single child
    /// scopes until something with signals (or a real fork) is found.
    pub fn initial(&self) -> &ModuleRow {
        let mut current = self;
        for _ in 0..8 {
            if !current.signals.is_empty() || current.children.len() != 1 {
                break;
            }
            current = &current.children[0];
        }
        current
    }

    /// Total number of signals in this scope and below.
    pub fn total_signals(&self) -> usize {
        self.signals.len()
            + self
                .children
                .iter()
                .map(|child| child.total_signals())
                .sum::<usize>()
    }
}

/// Shared handle on a sidebar tree.
///
/// Compares by pointer, so re-rendering the app never walks a whole design.
#[derive(Clone)]
pub struct SidebarTree(Rc<ModuleRow>);

impl SidebarTree {
    pub fn new(root: ModuleRow) -> Self {
        Self(Rc::new(root))
    }

    /// The tree of a dump: its scopes, in declaration order.
    pub fn from_hierarchy(hierarchy: &HierarchyView) -> Self {
        Self(Rc::new(module_of(hierarchy, hierarchy.root())))
    }

    pub fn root(&self) -> &ModuleRow {
        &self.0
    }

    /// The scope a fresh sidebar should open on.
    pub fn initial_path(&self) -> String {
        self.0.initial().path.clone()
    }

    pub fn find(&self, path: &str) -> Option<&ModuleRow> {
        self.0.find(path)
    }
}

impl PartialEq for SidebarTree {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl fmt::Debug for SidebarTree {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SidebarTree")
            .field("root", &self.0.path)
            .field("signals", &self.0.total_signals())
            .finish()
    }
}

fn module_of(hierarchy: &HierarchyView, node: &wave_insight_lib::waveform::ScopeNode) -> ModuleRow {
    let signals = node
        .signals()
        .iter()
        .filter_map(|&index| hierarchy.signal(index))
        .map(|info| SignalRow {
            name: info.name().to_string(),
            path: info.path().to_string(),
            width: (info.width() as usize).max(1),
        })
        .collect();
    ModuleRow {
        name: node.name().to_string(),
        path: node.path().to_string(),
        signals,
        children: node
            .children()
            .iter()
            .map(|child| module_of(hierarchy, child))
            .collect(),
    }
}
