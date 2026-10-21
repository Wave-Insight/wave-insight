//! Two way navigation between the waveform, the source pane and the design
//! database.
//!
//! The modules here are glue, not logic: [`DesignState`] owns the
//! [`wave_insight_lib::design::DesignDb`] and the elaborated design and answers
//! the four questions the UI has ("where is this declared", "who drives it",
//! "what is under the cursor", "which signals match this word"), while
//! [`SignalInspector`] and [`PathPicker`] only render those answers.
//!
//! Everything the components exchange is cheap to clone and compare: the
//! database is behind a [`SharedDesign`] identity handle, and the opened dump
//! lives in the store as a [`crate::state::DumpHandle`] (identity comparing), so
//! a re-render never deep compares a design or a waveform.

pub use design_state::{DesignState, SignalDetails, SourceRow};
pub use inspector::SignalInspector;
pub use picker::PathPicker;
pub use tree::{SidebarTree, SignalRow};

mod design_state;
mod inspector;
mod picker;
mod tree;

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use wave_insight_lib::data_struct::ShowType;
use wave_insight_lib::waveform::{WaveValue, Waveform};

/// A request to bring one source position into view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goto {
    /// File name exactly as it was handed to `DesignDb::add_source`, which is
    /// also the label of the editor tab.
    pub file: String,
    /// 1 based line.
    pub line: u32,
    /// 1 based column; `0` means "no column, just the line".
    pub col: u32,
    /// Bumped for every request, so clicking the same declaration twice still
    /// scrolls the editor.
    pub token: u32,
}

/// A click on an identifier inside the source editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceClick {
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// The identifier the user clicked, used for the fallback lookup when the
    /// elaborated design has nothing at that exact position.
    pub word: String,
}

/// Shared handle on the [`DesignState`].
///
/// Compares by identity, not by content: re-rendering the app must not walk a
/// whole design database, and the version counter is what tells the inspector
/// that the content behind the handle changed.
#[derive(Clone)]
pub struct SharedDesign(Rc<RefCell<DesignState>>);

impl SharedDesign {
    pub fn new(state: DesignState) -> Self {
        Self(Rc::new(RefCell::new(state)))
    }

    pub fn borrow(&self) -> Ref<'_, DesignState> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, DesignState> {
        self.0.borrow_mut()
    }
}

impl PartialEq for SharedDesign {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for SharedDesign {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.0.borrow();
        f.debug_struct("SharedDesign")
            .field("version", &state.version())
            .field("top", &state.top())
            .finish()
    }
}

impl Default for SharedDesign {
    fn default() -> Self {
        Self::new(DesignState::new())
    }
}

/// Text of one waveform value, formatted with a row's base format.
///
/// Used by the displays outside the wave canvas (the editor inlay hints); the
/// canvas has its own formatter for the same values, because it also draws them.
pub fn value_text(value: &WaveValue, width: usize, show: &ShowType) -> Option<String> {
    match value {
        WaveValue::Real(number) => Some(format!("{number}")),
        WaveValue::String(text) => Some(text.clone()),
        _ => value.render(width, show),
    }
}

/// Every dump signal a hierarchical path can mean, most specific first.
///
/// This is the waveform side of the mapping [`DesignState::resolve_candidates`]
/// does for the design: rows added from the sidebar carry *dump* paths, rows added
/// from the sources carry *design* paths, and a real design and a real dump can
/// spell the root scope differently (`tb_top.u_cpu.pc` versus `top.u_cpu.pc`).
pub fn match_dump_candidates(waveform: &Waveform, path: &str) -> Vec<String> {
    resolve_candidates(
        path,
        waveform.signals().iter(),
        |info| info.path(),
        |info| info.name(),
    )
    .into_iter()
    .map(|info| info.path().to_string())
    .collect()
}

