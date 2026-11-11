//! The signal list's multi-selection (Wave 19: Ctrl/Shift pick, explicit add).
//!
//! The Modules tree has had this contract since Wave 12: a plain click selects
//! one row, Ctrl/Cmd-click toggles it, Shift-click takes the range from the
//! anchor, and "Add selected (N)" adds the whole selection in one batch. The
//! signal list follows the same rules; the state is pure so the host tests can
//! check the selection math without a browser.
//!
//! The selection is kept over the *filtered* order, i.e. the list the user is
//! looking at: Shift ranges follow the logical filtered list (not only the rows
//! the virtualized DOM has drawn — the module tree ranges over its whole open
//! tree the same way). A plain click resets the selection to one row and makes
//! it the anchor; Ctrl/Cmd toggles without touching the rest; Shift keeps the
//! anchor so a second Shift-click re-ranges from it. When the filter changes,
//! paths that are no longer listed are pruned instead of being added invisibly.

/// What a signal-row click means for the list's own multi-selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalSelect {
    /// Plain click: the selection becomes this row (and it becomes the anchor).
    Reset(String),
    /// Ctrl/Cmd-click: toggle this row in the selection.
    Toggle(String),
    /// Shift-click: the filtered range from the anchor to this row.
    Range(String),
}

impl SignalSelect {
    /// Classify a row click. Ctrl/Cmd wins over Shift, exactly like the module
    /// tree's `Msg::Select`.
    pub fn from_click(path: &str, ctrl: bool, shift: bool) -> Self {
        if ctrl {
            SignalSelect::Toggle(path.to_string())
        } else if shift {
            SignalSelect::Range(path.to_string())
        } else {
            SignalSelect::Reset(path.to_string())
        }
    }
}

/// The multi-selection of one signal list (`SignalStruct` owns exactly one).
///
/// `paths` is in selection order (the order "Add selected" uses); `anchor` is
/// the row the last plain/Ctrl click landed on, the fixed end of a Shift range.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignalSelection {
    paths: Vec<String>,
    anchor: Option<String>,
}

impl SignalSelection {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    /// The selected paths, in selection order (the batch-add input).
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// Drop the whole selection. Returns whether anything changed, so the
    /// caller can skip a render for a no-op.
    pub fn clear(&mut self) -> bool {
        if self.paths.is_empty() && self.anchor.is_none() {
            return false;
        }
        self.paths.clear();
        self.anchor = None;
        true
    }

    /// Plain click: the selection becomes this row and the row is the anchor.
    pub fn reset(&mut self, path: &str) -> bool {
        if self.paths.len() == 1 && self.paths[0] == path {
            // already the only selected row: only the anchor can move, and it
            // did not (a no-op click must not re-render a 20k-row list)
            return false;
        }
        self.paths.clear();
        self.paths.push(path.to_string());
        self.anchor = Some(path.to_string());
        true
    }

    /// Ctrl/Cmd-click: toggle the row; it becomes the anchor.
    pub fn toggle(&mut self, path: &str) -> bool {
        if let Some(index) = self.paths.iter().position(|selected| selected == path) {
            self.paths.remove(index);
        } else {
            self.paths.push(path.to_string());
        }
        self.anchor = Some(path.to_string());
        true
    }

    /// Shift-click: every visible row between the anchor and `target`, in list
    /// order. Mirrors `crate::state::bulk::range_between` (the module tree's
    /// version) over the filtered signal order: an invisible target degrades to
    /// the target alone, an invisible anchor to the target alone as well.
    pub fn range(&mut self, visible: &[&str], target: &str) -> bool {
        let Some(target_index) = visible.iter().position(|path| *path == target) else {
            // the row left the filtered list between the click and the update
            self.anchor = Some(target.to_string());
            return self.set_paths(vec![target.to_string()]);
        };
        let anchor_index = self
            .anchor
            .as_deref()
            .and_then(|anchor| visible.iter().position(|path| *path == anchor))
            .unwrap_or(target_index);
        self.anchor = Some(visible[anchor_index].to_string());
        let (first, last) = if anchor_index <= target_index {
            (anchor_index, target_index)
        } else {
            (target_index, anchor_index)
        };
        let paths = visible[first..=last]
            .iter()
            .map(|path| (*path).to_string())
            .collect();
        self.set_paths(paths)
    }

    /// The header's "Select all": every row of the filtered list, in list order.
    /// The first row becomes the anchor so a following Shift-click ranges from
    /// the top (the module tree's "Select all" does the same).
    pub fn select_all(&mut self, visible: &[&str]) -> bool {
        if visible.is_empty() {
            return false;
        }
        self.anchor = Some(visible[0].to_string());
        self.set_paths(visible.iter().map(|path| (*path).to_string()).collect())
    }

    /// Keep only the selected paths still present in the filtered order; the
    /// anchor is dropped when its row is gone. Returns whether anything changed
    /// (a filter keystroke that hides nothing must not re-render the rows).
    ///
    /// A set lookup keeps "Select all 20k rows, then type a filter" linear: a
    /// per-selection `visible.contains` scan would be 20k x 20k comparisons per
    /// keystroke.
    pub fn prune(&mut self, visible: &[&str]) -> bool {
        let present: std::collections::HashSet<&str> = visible.iter().copied().collect();
        let anchor_dropped = self
            .anchor
            .as_deref()
            .is_some_and(|anchor| !present.contains(anchor));
        if anchor_dropped {
            self.anchor = None;
        }
        let before = self.paths.len();
        self.paths
            .retain(|selected| present.contains(selected.as_str()));
        anchor_dropped || self.paths.len() != before
    }

