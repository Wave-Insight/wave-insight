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

use wasm_bindgen::JsCast;

use wave_insight_lib::waveform::ShowType;
use wave_insight_lib::waveform::{WaveValue, Waveform};

// R2 step 3 moved the jump request into the store (`State.nav.goto`); the type
// is state data now and is re-exported here so the panes and the language server
// keep their old spelling.
pub use crate::state::types::Goto;

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

/// How one row's values are printed: the base format plus the fixed-point
/// scale.
///
/// The row's `Settings` owns both, but the formatter only needs these two
/// fields; keeping them in one value lets the value column, the canvas band
/// labels and the editor inlay hints share one code path (and therefore never
/// disagree).
#[derive(Clone, Debug, PartialEq)]
pub struct ValueFormat {
    /// Base number format (radix, signedness).
    pub show: ShowType,
    /// `Some(fraction_bits)`: print `value / 2^fraction_bits` with one
    /// decimal instead of the integer base format.
    pub fixed_point: Option<u32>,
}

impl ValueFormat {
    /// The base format alone, i.e. fixed-point display off.
    pub fn base(show: ShowType) -> Self {
        Self {
            show,
            fixed_point: None,
        }
    }

    /// The format a row's `Settings` asks for.
    pub fn from_setting(setting: &crate::state::Settings) -> Self {
        Self {
            show: setting.show_type.clone(),
            fixed_point: setting.fixed_active.then_some(setting.fixed_point),
        }
    }
}

/// Text of one waveform value, formatted with a row's base format (or its
/// fixed-point scale, when that is active).
///
/// Multi-bit vectors carry their radix in Verilog style (`'h2a`, `'d42`,
/// `'b1010`, `'o52`, `'sd-3`), because a bare `2a` reads as decimal at a glance.
/// A 1-bit value keeps its bare `0`/`1`/`x`/`z`, and reals/strings are printed
/// as they are: a prefix there would say nothing true.
///
/// Used by the editor inlay hints, the value column and the canvas band labels
/// (the canvas calls this too, so all three displays always agree).
pub fn value_text(value: &WaveValue, width: usize, format: &ValueFormat) -> Option<String> {
    if let Some(fraction_bits) = format.fixed_point {
        // an unknown bit has no number to scale; fall through to the base
        // format rather than hiding the value
        if let Some(text) = fixed_point_text(value, &format.show, fraction_bits) {
            return Some(text);
        }
    }
    let show = &format.show;
    let text = match value {
        WaveValue::Real(number) => return Some(format!("{number}")),
        WaveValue::String(text) => return Some(text.clone()),
        _ => value.render(width, show)?,
    };
    if width <= 1 {
        return Some(text);
    }
    let radix = match show {
        ShowType::Hex => "'h",
        ShowType::UInt => "'d",
        ShowType::SInt => "'sd",
        ShowType::Oct => "'o",
        ShowType::Bin => "'b",
        // ASCII (and any future non numeric presentation) stays bare text
        ShowType::Ascii => "",
    };
    Some(format!("{radix}{text}"))
}

/// Widest fixed-point exponent the formatter accepts: a larger one has no
/// representable two's-complement integer on the other side of the scale.
pub const MAX_FIXED_POINT: u32 = 63;

