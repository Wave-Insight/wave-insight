//! The sidebar's module tree.
//!
//! Two things can fill the sidebar: the scope tree of an opened dump
//! ([`SidebarTree::from_hierarchy`] / [`SidebarTree::from_hierarchies`]) or the
//! instance tree of the elaborated design
//! ([`DesignState::sidebar_tree`](super::DesignState::sidebar_tree)). Both
//! produce this one owned shape, so the tree component does not care which of
//! the two it is showing, and the old `Module` type could go away.
//!
//! With comparison dumps loaded ([`SidebarTree::from_hierarchies`]) the tree is
//! the base dump's scopes **plus one synthetic root per comparison member**,
//! labelled `compare 2 — run2.vcd`, holding that member's own top scopes under a
//! reserved path prefix. A member-only signal is therefore reachable, and the
//! signal rows carry a [`SignalRow::members`] mask saying which dumps have it.
//! Scope paths inside a member root are prefixed so two scopes that share a name
//! (`top` in base and in member) stay distinct rows; signal paths are **not**
//! rewritten — the same dotted path is what makes one wave row draw every dump's
//! series.

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
    /// Bitmask of the loaded dumps that have this signal: bit 0 is the base
    /// dump, bit `slot - 1` a comparison member. `0` when nothing says (a design
    /// tree, a native dump).
    pub members: u8,
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

/// One comparison slot as the rows and the toolbar label it: the number, the
/// dump's file name and the palette colour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotLabel {
    pub slot: u8,
    pub name: String,
    pub color: (u8, u8, u8),
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

/// Reserved prefix of a comparison member's synthetic root path.
///
/// A member's scopes are *not* part of the base dump's hierarchy, so they are
/// keyed under a path no dump can declare (`compare #2` is not a Verilog
/// identifier), which keeps the expansion set and the row keys unambiguous while
/// `top` exists in both trees.
pub const COMPARE_ROOT_PREFIX: &str = "#compare";

/// The path of the synthetic root of comparison `slot`.
pub fn compare_root_path(slot: u8) -> String {
    format!("{COMPARE_ROOT_PREFIX}{slot}")
}

