//! The sidebar's module tree.
//!
//! Two things can fill the sidebar: the scope tree of an opened dump
//! ([`SidebarTree::from_hierarchy`]) or the instance tree of the elaborated
//! design ([`DesignState::sidebar_tree`](super::DesignState::sidebar_tree)).
//! Both produce this one owned shape, so the tree component does not care which
//! of the two it is showing, and the old `Module` type could go away.

use std::collections::HashSet;
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

    /// Number of scopes in this subtree, including this one.
    pub fn count_scopes(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(|child| child.count_scopes())
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

    /// The rows a sidebar shows when `expanded` holds the open scope paths, in
    /// display order (depth first, declaration order).
    ///
    /// The virtual root is not a row itself (as before this change): the root's
    /// children are the top-level rows, and a scope's children appear once it
    /// is expanded. This is exactly the set of rows the old recursive
    /// component rendered, so the sidebar can virtualize the drawing without
    /// changing what is shown.
    pub fn visible_rows(&self, expanded: &HashSet<String>) -> Vec<VisibleRow> {
        let mut rows = Vec::new();
        push_rows(self.root(), 0, expanded, &mut rows);
        rows
    }
}

/// Append the rows of `scope`'s children (and, for expanded scopes, their
/// subtrees) to `rows`.
fn push_rows(
    scope: &ModuleRow,
    depth: usize,
    expanded: &HashSet<String>,
    rows: &mut Vec<VisibleRow>,
) {
    for child in &scope.children {
        let open = expanded.contains(&child.path);
        rows.push((
            child.path.clone(),
            child.name.clone(),
            depth,
            !child.children.is_empty(),
            open,
        ));
        if open && !child.children.is_empty() {
            push_rows(child, depth + 1, expanded, rows);
        }
    }
}

impl PartialEq for SidebarTree {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// One row the sidebar tree shows, as a flat tuple the sidebar module can draw
/// without knowing the tree types: (path, name, depth, has_children, expanded).
///
/// The tuple keeps the drawing type in the sidebar module: `nav` only exports
/// `SidebarTree` and `SignalRow`, and the row windowing needs a plain list.
pub type VisibleRow = (String, String, usize, bool, bool);

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
#[cfg(test)]
mod tests {
    use super::*;

    fn signal(name: &str, width: usize) -> SignalRow {
        SignalRow {
            name: name.to_string(),
            path: format!("top.{name}"),
            width,
        }
    }

    fn scope(
        path: &str,
        name: &str,
        signals: Vec<SignalRow>,
        children: Vec<ModuleRow>,
    ) -> ModuleRow {
        ModuleRow {
            name: name.to_string(),
            path: path.to_string(),
            signals,
            children,
        }
    }

    // root
    //  +- top            clk
    //  |   +- u_cpu      pc
    //  |       +- u_core state
    //  +- top2            (no signals, no children)
    fn sample() -> SidebarTree {
        SidebarTree::new(scope(
            "",
            "",
            vec![],
            vec![
                scope(
                    "top",
                    "top",
                    vec![signal("clk", 1)],
                    vec![scope(
                        "top.u_cpu",
                        "u_cpu",
                        vec![signal("pc", 8)],
                        vec![scope(
                            "top.u_cpu.u_core",
                            "u_core",
                            vec![signal("state", 2)],
                            vec![],
                        )],
                    )],
                ),
                scope("top2", "top2", vec![], vec![]),
            ],
        ))
    }

    fn expand(paths: &[&str]) -> HashSet<String> {
        paths
            .iter()
            .map(|path| (*path).to_string())
            .collect::<HashSet<_>>()
    }

    fn names(rows: &[VisibleRow]) -> Vec<&str> {
        rows.iter().map(|row| row.1.as_str()).collect()
    }

    #[test]
    fn the_root_is_not_a_row() {
        let rows = sample().visible_rows(&HashSet::new());
        assert_eq!(names(&rows), ["top", "top2"]);
        // both are top-level rows, and neither is expanded yet
        assert!(rows.iter().all(|row| row.2 == 0));
        assert!(rows.iter().all(|row| !row.4));
        assert_eq!(rows[0].0, "top");
        assert!(rows[0].3);
        assert!(!rows[1].3);
    }

    #[test]
    fn expanding_a_scope_inserts_its_children_after_it() {
        let rows = sample().visible_rows(&expand(&["top"]));
        assert_eq!(names(&rows), ["top", "u_cpu", "top2"]);
        // the child is one level deeper and is the expanded row's child
        assert_eq!(rows[1].2, 1);
        assert_eq!(rows[1].0, "top.u_cpu");
        // the expanded row carries its open flag, its child does not
        assert!(rows[0].4);
        assert!(!rows[1].4);
    }

    #[test]
    fn nested_expansion_walks_depth_first() {
        let rows = sample().visible_rows(&expand(&["top", "top.u_cpu"]));
        assert_eq!(names(&rows), ["top", "u_cpu", "u_core", "top2"]);
        assert_eq!(rows[2].2, 2);
        assert_eq!(rows[2].0, "top.u_cpu.u_core");
        // a leaf cannot be expanded any further
        let rows = sample().visible_rows(&expand(&["top", "top.u_cpu", "top.u_cpu.u_core"]));
        assert_eq!(names(&rows), ["top", "u_cpu", "u_core", "top2"]);
    }

    #[test]
    fn expansion_flags_follow_the_set() {
        let expanded = expand(&["top.u_cpu"]);
        let rows = sample().visible_rows(&expanded);
        // u_cpu is not a row at all: its parent is closed, so only the parent
        // flag can be set
        assert_eq!(names(&rows), ["top", "top2"]);
        assert!(!rows[0].4);
    }

    #[test]
    fn count_scopes_counts_the_subtree() {
        let tree = sample();
        // root + top + u_cpu + u_core + top2
        assert_eq!(tree.root().count_scopes(), 5);
        let top = tree.find("top").expect("top exists");
        assert_eq!(top.count_scopes(), 3);
        // a signal list is not a scope list
        assert_eq!(tree.root().total_signals(), 3);
    }

    #[test]
    fn an_empty_tree_shows_nothing() {
        let tree = SidebarTree::new(scope("", "", vec![], vec![]));
        assert!(tree.visible_rows(&HashSet::new()).is_empty());
        assert_eq!(tree.root().count_scopes(), 1);
    }

    #[test]
    fn duplicate_names_in_different_scopes_are_different_rows() {
        let tree = SidebarTree::new(scope(
            "",
            "",
            vec![],
            vec![
                scope("top", "top", vec![], vec![]),
                scope("top.u0", "u0", vec![], vec![]),
                scope("top.u1", "u0", vec![], vec![]),
            ],
        ));
        let rows = tree.visible_rows(&expand(&["top"]));
        // the expansion set is keyed by path, so two scopes that happen to
        // share a name stay independent
        assert_eq!(names(&rows), ["top", "u0", "u0"]);
        let paths: Vec<&str> = rows.iter().map(|row| row.0.as_str()).collect();
        assert_eq!(paths, ["top", "top.u0", "top.u1"]);
    }
}
