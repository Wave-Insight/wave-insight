//! Bulk adding signals from the module tree (Wave 12).
//!
//! The module tree is the most common way to fill the wave view, and adding one
//! signal per click does not scale. This module holds the pure parts: walking a
//! scope subtree for its signal paths, merging several scopes, and the visible
//! range a Shift+click selects. The reducer owns the batch itself (dedupe,
//! row_prefs, one rows-version bump, one session write).

use crate::nav::tree::ModuleRow;
use crate::nav::SidebarTree;

/// A batch larger than this is refused with an actionable status line: a
/// mis-click on a huge design must not freeze the page.
pub const BULK_ADD_CAP: usize = 2000;

/// Every signal path of the scope at `path`, including its descendants, in tree
/// order. Paths are the fully expanded ones the tree already holds (so a
/// generated instance adds `g[0].u_leaf.x`, never a wildcard).
pub fn signals_under(tree: &SidebarTree, path: &str) -> Vec<String> {
    let Some(scope) = tree.find(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    collect_scope(scope, &mut out);
    out
}

fn collect_scope(scope: &ModuleRow, out: &mut Vec<String>) {
    for signal in &scope.signals {
        out.push(signal.path.clone());
    }
    for child in &scope.children {
        collect_scope(child, out);
    }
}

/// The signals of several scopes, deduped in first-seen order (a selected
/// parent and its selected child must not add the same signal twice).
pub fn signals_under_many(tree: &SidebarTree, paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for path in paths {
        for signal in signals_under(tree, path) {
            if !out.contains(&signal) {
                out.push(signal);
            }
        }
    }
    out
}

/// The scope a signal path lives in (`top.u_cpu.pc` -> `top.u_cpu`).
///
/// Used by the signal-row context menu: bulk add works on scopes, so
/// "select for bulk add" on a signal selects its parent scope.
pub fn parent_scope(path: &str) -> Option<String> {
    path.rsplit_once('.')
        .map(|(parent, _)| parent.to_string())
        .filter(|parent| !parent.is_empty())
}

/// The visible rows between `anchor` and `target`, inclusive, in display order.
///
/// Shift+click keeps the anchor (the last plain/ctrl click) so a second
/// Shift+click extends or shrinks the same range; a target or anchor that is
/// not visible falls back to the target alone.
pub fn range_between(visible: &[String], anchor: Option<&str>, target: &str) -> Vec<String> {
    let Some(target_index) = visible.iter().position(|path| path == target) else {
        return vec![target.to_string()];
    };
    let anchor_index = anchor
        .and_then(|anchor| visible.iter().position(|path| path == anchor))
        .unwrap_or(target_index);
    let (first, last) = if anchor_index <= target_index {
        (anchor_index, target_index)
    } else {
        (target_index, anchor_index)
    };
    visible[first..=last].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::tree::SignalRow;

    fn signal(path: &str) -> SignalRow {
        SignalRow {
            name: path.rsplit('.').next().unwrap_or(path).to_string(),
            path: path.to_string(),
            width: 8,
        }
    }

    fn scope(name: &str, path: &str, signals: &[&str], children: Vec<ModuleRow>) -> ModuleRow {
        ModuleRow {
            name: name.to_string(),
            path: path.to_string(),
            signals: signals.iter().map(|path| signal(path)).collect(),
            children,
        }
    }

    /// top { clk, rst, u_cpu { pc, u_leaf { x } }, g[0] { u_leaf { y } } }
    fn tree() -> SidebarTree {
        SidebarTree::new(scope(
            "",
            "",
            &[],
            vec![scope(
                "top",
                "top",
                &["top.clk", "top.rst"],
                vec![
                    scope(
                        "u_cpu",
                        "top.u_cpu",
                        &["top.u_cpu.pc"],
                        vec![scope(
                            "u_leaf",
                            "top.u_cpu.u_leaf",
                            &["top.u_cpu.u_leaf.x"],
                            vec![],
                        )],
                    ),
                    scope(
                        "g[0]",
                        "top.g[0]",
                        &[],
                        vec![scope(
                            "u_leaf",
                            "top.g[0].u_leaf",
                            &["top.g[0].u_leaf.y"],
                            vec![],
                        )],
                    ),
                ],
            )],
        ))
    }

    #[test]
    fn a_scope_lists_its_signals_and_the_whole_subtree() {
        let tree = tree();
        assert_eq!(
            signals_under(&tree, "top"),
            vec![
                "top.clk",
                "top.rst",
                "top.u_cpu.pc",
                "top.u_cpu.u_leaf.x",
                "top.g[0].u_leaf.y"
            ]
        );
        // a generated instance keeps its fully expanded path
        assert_eq!(
            signals_under(&tree, "top.g[0].u_leaf"),
            vec!["top.g[0].u_leaf.y"]
        );
        assert!(signals_under(&tree, "top.gone").is_empty());
    }

    #[test]
    fn several_scopes_are_merged_without_duplicates() {
        let tree = tree();
        let merged = signals_under_many(
            &tree,
            &[
                "top".to_string(),
                "top.u_cpu".to_string(),
                "top".to_string(),
            ],
        );
        assert_eq!(
            merged,
            vec![
                "top.clk",
                "top.rst",
                "top.u_cpu.pc",
                "top.u_cpu.u_leaf.x",
                "top.g[0].u_leaf.y"
            ]
        );
    }

    #[test]
    fn a_signal_path_resolves_to_its_parent_scope() {
        assert_eq!(parent_scope("top.u_cpu.pc").as_deref(), Some("top.u_cpu"));
        assert_eq!(parent_scope("top.clk").as_deref(), Some("top"));
        // a top-level name has no scope to select
        assert_eq!(parent_scope("clk"), None);
        assert_eq!(parent_scope(""), None);
    }

    #[test]
    fn the_shift_range_keeps_the_visible_order_and_falls_back() {
        let visible: Vec<String> = ["top", "top.u_cpu", "top.u_cpu.u_leaf", "top.g[0]"]
            .iter()
            .map(|path| path.to_string())
            .collect();
        assert_eq!(
            range_between(&visible, Some("top.u_cpu"), "top.g[0]"),
            vec!["top.u_cpu", "top.u_cpu.u_leaf", "top.g[0]"]
        );
        // upwards is the same range
        assert_eq!(
            range_between(&visible, Some("top.g[0]"), "top.u_cpu"),
            vec!["top.u_cpu", "top.u_cpu.u_leaf", "top.g[0]"]
        );
        // no anchor / invisible rows degrade to the target alone
        assert_eq!(
            range_between(&visible, None, "top.u_cpu"),
            vec!["top.u_cpu"]
        );
        assert_eq!(
            range_between(&visible, Some("gone"), "top.u_cpu"),
            vec!["top.u_cpu"]
        );
        assert_eq!(range_between(&visible, Some("top"), "gone"), vec!["gone"]);
    }
}