/// The visible label of a comparison root: `compare 2 — run2.vcd`.
pub fn compare_root_name(slot: u8, name: &str) -> String {
    if name.is_empty() {
        format!("compare {slot}")
    } else {
        format!("compare {slot} — {name}")
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

    /// The tree of a dump **and** its comparison members.
    ///
    /// The base hierarchy's tree comes first, unchanged; then one synthetic root
    /// per member (`(slot, name, hierarchy)`), labelled
    /// [`compare_root_name`] and keyed under [`compare_root_path`]. Every signal
    /// row carries the mask of the dumps that resolved the same dotted path, so
    /// the sidebar can badge it without walking the hierarchies again.
    ///
    /// `extras` is empty for the no-comparison case; `base` is the dump the rows
    /// belong to, so its bit is always set.
    pub fn from_hierarchies(base: &HierarchyView, extras: &[(u8, &str, &HierarchyView)]) -> Self {
        let mut root = base_tree(base);
        if extras.is_empty() {
            return Self::new(root);
        }
        // one mask lookup per signal of the base tree, plus one per member signal
        let mask_of = |path: &str| -> u8 {
            let mut mask = 1u8;
            for (slot, _, hierarchy) in extras {
                let bit = crate::state::compare::slot_bit(*slot);
                if hierarchy.signal_by_path(path).is_some() {
                    mask |= bit;
                }
            }
            mask
        };
        for signal in root_signals_mut(&mut root) {
            signal.members = mask_of(&signal.path);
        }
        for (slot, name, hierarchy) in extras {
            // the member's own scope tree; its scope keys move under the
            // reserved prefix, its signal paths stay exactly the dump's paths
            // (one wave row, one lookup)
            let mut member = module_of(hierarchy, hierarchy.root());
            key_under(&mut member, &compare_root_path(*slot));
            for signal in root_signals_mut(&mut member) {
                signal.members = mask_of(&signal.path);
            }
            member.name = compare_root_name(*slot, name);
            member.path = compare_root_path(*slot);
            root.children.push(member);
        }
        Self::new(root)
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

/// The base tree of a hierarchy (its signal masks are filled in by
/// [`SidebarTree::from_hierarchies`] when comparison dumps are loaded).
fn base_tree(hierarchy: &HierarchyView) -> ModuleRow {
    module_of(hierarchy, hierarchy.root())
}

/// Build a scope subtree. Every signal starts with mask `0` ("no comparison
/// loaded"); [`SidebarTree::from_hierarchies`] fills the masks in.
fn module_of(hierarchy: &HierarchyView, node: &wave_insight_lib::waveform::ScopeNode) -> ModuleRow {
    let signals = node
        .signals()
        .iter()
        .filter_map(|&index| hierarchy.signal(index))
        .map(|info| SignalRow {
            name: info.name().to_string(),
            path: info.path().to_string(),
            width: (info.width() as usize).max(1),
            members: 0,
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

/// Re-key every scope of `scope` (and its descendants) under `prefix`.
///
/// The top level call sees the member's *root* itself, which is not a row; only
/// its children and below need the prefix (a member root is by definition not a
/// base scope, so nothing can collide with it).
fn key_under(scope: &mut ModuleRow, prefix: &str) {
    let key = |path: &str| -> String {
        if path.is_empty() {
            prefix.to_string()
        } else {
            format!("{prefix}.{path}")
        }
    };
    for child in &mut scope.children {
        child.path = key(&child.path);
        key_under(child, prefix);
    }
}

/// Every signal of a scope subtree, for the mask pass.
fn root_signals_mut(scope: &mut ModuleRow) -> Vec<&mut SignalRow> {
    let mut signals: Vec<&mut SignalRow> = scope.signals.iter_mut().collect();
    for child in &mut scope.children {
        signals.extend(root_signals_mut(child));
    }
    signals
}
#[cfg(test)]
mod tests {
    use super::*;

    fn signal(name: &str, width: usize) -> SignalRow {
        SignalRow {
            name: name.to_string(),
            path: format!("top.{name}"),
            width,
            members: 0,
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

    // --- the union tree of a dump and its comparison members ----------------

    /// Two tiny VCDs: the base has `clk` and `pc` under `top`, the member has
    /// `clk` and its own `only_here` under *its* `top` (the same scope name on
    /// purpose: the member root must keep the two apart).
    fn dump(text: &str) -> wave_insight_lib::waveform::Waveform {
        wave_insight_lib::waveform::Waveform::open_bytes(text.as_bytes().to_vec())
            .expect("the test dump opens")
    }

    fn base_dump() -> wave_insight_lib::waveform::Waveform {
        dump(
            "$timescale 1ns $end\n\
             $scope module top $end\n\
             $var wire 1 ! clk $end\n\
             $var wire 8 # pc [7:0] $end\n\
             $upscope $end\n\
             $enddefinitions $end\n\
             $dumpvars\n0!\nb0 #\n$end\n#10\n1!\n",
        )
    }

    fn member_dump() -> wave_insight_lib::waveform::Waveform {
        dump(
            "$timescale 1ns $end\n\
             $scope module top $end\n\
             $var wire 1 ! clk $end\n\
             $var wire 4 % only_here [3:0] $end\n\
             $upscope $end\n\
             $enddefinitions $end\n\
             $dumpvars\n0!\nb0 %\n$end\n#10\n1!\n",
        )
    }

    #[test]
    fn the_no_comparison_tree_is_unchanged() {
        let base = base_dump();
        let plain = SidebarTree::from_hierarchy(base.hierarchy());
        let union = SidebarTree::from_hierarchies(base.hierarchy(), &[]);
        // one top-level scope, no synthetic root
        assert_eq!(names(&plain.visible_rows(&HashSet::new())), ["top"]);
        assert_eq!(
            names(&union.visible_rows(&HashSet::new())),
            names(&plain.visible_rows(&HashSet::new()))
        );
        // and with no comparison loaded the masks say nothing
        assert_eq!(plain.root().total_signals(), 2);
        assert!(union
            .root()
            .children
            .iter()
            .flat_map(|scope| scope.signals.iter())
            .all(|signal| signal.members == 0));
    }

    #[test]
    fn the_union_tree_puts_the_base_first_and_one_labelled_root_per_member() {
        let base = base_dump();
        let member = member_dump();
        let union =
            SidebarTree::from_hierarchies(base.hierarchy(), &[(2, "run2.vcd", member.hierarchy())]);
        let rows = union.visible_rows(&HashSet::new());
        let roots = names(&rows);
        assert_eq!(roots, ["top", "compare 2 — run2.vcd"]);
        // the member root is keyed under the reserved prefix, so its `top` is not
        // the base `top`
        let member_root = union
            .find(&compare_root_path(2))
            .expect("the member root exists");
        assert_eq!(member_root.name, "compare 2 — run2.vcd");
        assert_eq!(member_root.children.len(), 1);
        assert_eq!(member_root.children[0].name, "top");
        assert_eq!(member_root.children[0].path, "#compare2.top");
    }

    #[test]
    fn a_member_only_signal_is_reachable_and_carries_the_right_mask() {
        let base = base_dump();
        let member = member_dump();
        let union =
            SidebarTree::from_hierarchies(base.hierarchy(), &[(2, "run2.vcd", member.hierarchy())]);
        // expand both roots: the module rows are the base `top`, the member root
        // and the member's own `top` (its signals live in the signal list, as the
        // sidebar only ever draws scopes here)
        let rows = union.visible_rows(&expand(&["top", "#compare2", "#compare2.top"]));
        let names: Vec<&str> = rows.iter().map(|row| row.1.as_str()).collect();
        assert_eq!(names, ["top", "compare 2 — run2.vcd", "top"]);
        // the base clk is in both dumps (bit 0 + slot 2)
        let base_top = union.find("top").expect("the base scope");
        let clk = base_top
            .signals
            .iter()
            .find(|signal| signal.name == "clk")
            .expect("the base has clk");
        assert_eq!(clk.members, 0b11);
        // pc exists only in the base
        let pc = base_top
            .signals
            .iter()
            .find(|signal| signal.name == "pc")
            .expect("the base has pc");
        assert_eq!(pc.members, 0b1);
        // the member-only signal is reachable under the member root and resolves
        // to the member's own dump path (so adding it draws the member's series)
        let member_top = union.find("#compare2.top").expect("the member scope");
        let only_here = member_top
            .signals
            .iter()
            .find(|signal| signal.name == "only_here")
            .expect("the member has a signal the base does not");
        assert_eq!(only_here.path, "top.only_here");
        assert_eq!(only_here.members, 0b11);
    }

    #[test]
    fn two_members_are_two_roots_in_slot_order() {
        let base = base_dump();
        let member = member_dump();
        let union = SidebarTree::from_hierarchies(
            base.hierarchy(),
            &[
                (3, "run3.fst", member.hierarchy()),
                (2, "run2.vcd", member.hierarchy()),
            ],
        );
        // the caller's order is kept: the app passes the set in slot order
        let rows = union.visible_rows(&HashSet::new());
        let roots = names(&rows);
        assert_eq!(
            roots,
            ["top", "compare 3 — run3.fst", "compare 2 — run2.vcd"]
        );
        // a mask names every member that has the signal
        let base_top = union.find("top").expect("the base scope");
        let clk = base_top
            .signals
            .iter()
            .find(|signal| signal.name == "clk")
            .expect("clk");
        assert_eq!(clk.members, 0b1 | 0b10 | 0b100);
    }
}