/// Ordered path resolution shared by the design and the dump.
///
/// The two hierarchies describe the same signals but can spell their root scope
/// differently, so a path from one side is resolved against the other in this
/// order:
///
/// 1. the exact hierarchical path;
/// 2. the same depth with the root scope replaced (`tb_top.clk` ↔ `top.clk`);
/// 3. a candidate with extra leading scopes (`tb_top.dut.clk` for `dut.clk`);
/// 4. a candidate whose tail equals the query tail after dropping the query's
///    leading scopes (`tb_top.u_cpu.pc` for `top.u_cpu.pc`);
/// 5. the unique local name.
///
/// The first step that matches wins, even when it matches several items: callers
/// must not silently pick one, because a wrong-but-plausible signal is worse than
/// a message. They surface the candidates instead.
pub(crate) fn resolve_candidates<'a, T>(
    path: &str,
    items: impl Iterator<Item = &'a T>,
    path_of: impl Fn(&T) -> &str,
    local_of: impl Fn(&T) -> &str,
) -> Vec<&'a T> {
    let items: Vec<&T> = items.collect();
    let query: Vec<&str> = path.split('.').filter(|part| !part.is_empty()).collect();
    if query.is_empty() {
        return Vec::new();
    }

    let exact: Vec<&T> = items
        .iter()
        .copied()
        .filter(|item| path_of(item) == path)
        .collect();
    if !exact.is_empty() {
        return exact;
    }

    // the root scope is spelled differently: compare everything below it at the
    // same depth (`tb_top.clk` ↔ `top.clk`)
    if query.len() > 1 {
        let query_tail = query[1..].join(".");
        let same_depth: Vec<&T> = items
            .iter()
            .copied()
            .filter(|item| {
                let parts: Vec<&str> = path_of(item).split('.').filter(|part| !part.is_empty()).collect();
                parts.len() == query.len() && parts[1..].join(".") == query_tail
            })
            .collect();
        if !same_depth.is_empty() {
            return same_depth;
        }
    }

    // the candidate has extra leading scopes (a testbench level above the
    // elaborated top, or the other way round)
    let deeper: Vec<&T> = items
        .iter()
        .copied()
        .filter(|item| {
            let candidate = path_of(item);
            candidate == path || candidate.ends_with(&format!(".{path}"))
        })
        .collect();
    if !deeper.is_empty() {
        return deeper;
    }

    // drop the query's leading scopes and look for a matching tail
    for skip in 1..query.len() {
        let suffix = query[skip..].join(".");
        let tail: Vec<&T> = items
            .iter()
            .copied()
            .filter(|item| {
                let candidate = path_of(item);
                candidate == suffix || candidate.ends_with(&format!(".{suffix}"))
            })
            .collect();
        if !tail.is_empty() {
            return tail;
        }
    }

    let word = query[query.len() - 1];
    items
        .iter()
        .copied()
        .filter(|item| local_of(item) == word)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::resolve_candidates;

    /// Resolve a path against path strings; returns the matching paths.
    fn resolve<'a>(path: &str, paths: &'a [&'a str]) -> Vec<&'a str> {
        resolve_candidates(
            path,
            paths.iter(),
            |item| *item,
            |item| item.rsplit('.').next().unwrap_or(item),
        )
        .into_iter()
        .copied()
        .collect()
    }

    #[test]
    fn resolution_order_start_with_the_exact_path() {
        let paths = ["top.clk", "top.u_c.clk"];
        assert_eq!(resolve("top.clk", &paths), vec!["top.clk"]);
        assert_eq!(resolve("top.u_c.clk", &paths), vec!["top.u_c.clk"]);
        assert!(resolve("top.missing", &paths).is_empty());
    }

    #[test]
    fn a_differently_spelled_root_resolves_by_depth() {
        let paths = ["top.clk", "top.u_c.clk"];
        // `tb_top` is the dump's root name for the same elaborated `top`
        assert_eq!(resolve("tb_top.clk", &paths), vec!["top.clk"]);
        assert_eq!(resolve("tb_top.u_c.clk", &paths), vec!["top.u_c.clk"]);
    }

    #[test]
    fn extra_leading_scopes_on_the_candidate_resolve() {
        // the dump can have a wrapper scope above the elaborated module
        let paths = ["top.dut.clk"];
        assert_eq!(resolve("dut.clk", &paths), vec!["top.dut.clk"]);
        // and a deeper dump path resolves against a shallower design path
        let paths = ["dut.clk"];
        assert_eq!(resolve("tb.dut.clk", &paths), vec!["dut.clk"]);
    }

    #[test]
    fn ambiguous_names_are_returned_not_guessed() {
        let paths = ["top.clk", "top.u_c.clk"];
        // a bare local name matches both instances: the caller must ask, and
        // resolve_path (the single-candidate view) therefore refuses
        assert_eq!(resolve("clk", &paths), vec!["top.clk", "top.u_c.clk"]);
    }
}