/// Fixed-point text: `value / 2^fraction_bits`, one decimal, rounded.
///
/// The row's base format decides how the raw bits are read, so `Signed` keeps
/// its two's-complement interpretation (a Q4.4 `0xF0` reads `-1.0`, not
/// `15.0`). A real is scaled the same way; a string, an event, or a vector with
/// an `x`/`z` bit has no number and returns `None` (the caller then prints the
/// base format). `fraction_bits` is clamped to 63, the widest scale whose
/// two's-complement integer still fits the `i64` the signed path uses.
pub fn fixed_point_text(value: &WaveValue, show: &ShowType, fraction_bits: u32) -> Option<String> {
    let number = match value {
        WaveValue::Real(number) => *number,
        other => {
            let bits = other.as_bits()?;
            let raw = bits.as_u64()?;
            let width = bits.width();
            if *show == ShowType::SInt && (1..=63).contains(&width) {
                // two's complement, so a signed bus scales around zero
                let modulus = 1i64 << width;
                let signed = if raw & (1u64 << (width - 1)) != 0 {
                    raw as i64 - modulus
                } else {
                    raw as i64
                };
                signed as f64
            } else {
                raw as f64
            }
        }
    };
    if !number.is_finite() {
        return None;
    }
    let scale = 2f64.powi(fraction_bits.min(MAX_FIXED_POINT) as i32);
    let scaled = number / scale;
    // one decimal, and never "-0.0" for a value that rounds to zero
    let rounded = (scaled * 10.0).round() / 10.0;
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    Some(format!("{rounded:.1}"))
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
/// 2. the same depth with the root scope replaced (`tb_top.clk` → `top.clk`);
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
    // same depth (`tb_top.clk` → `top.clk`)
    if query.len() > 1 {
        let query_tail = query[1..].join(".");
        let same_depth: Vec<&T> = items
            .iter()
            .copied()
            .filter(|item| {
                let parts: Vec<&str> = path_of(item)
                    .split('.')
                    .filter(|part| !part.is_empty())
                    .collect();
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

    // -- fixed point (settings dialog: "Fixed point" active + exponent) ------

    use wave_insight_lib::waveform::{BitValue, Bits};

    /// A bit vector with the given raw value, `width` bits wide.
    fn vector(raw: u64, width: usize) -> super::WaveValue {
        super::WaveValue::Bits(Bits::new_msb_first(
            (0..width)
                .rev()
                .map(|bit| {
                    if raw & (1u64 << bit) != 0 {
                        BitValue::One
                    } else {
                        BitValue::Zero
                    }
                })
                .collect(),
        ))
    }

    #[test]
    fn fixed_point_scales_by_two_to_the_exponent() {
        use super::{fixed_point_text, ShowType};
        // 0x10 = 16, Q12.4 -> 1.0
        assert_eq!(
            fixed_point_text(&vector(0x10, 8), &ShowType::Hex, 4).as_deref(),
            Some("1.0")
        );
        // 8 / 2^4 = 0.5
        assert_eq!(
            fixed_point_text(&vector(8, 8), &ShowType::UInt, 4).as_deref(),
            Some("0.5")
        );
        // 0 / 2^4 = 0.0
        assert_eq!(
            fixed_point_text(&vector(0, 8), &ShowType::Hex, 4).as_deref(),
            Some("0.0")
        );
        // 0xFF as a signed byte is -1: -1/16 rounds to -0.1
        assert_eq!(
            fixed_point_text(&vector(0xFF, 8), &ShowType::SInt, 4).as_deref(),
            Some("-0.1")
        );
        // ... and a negative that rounds to zero prints "0.0", never "-0.0"
        assert_eq!(
            fixed_point_text(&vector(0xFF, 8), &ShowType::SInt, 8).as_deref(),
            Some("0.0")
        );
        // 3 / 2^3 = 0.375 -> one decimal, rounded
        assert_eq!(
            fixed_point_text(&vector(3, 8), &ShowType::UInt, 3).as_deref(),
            Some("0.4")
        );
        // a zero exponent is the integer value itself
        assert_eq!(
            fixed_point_text(&vector(42, 8), &ShowType::UInt, 0).as_deref(),
            Some("42.0")
        );
    }

    #[test]
    fn fixed_point_is_signed_when_the_base_format_is_signed() {
        use super::{fixed_point_text, ShowType};
        // 0xF0 as an unsigned byte is 240, as a signed byte it is -16:
        // Q4.4 -> 15.0 vs -1.0. The base format decides, like everywhere else.
        assert_eq!(
            fixed_point_text(&vector(0xF0, 8), &ShowType::UInt, 4).as_deref(),
            Some("15.0")
        );
        assert_eq!(
            fixed_point_text(&vector(0xF0, 8), &ShowType::SInt, 4).as_deref(),
            Some("-1.0")
        );
        // 0x80 is -128 signed: Q8.8 -> -0.5
        assert_eq!(
            fixed_point_text(&vector(0x80, 8), &ShowType::SInt, 8).as_deref(),
            Some("-0.5")
        );
    }

    #[test]
    fn fixed_point_has_no_number_for_unknown_bits_or_strings() {
        use super::{fixed_point_text, ShowType};
        let unknown =
            super::WaveValue::Bits(Bits::new_msb_first(vec![BitValue::X, BitValue::Zero]));
        assert!(fixed_point_text(&unknown, &ShowType::Hex, 4).is_none());
        let text = super::WaveValue::String("hello".into());
        assert!(fixed_point_text(&text, &ShowType::Hex, 4).is_none());
        // ... and value_text then falls back to the base format instead of
        // hiding the value
        let format = super::ValueFormat {
            show: ShowType::Hex,
            fixed_point: Some(4),
        };
        assert!(super::value_text(&unknown, 2, &format).is_some());
    }

    #[test]
    fn value_text_only_scales_when_fixed_point_is_active() {
        use super::{value_text, ShowType, ValueFormat};
        let raw = vector(0x10, 8);
        // off: the plain radix text, unchanged from before this feature
        assert_eq!(
            value_text(&raw, 8, &ValueFormat::base(ShowType::Hex)).as_deref(),
            Some("'h10")
        );
        // on: value / 2^4, one decimal
        let fixed = ValueFormat {
            show: ShowType::Hex,
            fixed_point: Some(4),
        };
        assert_eq!(value_text(&raw, 8, &fixed).as_deref(), Some("1.0"));
    }

    #[test]
    fn a_setting_maps_to_the_format_the_renderer_uses() {
        use super::{ShowType, ValueFormat};
        let mut setting = crate::state::Settings::new();
        setting.show_type = ShowType::SInt;
        // fixed point off: the base format only
        assert_eq!(
            ValueFormat::from_setting(&setting),
            ValueFormat::base(ShowType::SInt)
        );
        setting.fixed_active = true;
        setting.fixed_point = 8;
        assert_eq!(
            ValueFormat::from_setting(&setting),
            ValueFormat {
                show: ShowType::SInt,
                fixed_point: Some(8)
            }
        );
    }
}

/// What a modal's keydown handler should do with one key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalKey {
    /// Escape: close the dialog.
    Close,
    /// The key was consumed (focus was wrapped inside the dialog).
    Trapped,
    /// Not ours.
    Ignore,
}

/// The keys a modal owns: Escape closes it, Tab stays inside it.
///
/// This is the "simple focus trap" the accessibility pass promises: focus cannot
/// leave the open dialog with Tab and the dialog is dismissible from the
/// keyboard. Only one modal is open at a time (the settings dialog or the path
/// picker), so the trap is the single `.modal` in the document.
///
/// DOM only (it reads `document.activeElement`), so it has no host test; the
/// browser QA run is what exercises it.
pub fn modal_keydown(event: &web_sys::KeyboardEvent) -> ModalKey {
    if event.key() == "Escape" {
        return ModalKey::Close;
    }
    if event.key() != "Tab" {
        return ModalKey::Ignore;
    }
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return ModalKey::Ignore;
    };
    let Ok(Some(modal)) = document.query_selector(".modal") else {
        return ModalKey::Ignore;
    };
    let selector = "button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex='-1'])";
    let Ok(items) = modal.query_selector_all(selector) else {
        return ModalKey::Ignore;
    };
    let count = items.length();
    if count == 0 {
        return ModalKey::Ignore;
    }
    let element = |index: u32| {
        items
            .get(index)
            .and_then(|node| node.dyn_into::<web_sys::Element>().ok())
    };
    let active = document.active_element();
    let first = element(0);
    let last = element(count - 1);
    if event.shift_key() {
        if active == first || active.is_none() {
            if let Some(last) = last {
                event.prevent_default();
                if let Ok(element) = last.dyn_into::<web_sys::HtmlElement>() {
                    let _ = element.focus();
                }
                return ModalKey::Trapped;
            }
        }
    } else if active == last {
        if let Some(first) = first {
            event.prevent_default();
            if let Ok(element) = first.dyn_into::<web_sys::HtmlElement>() {
                let _ = element.focus();
            }
            return ModalKey::Trapped;
        }
    }
    ModalKey::Ignore
}

/// Focus the first control of the open modal, so the keyboard lands inside it.
pub fn focus_first_in_modal() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Ok(Some(modal)) = document.query_selector(".modal") else {
        return;
    };
    let selector = "button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex='-1'])";
    let Ok(items) = modal.query_selector_all(selector) else {
        return;
    };
    if let Some(first) = items
        .get(0)
        .and_then(|node| node.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = first.focus();
    }
}
