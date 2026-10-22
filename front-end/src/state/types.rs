//! Plain data owned by the store: layout, status, the wave rows and the
//! per-row presentation settings.
//!
//! Everything here is `Copy`/`Clone` + `PartialEq` and has no behaviour beyond
//! clamping/formatting, so it can be compared in a subscription and tested on
//! the host target (no `web_sys`). The *decoded values* of a row are not here:
//! they belong to the waveform backend's cache and stay with the renderer.

use std::rc::Rc;

use serde::{Deserialize, Serialize};
use wave_insight_lib::data_struct::ShowType;
use wave_insight_lib::waveform::{TimeRange, Waveform};

/// Identity-comparing handle on the opened dump.
///
/// `Waveform` caches decoded data behind a mutex and has no `PartialEq`, so the
/// store keeps it behind this `Rc` wrapper: two handles are equal when they point
/// at the same file, never by deep comparison (design doc §2.4).
#[derive(Clone)]
pub struct DumpHandle(Rc<Waveform>);

impl DumpHandle {
    pub fn new(waveform: Waveform) -> Self {
        Self(Rc::new(waveform))
    }

    pub fn waveform(&self) -> &Waveform {
        &self.0
    }
}

impl PartialEq for DumpHandle {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for DumpHandle {}

impl std::fmt::Debug for DumpHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DumpHandle")
            .field("format", &self.0.file_format())
            .field("signals", &self.0.signal_count())
            .finish()
    }
}

/// The opened dump, as the store holds it.
///
/// The handle is the source of truth (`State.dump`); the time range is derived
/// from it by the reducer so the ruler and the fit zoom stay cheap reads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dump {
    pub waveform: Option<DumpHandle>,
    pub time_range: Option<TimeRange>,
}

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
            // A little under half: the waveform is the thing that is read, and
            // once the inspector dock opens the two panes share one column. At
            // 0.5 the wave pane kept only ~220 px on a 900 px window, which read
            // as "the waveform is squashed" as soon as the sources were opened.
            editor_ratio: 0.42,
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

/// How one row is rendered: base number format, colour and the fixed/analog
/// readouts. Moved here from `wave_show/settings.rs` in R2 step 2 (B1) so the
/// store owns it; `wave_show::Settings` still names the same type.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Settings {
    pub show_type: ShowType,
    pub color: (u8, u8, u8),

    pub fixed_active: bool,
    pub fixed_point: u32,

    pub analog_active: bool,

    pub load_all: bool,
    pub load_one: i32,
    pub drive_all: bool,
    pub drive_one: i32,
}

