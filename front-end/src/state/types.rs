//! Plain data owned by the store: the layout and the status line.
//!
//! Everything here is `Copy`/`Clone` + `PartialEq` and has no behaviour beyond
//! clamping, so it can be compared by value in a subscription and tested on the
//! host target (no `web_sys`).

/// Width of the sidebar when nothing else was chosen.
pub const SIDEBAR_DEFAULT: f64 = 280.0;
/// The sidebar never gets narrower than this.
pub const SIDEBAR_MIN: f64 = 180.0;

/// The editor ratio is clamped to this range on every write
/// (`docs/state-store.md` §3.4).
pub const EDITOR_RATIO_MIN: f32 = 0.15;
pub const EDITOR_RATIO_MAX: f32 = 0.75;

/// The window layout, i.e. the two CSS variables the panes are sized from plus
/// the sidebar drawer state.
///
/// `editor_ratio` is the *stored* value; the pixel height is derived from the
/// measured main-pane height by `App` (never stored, see §3.2 of the design).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub sidebar_w: f64,
    pub editor_ratio: f32,
    pub drawer_open: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_w: SIDEBAR_DEFAULT,
            editor_ratio: 0.5,
            drawer_open: true,
        }
    }
}

impl Layout {
    /// The same layout with every field brought back into its documented range.
    pub fn clamped(self) -> Self {
        Self {
            sidebar_w: clamp_sidebar(self.sidebar_w, f64::INFINITY),
            editor_ratio: clamp_ratio(self.editor_ratio),
            drawer_open: self.drawer_open,
        }
    }
}

/// Clamp the sidebar width to `[SIDEBAR_MIN, max]`.
///
/// `max` comes from the caller (`App` measures the window) because the reducer
/// stays pure; a non-finite or too small `max` degrades to `SIDEBAR_MIN`.
pub fn clamp_sidebar(width: f64, max: f64) -> f64 {
    let max = if max.is_finite() { max.max(SIDEBAR_MIN) } else { f64::INFINITY };
    if width.is_nan() { SIDEBAR_DEFAULT.min(max) } else { width.max(SIDEBAR_MIN).min(max) }
}

/// Clamp the editor ratio to `[EDITOR_RATIO_MIN, EDITOR_RATIO_MAX]`.
pub fn clamp_ratio(ratio: f32) -> f32 {
    if ratio.is_finite() {
        ratio.max(EDITOR_RATIO_MIN).min(EDITOR_RATIO_MAX)
    } else {
        0.5
    }
}

/// Whether a status line is neutral information or something the user must act
/// on; the top bar colours them differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Info,
    Error,
}

/// One line of feedback in the top bar.
///
/// Errors are phrased as "what failed + what to do about it"; see the R2 polish
/// list (the user asked for messages a human can act on, not console lines).
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub text: String,
    pub tone: StatusTone,
}

impl Status {
    pub fn info(text: impl Into<String>) -> Self {
        Self { text: text.into(), tone: StatusTone::Info }
    }

    pub fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), tone: StatusTone::Error }
    }

    pub fn is_error(&self) -> bool {
        self.tone == StatusTone::Error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_is_clamped_into_range() {
        assert_eq!(clamp_sidebar(10.0, 640.0), SIDEBAR_MIN);
        assert_eq!(clamp_sidebar(300.0, 640.0), 300.0);
        assert_eq!(clamp_sidebar(900.0, 640.0), 640.0);
    }

    #[test]
    fn sidebar_clamp_degrades_gracefully() {
        // a max below the minimum still yields a usable (minimum) width
        assert_eq!(clamp_sidebar(500.0, 40.0), SIDEBAR_MIN);
        // a non-finite width falls back to the default instead of NaN
        assert_eq!(clamp_sidebar(f64::NAN, 640.0), SIDEBAR_DEFAULT);
        assert_eq!(clamp_sidebar(f64::INFINITY, 640.0), 640.0);
    }

    #[test]
    fn editor_ratio_is_clamped_into_range() {
        assert_eq!(clamp_ratio(0.01), EDITOR_RATIO_MIN);
        assert_eq!(clamp_ratio(0.5), 0.5);
        assert_eq!(clamp_ratio(0.99), EDITOR_RATIO_MAX);
        assert_eq!(clamp_ratio(f32::NAN), 0.5);
    }

    #[test]
    fn layout_default_matches_the_design() {
        let layout = Layout::default();
        assert_eq!(layout.sidebar_w, SIDEBAR_DEFAULT);
        assert_eq!(layout.editor_ratio, 0.5);
        assert!(layout.drawer_open);
        assert_eq!(layout.clamped(), layout);
    }

    #[test]
    fn status_keeps_its_tone() {
        assert!(!Status::info("parsed top.sv").is_error());
        assert!(Status::error("top.sv: not utf-8").is_error());
    }
}