    /// Replace the selected paths, reporting whether they really changed.
    fn set_paths(&mut self, paths: Vec<String>) -> bool {
        if self.paths == paths {
            return false;
        }
        self.paths = paths;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_selection(paths: &[&str]) -> SignalSelection {
        SignalSelection {
            paths: paths.iter().map(|path| path.to_string()).collect(),
            anchor: paths.last().map(|path| path.to_string()),
        }
    }

    fn visible<'a>(paths: &'a [&'a str]) -> Vec<&'a str> {
        paths.to_vec()
    }

    #[test]
    fn a_plain_click_resets_the_selection_and_moves_the_anchor() {
        let mut selection = with_selection(&["top.a", "top.b"]);
        assert!(selection.reset("top.c"));
        assert_eq!(selection.paths(), ["top.c"]);
        assert_eq!(selection.anchor.as_deref(), Some("top.c"));
        // clicking the only selected row again is a no-op (no render)
        assert!(!selection.reset("top.c"));
        assert!(selection.reset("top.d"));
        assert_eq!(selection.paths(), ["top.d"]);
    }

    #[test]
    fn ctrl_toggles_and_moves_the_anchor() {
        let mut selection = SignalSelection::default();
        assert!(selection.toggle("top.a"));
        assert!(selection.toggle("top.b"));
        assert_eq!(selection.paths(), ["top.a", "top.b"]);
        assert_eq!(selection.anchor.as_deref(), Some("top.b"));
        assert!(selection.toggle("top.a"));
        assert_eq!(selection.paths(), ["top.b"]);
        assert_eq!(selection.anchor.as_deref(), Some("top.a"));
    }

    #[test]
    fn shift_takes_the_visible_range_from_the_anchor() {
        let rows = visible(&["top.a", "top.b", "top.c", "top.d", "top.e"]);
        let mut selection = SignalSelection::default();
        selection.reset("top.b");
        assert!(selection.range(&rows, "top.d"));
        assert_eq!(selection.paths(), ["top.b", "top.c", "top.d"]);
        // the anchor stayed on b: a second Shift-click shrinks the same range
        assert!(selection.range(&rows, "top.c"));
        assert_eq!(selection.paths(), ["top.b", "top.c"]);
        // upwards is the same range, in list order
        assert!(selection.range(&rows, "top.a"));
        assert_eq!(selection.paths(), ["top.a", "top.b"]);
        assert_eq!(selection.anchor.as_deref(), Some("top.b"));
    }

    #[test]
    fn shift_without_a_visible_anchor_falls_back_to_the_target() {
        let rows = visible(&["top.a", "top.b", "top.c"]);
        let mut selection = SignalSelection::default();
        assert!(selection.range(&rows, "top.b"));
        assert_eq!(selection.paths(), ["top.b"]);
        // the target is not in the filtered list at all: it stays the selection
        assert!(selection.range(&rows, "top.gone"));
        assert_eq!(selection.paths(), ["top.gone"]);
        // an invisible anchor degrades to the target alone
        let mut selection = with_selection(&["top.gone"]);
        assert!(selection.range(&rows, "top.c"));
        assert_eq!(selection.paths(), ["top.c"]);
    }

    #[test]
    fn select_all_takes_the_filtered_order_and_anchors_on_the_first_row() {
        let rows = visible(&["top.c", "top.d"]);
        let mut selection = SignalSelection::default();
        assert!(selection.select_all(&rows));
        assert_eq!(selection.paths(), ["top.c", "top.d"]);
        assert_eq!(selection.anchor.as_deref(), Some("top.c"));
        assert_eq!(selection.len(), 2);
        // selecting the same rows again changes nothing
        assert!(!selection.select_all(&rows));
        // an empty list cannot be selected
        let mut empty = SignalSelection::default();
        assert!(!empty.select_all(&[]));
    }

    #[test]
    fn a_filter_change_prunes_the_selection_and_drops_a_hidden_anchor() {
        let rows = visible(&["top.c", "top.d"]);
        let mut selection = with_selection(&["top.a", "top.c", "top.e"]);
        assert!(selection.prune(&rows));
        assert_eq!(selection.paths(), ["top.c"]);
        // the anchor was top.e (hidden): it is dropped, so a Shift-click falls
        // back to the target instead of ranging from a row the user cannot see
        assert_eq!(selection.anchor, None);
        assert!(selection.range(&rows, "top.d"));
        assert_eq!(selection.paths(), ["top.d"]);
        // a filter that hides nothing changes nothing (no re-render)
        assert!(!selection.prune(&rows));
    }

    #[test]
    fn clear_empty_selection_is_a_no_op() {
        let mut selection = SignalSelection::default();
        assert!(selection.is_empty());
        assert_eq!(selection.len(), 0);
        assert!(!selection.clear());
        selection.reset("top.a");
        assert!(!selection.is_empty());
        assert!(selection.clear());
        assert!(selection.paths().is_empty());
        assert_eq!(selection.anchor, None);
    }

    #[test]
    fn a_click_is_classified_like_the_module_tree() {
        assert_eq!(
            SignalSelect::from_click("top.a", false, false),
            SignalSelect::Reset("top.a".to_string())
        );
        assert_eq!(
            SignalSelect::from_click("top.a", true, false),
            SignalSelect::Toggle("top.a".to_string())
        );
        assert_eq!(
            SignalSelect::from_click("top.a", false, true),
            SignalSelect::Range("top.a".to_string())
        );
        // Ctrl/Cmd wins when both modifiers are held
        assert_eq!(
            SignalSelect::from_click("top.a", true, true),
            SignalSelect::Toggle("top.a".to_string())
        );
    }
}