impl Settings {
    pub fn new() -> Self {
        Self {
            show_type: ShowType::Hex,
            color: (76, 194, 255),

            fixed_active: false,
            fixed_point: 0,

            analog_active: false,

            load_all: false,
            load_one: -1,
            drive_all: false,
            drive_one: -1,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

/// One row of the wave view, as the store owns it.
///
/// Order, per-row settings and the multi-selection are user state and live here;
/// the decoded values do not (they are the waveform backend's cache, sliced by
/// the renderer).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Hierarchical path; also the identity used to dedupe and to resolve the
    /// signal in the dump.
    pub path: String,
    /// Display name without the `[n:0]` suffix (the renderer adds that from the
    /// resolved width).
    pub name: String,
    /// Width the design declares; the dump's width wins when it has the signal.
    pub width: usize,
    pub setting: Settings,
    pub chosen: bool,
    /// Source locations of the drivers/loads, for the settings dialog.
    pub load: Vec<String>,
    pub drive: Vec<String>,
}

/// A versioned snapshot of the rows.
///
/// Cloning is one `Rc` bump and comparing is one integer, which is what makes a
/// row subscription cheap enough to fire only when the rows really changed.
#[derive(Clone, Debug)]
pub struct RowsSlice {
    pub version: u64,
    pub items: Rc<Vec<Row>>,
}

impl RowsSlice {
    pub fn empty() -> Self {
        Self { version: 0, items: Rc::new(Vec::new()) }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

}

impl PartialEq for RowsSlice {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl Default for RowsSlice {
    fn default() -> Self {
        Self::empty()
    }
}


/// A time step of the waveform backend (non-negative).
pub type Time = u64;

/// Cursors and settings are `i32`; clamp instead of wrapping.
pub fn clamp_i32(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    value.clamp(0.0, i32::MAX as f64) as i32
}

/// A non-negative, saturating `f64 -> Time` conversion.
pub fn clamp_time(value: f64) -> Time {
    if value.is_nan() || value <= 0.0 {
        0
    } else if value >= u64::MAX as f64 {
        u64::MAX
    } else {
        value as u64
    }
}

/// Zoom and pan state: which time range is visible and how wide it is drawn.
///
/// Moved here from `wave_show::geometry` in R2 step 2 (B2): the committed
/// viewport is store state, the renderer only keeps a copy for drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Time at the left edge of the wave surface.
    pub x_axis: f64,
    /// Pixels per time step.
    pub size: f64,
}

impl Default for View {
    fn default() -> Self {
        Self { x_axis: 0.0, size: 1.0 }
    }
}

impl View {
    pub fn new(x_axis: f64, size: f64) -> Self {
        Self { x_axis, size }
    }

    /// Pixels per time step, never zero (a zero scale would divide by zero).
    fn scale(&self) -> f64 {
        if self.size.is_finite() && self.size > 1e-9 {
            self.size
        } else {
            1e-9
        }
    }

    /// x position of a time stamp, relative to the left edge.
    pub fn x_of(&self, time: Time) -> f64 {
        (time as f64 - self.x_axis) * self.scale()
    }

    /// Time at an x position.
    pub fn time_of(&self, x: f64) -> f64 {
        x / self.scale() + self.x_axis
    }

    /// The time step a click at `x` lands on (what the cursors store).
    pub fn time_at(&self, x: f64) -> i32 {
        clamp_i32(self.time_of(x).floor())
    }

    /// Visible time window for a surface `width` css pixels wide.
    ///
    /// The result is clamped to `>= 0` (a waveform has no negative time) and
    /// `end >= start`, so it can be used as a slice bound directly.
    pub fn window(&self, width: f64) -> (Time, Time) {
        let start = self.time_of(0.0).floor();
        let end = self.time_of(width.max(0.0)).ceil();
        let start = clamp_time(start);
        let end = clamp_time(end).max(start);
        (start, end)
    }

    /// Time steps a surface `width` pixels wide covers.
    pub fn time_span(&self, width: f64) -> f64 {
        width / self.scale()
    }

    /// The same view moved back so its right edge is at (or before) `end_time`.
    ///
    /// Panning past the end of a dump used to leave the last value stretched
    /// over the empty half of the surface, which reads as "the signal never
    /// changes again"; the viewer now stops at the end. A zoomed-out view whose
    /// whole trace is narrower than the surface is left alone (`x_axis` 0).
    pub fn clamp_to_end(self, end_time: i64, width: f64) -> Self {
        if end_time <= 0 || !(width > 0.0) || !self.x_axis.is_finite() {
            return self;
        }
        let max_axis = (end_time as f64 - self.time_span(width)).max(0.0);
        if self.x_axis > max_axis {
            Self::new(max_axis, self.size)
        } else {
            self
        }
    }

    /// Scale that fits `end_time` time steps into `width` pixels.
    pub fn fit_size(end_time: i64, width: f64) -> f64 {
        let steps = (end_time.max(1) as f64) + 1.0;
        let width = if width.is_finite() && width > 0.0 { width } else { 1.0 };
        (width / steps).max(1e-9)
    }

    /// Pan by a number of pixels (positive = to the right = later in time).
    pub fn pan_px(&mut self, dx: f64) {
        self.x_axis = (self.x_axis + dx / self.scale()).max(0.0);
    }

    /// Zoom around the left edge of the surface.
    pub fn zoom(&mut self, factor: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.size = self.scale() * factor;
    }

    /// Zoom around a surface pixel: the time under `px` stays under `px`.
    ///
    /// Verdi zooms at the pointer, not at the left edge: with the edge zoom the
    /// thing you are looking at slides away exactly when you try to see more of
    /// it.
    pub fn zoom_at_px(&mut self, px: f64, factor: f64, width: f64) {
        if !factor.is_finite() || factor <= 0.0 || !px.is_finite() || !(width > 0.0) {
            return;
        }
        let anchor = self.time_of(px.clamp(0.0, width));
        self.zoom(factor);
        self.x_axis = (anchor - px / self.scale()).max(0.0);
    }

    /// Zoom so that the range `[from_px, to_px]` fills the surface again.
    ///
    /// Both x positions are in css pixels of the current view; the left edge
    /// lands on the left of the selection. A selection narrower than one pixel
    /// is ignored (that is a click, not a drag).
    pub fn zoom_to_px(&mut self, from_px: f64, to_px: f64, surface_width: f64) {
        let (from, to) = if from_px <= to_px { (from_px, to_px) } else { (to_px, from_px) };
        let selected = to - from;
        if !(selected >= 1.0) || !(surface_width > 0.0) {
            return;
        }
        let left = self.time_of(from);
        self.size = self.scale() * surface_width / selected;
        self.x_axis = left.max(0.0);
    }
}

/// The two time cursors. Committed values: the click that sets one is the only
/// thing that changes them (the band under the pointer is a local preview).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cursors {
    pub one: Option<i32>,
    pub two: Option<i32>,
}

/// Everything about the visible time range: the viewport and the cursors.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Viewport {
    pub view: View,
    pub cursors: Cursors,
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
        // docs/state-store.md §3.2: a little under half goes to the editor, so
        // the waveform keeps the larger share of the column it sits in
        assert_eq!(layout.editor_ratio, 0.42);
        assert!(layout.drawer_open);
        assert_eq!(layout.clamped(), layout);
    }

    #[test]
    fn status_keeps_its_tone() {
        assert!(!Status::info("parsed top.sv").is_error());
        assert!(Status::error("top.sv: not utf-8").is_error());
    }

    #[test]
    fn rows_compare_by_version() {
        let row = Row {
            path: "top.clk".into(),
            name: "top.clk".into(),
            width: 1,
            setting: Settings::new(),
            chosen: false,
            load: Vec::new(),
            drive: Vec::new(),
        };
        let a = RowsSlice { version: 3, items: Rc::new(vec![row.clone()]) };
        let b = RowsSlice { version: 3, items: Rc::new(vec![]) };
        let c = RowsSlice { version: 4, items: Rc::new(vec![row]) };
        assert_eq!(a, b, "same version means unchanged, whatever the contents");
        assert_ne!(a, c);
    }
}
