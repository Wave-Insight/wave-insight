//! Pure geometry for the canvas renderer.
//!
//! Nothing in this module touches the DOM, so the tests run on the host
//! (`cargo test -p front-end --bins`) instead of needing a browser. The canvas
//! layer below (`canvas.rs`) only turns these numbers into drawing calls, and
//! the component (`wave.rs`) only turns DOM measurements into these inputs.
//!
//! Time is in wave units (the dump's time steps, `u64`); `x` is in css pixels
//! inside the wave surface, `0` being the left edge of the visible window.

use wave_insight_lib::waveform::Change;

/// A time stamp in wave units.
///
/// The library's [`Change`] is `(Time, WaveValue)` with `Time = u64`; the alias
/// is not re-exported, so the renderer uses its own name for it.
pub type Time = u64;

/// Height of one signal row, in css pixels.
///
/// Keep in sync with `--row-h` in `styles.css`.
pub const ROW_HEIGHT: f64 = 30.0;

/// Rows that are decoded around the viewport so a scroll does not have to wait
/// for the backend. Drawing only uses the rows that are really visible.
pub const PREFETCH_ROWS: usize = 12;

/// Minimum horizontal distance between two drawn changes, in css pixels.
///
/// A dump can have hundreds of changes per pixel; drawing all of them costs a
/// line call each and looks the same as drawing the last one per bucket.
const MIN_PX_PER_CHANGE: f64 = 1.0;

/// Zoom and pan state: which time range is visible and how wide it is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Time at the left edge of the wave surface.
    pub x_axis: f64,
    /// Pixels per time step.
    pub size: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            x_axis: 0.0,
            size: 1.0,
        }
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

    /// Scale that fits `end_time` time steps into `width` pixels.
    pub fn fit_size(end_time: i64, width: f64) -> f64 {
        let steps = (end_time.max(1) as f64) + 1.0;
        let width = if width.is_finite() && width > 0.0 {
            width
        } else {
            1.0
        };
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

    /// Zoom so that the range `[from_px, to_px]` fills the surface again.
    ///
    /// Both x positions are in css pixels of the current view; the left edge
    /// lands on the left of the selection. A selection narrower than one pixel
    /// is ignored (that is a click, not a drag).
    pub fn zoom_to_px(&mut self, from_px: f64, to_px: f64, surface_width: f64) {
        let (from, to) = if from_px <= to_px {
            (from_px, to_px)
        } else {
            (to_px, from_px)
        };
        let selected = to - from;
        if !(selected >= 1.0) || !(surface_width > 0.0) {
            return;
        }
        let left = self.time_of(from);
        self.size = self.scale() * surface_width / selected;
        self.x_axis = left.max(0.0);
    }
}

/// Old `ModuleValue`/cursor time stamps are `i32`; clamp instead of wrapping.
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

/// The rows that intersect the viewport, plus a prefetch margin.
///
/// Returns `(draw_first, draw_last, fetch_first, fetch_last)`; the draw range is
/// half open (`first..last`) and clamped to `rows`, the fetch range is what the
/// backend should decode (never more than `PREFETCH_ROWS` beyond the draw range).
pub fn visible_rows(
    scroll_top: f64,
    viewport_height: f64,
    rows: usize,
) -> (std::ops::Range<usize>, std::ops::Range<usize>) {
    if rows == 0 || !scroll_top.is_finite() || !viewport_height.is_finite() {
        return (0..0, 0..0);
    }
    let first = (scroll_top / ROW_HEIGHT).floor().max(0.0) as usize;
    let visible = (viewport_height / ROW_HEIGHT).ceil().max(1.0) as usize;
    let last = (first + visible).min(rows);
    let first = first.min(rows);

    let fetch_first = first.saturating_sub(PREFETCH_ROWS);
    let fetch_last = (last + PREFETCH_ROWS).min(rows);
    (first..last, fetch_first..fetch_last)
}

/// Total height of `rows` rows, for the scroll container.
pub fn content_height(rows: usize) -> f64 {
    rows as f64 * ROW_HEIGHT
}

/// Anything the renderer can draw: a `(time, value)` pair.
///
/// Implemented for the backend's [`Change`] and for the legacy transports'
/// `(i32, BoolData)` / `(i32, BitsData)` pairs, so the slicing and decimation
/// below only exist once.
pub trait Stamped {
    /// Time stamp in wave units.
    fn stamp(&self) -> Time;
}

impl Stamped for Change {
    fn stamp(&self) -> Time {
        self.0
    }
}

impl Stamped for (i32, wave_insight_lib::data_struct::BoolData) {
    fn stamp(&self) -> Time {
        clamp_time(self.0 as f64)
    }
}

impl Stamped for (i32, wave_insight_lib::data_struct::BitsData) {
    fn stamp(&self) -> Time {
        clamp_time(self.0 as f64)
    }
}

/// [`slice_window`] for anything time stamped.
pub fn slice_window_stamped<T: Stamped>(items: &[T], start: Time, end: Time) -> &[T] {
    let from = items.partition_point(|item| item.stamp() < start);
    let to = items.partition_point(|item| item.stamp() <= end);
    if from >= to {
        &[]
    } else {
        &items[from..to]
    }
}

/// [`value_at`] for anything time stamped.
pub fn value_at_stamped<T: Stamped>(items: &[T], time: Time) -> Option<&T> {
    let position = items.partition_point(|item| item.stamp() <= time);
    if position == 0 {
        None
    } else {
        items.get(position - 1)
    }
}

/// Visit the changes of `window` that deserve at least one pixel.
///
/// One change per pixel bucket is emitted, always the *last* one of the bucket,
/// so the level that is drawn is the level in effect at the end of that bucket.
/// A sparse window emits every change; a window with hundreds of changes per
/// pixel emits at most one per pixel instead of hundreds, which is what keeps a
/// long history cheap to draw.
pub fn for_each_pixel_change<'a>(
    window: &'a [Change],
    view: &View,
    width: f64,
    visit: impl FnMut(f64, &'a Change),
) {
    for_each_pixel_stamped(window, view, width, visit)
}

/// [`for_each_pixel_change`] for anything time stamped.
pub fn for_each_pixel_stamped<'a, T: Stamped>(
    window: &'a [T],
    view: &View,
    width: f64,
    mut visit: impl FnMut(f64, &'a T),
) {
    if window.is_empty() || !width.is_finite() || width <= 0.0 {
        return;
    }
    let bucket = MIN_PX_PER_CHANGE.max(width / 4096.0);
    let mut index = 0usize;
    while index < window.len() {
        let x = view.x_of(window[index].stamp());
        let bucket_end = ((x / bucket).floor() + 1.0) * bucket;
        //`partition_point` is a binary search, so a bucket with thousands of
        //changes costs log(n), not n
        let end = index + window[index..].partition_point(|item| view.x_of(item.stamp()) <= bucket_end);
        let last = end.max(index + 1) - 1;
        visit(
            view.x_of(window[last].stamp()).clamp(0.0, width),
            &window[last],
        );
        index = last + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wave_insight_lib::waveform::{BitValue, Bits, WaveValue};

    fn change(time: Time, bit: u8) -> Change {
        let value = if bit == 0 {
            BitValue::Zero
        } else {
            BitValue::One
        };
        (
            time,
            WaveValue::Bits(Bits::new_msb_first(vec![value])),
        )
    }

    fn history(times: &[Time]) -> Vec<Change> {
        times
            .iter()
            .enumerate()
            .map(|(index, time)| change(*time, (index % 2) as u8))
            .collect()
    }

    #[test]
    fn x_and_time_round_trip() {
        let view = View::new(100.0, 2.0);
        assert_eq!(view.x_of(100), 0.0);
        assert_eq!(view.x_of(150), 100.0);
        assert!((view.time_of(100.0) - 150.0).abs() < 1e-9);
        assert_eq!(view.time_at(100.0), 150);
        assert_eq!(view.time_at(-1000.0), 0);
    }

    #[test]
    fn window_is_clamped_and_ordered() {
        let view = View::new(50.0, 1.0);
        assert_eq!(view.window(100.0), (50, 150));
        //a view panned to the left never reports a negative time
        let view = View::new(-20.0, 1.0);
        assert_eq!(view.window(100.0), (0, 80));
        //a tiny width still gives a usable window
        let view = View::new(0.0, 1.0);
        assert_eq!(view.window(0.0), (0, 0));
        //a huge scale does not produce a reversed window
        let view = View::new(10.0, 1e9);
        let (start, end) = view.window(100.0);
        assert!(end >= start);
    }

    #[test]
    fn fit_size_shows_the_whole_trace() {
        //100 time steps, 1000 px: every step is 10 px wide, which is what the
        //old renderer used as the minimum zoom
        let size = View::fit_size(99, 1000.0);
        assert!((size - 10.0).abs() < 1e-9);
        //a degenerate dump does not divide by zero
        assert!(View::fit_size(0, 0.0) > 0.0);
        assert!(View::fit_size(-5, 100.0).is_finite());
    }

    #[test]
    fn pan_and_zoom_stay_non_negative() {
        let mut view = View::new(0.0, 2.0);
        view.pan_px(-100.0);
        assert_eq!(view.x_axis, 0.0, "panning left stops at time zero");
        view.pan_px(100.0);
        assert!((view.x_axis - 50.0).abs() < 1e-9);
        view.zoom(2.0);
        assert!((view.size - 4.0).abs() < 1e-9);
        view.zoom(f64::NAN);
        assert!((view.size - 4.0).abs() < 1e-9, "a bad factor is ignored");
    }

    #[test]
    fn drag_zoom_puts_the_selection_on_the_whole_surface() {
        //1000 px surface, 1 px per step, dragging 200..400
        let mut view = View::new(0.0, 1.0);
        view.zoom_to_px(200.0, 400.0, 1000.0);
        //the left edge is at time 200 and 200 steps now fill 1000 px
        assert!((view.x_axis - 200.0).abs() < 1e-9);
        assert!((view.size - 5.0).abs() < 1e-9);
        assert_eq!(view.window(1000.0), (200, 400));

        //a reversed drag selects the same range
        let mut backwards = View::new(0.0, 1.0);
        backwards.zoom_to_px(400.0, 200.0, 1000.0);
        assert!((backwards.x_axis - 200.0).abs() < 1e-9);
        assert!((backwards.size - 5.0).abs() < 1e-9);

        //a click (or a zero width surface) does not change the view
        let mut click = View::new(7.0, 3.0);
        click.zoom_to_px(100.0, 100.4, 1000.0);
        assert_eq!(click, View::new(7.0, 3.0));
        click.zoom_to_px(100.0, 200.0, 0.0);
        assert_eq!(click, View::new(7.0, 3.0));

        //zooming into the very beginning does not pan into negative time
        let mut at_zero = View::new(0.0, 1.0);
        at_zero.zoom_to_px(0.0, 500.0, 1000.0);
        assert_eq!(at_zero.x_axis, 0.0);
    }

    #[test]
    fn visible_rows_covers_the_viewport_with_a_prefetch_margin() {
        //1000 rows, 30px each; a 900px viewport shows 30 rows
        let (draw, fetch) = visible_rows(0.0, 900.0, 1000);
        assert_eq!(draw, 0..30);
        assert_eq!(fetch, 0..(30 + PREFETCH_ROWS));

        let (draw, fetch) = visible_rows(300.0 * ROW_HEIGHT, 900.0, 1000);
        assert_eq!(draw, 300..330);
        assert_eq!(fetch.start, 300 - PREFETCH_ROWS);
        assert_eq!(fetch.end, 330 + PREFETCH_ROWS);

        //scrolled to the bottom: never past the last row
        let (draw, fetch) = visible_rows(100_000.0, 900.0, 40);
        assert!(draw.end <= 40 && draw.start <= 40);
        assert_eq!(fetch.end, 40);

        //an empty list, or a viewport with no height, draws nothing
        assert_eq!(visible_rows(0.0, 900.0, 0), (0..0, 0..0));
        assert_eq!(visible_rows(0.0, 0.0, 10), (0..1, 0..10));
        assert_eq!(visible_rows(f64::NAN, 900.0, 10), (0..0, 0..0));
    }

    #[test]
    fn content_height_matches_the_rows() {
        assert_eq!(content_height(0), 0.0);
        assert_eq!(content_height(10), 300.0);
    }

    #[test]
    fn slice_window_bounds_are_inclusive() {
        let changes = history(&[0, 10, 20, 30, 30]);
        let times = |slice: &[Change]| slice.iter().map(|(time, _)| *time).collect::<Vec<_>>();
        assert_eq!(times(slice_window_stamped(&changes, 0, 30)), vec![0, 10, 20, 30, 30]);
        assert_eq!(times(slice_window_stamped(&changes, 10, 20)), vec![10, 20]);
        assert_eq!(
            times(slice_window_stamped(&changes, 11, 19)),
            Vec::<Time>::new()
        );
        assert_eq!(times(slice_window_stamped(&changes, 30, 30)), vec![30, 30]);
        assert!(slice_window_stamped::<Change>(&[], 0, 100).is_empty());
        //a reversed window is empty, not a panic
        assert!(slice_window_stamped(&changes, 30, 10).is_empty());
    }

    #[test]
    fn value_at_picks_the_last_change_at_or_before() {
        let changes = history(&[0, 10, 20, 30, 30]);
        assert!(value_at_stamped(&changes, 0).is_some());
        assert!(value_at_stamped(&[] as &[Change], 0).is_none());
        assert!(value_at_stamped(&changes[1..], 5).is_none());
        //the second `30` wins over the first one (delta cycle)
        let last = value_at_stamped(&changes, 30).unwrap();
        assert_eq!(last, &changes[4]);
        assert_eq!(value_at_stamped(&changes, 1_000_000).unwrap(), &changes[4]);
    }

    #[test]
    fn pixel_buckets_collapse_a_dense_history() {
        //20000 changes over 1 time step each, drawn 1000 px wide: 20 changes/pixel
        let changes: Vec<Change> = (0..20_000u64).map(|time| change(time, (time % 2) as u8)).collect();
        let view = View::new(0.0, 1000.0 / 20_000.0);
        let mut drawn = Vec::new();
        for_each_pixel_change(&changes, &view, 1000.0, |x, change| drawn.push((x, change.0)));

        //at most one change per pixel (plus the bucket alignment)
        assert!(drawn.len() <= 1002, "drew {} points", drawn.len());
        //the last change is always drawn, so the trace ends where it should
        assert_eq!(drawn.last().unwrap().1, 19_999);
        //x positions never run backwards
        assert!(drawn.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        //and they stay inside the surface
        assert!(drawn.iter().all(|(x, _)| (0.0..=1000.0).contains(x)));
    }

    #[test]
    fn pixel_buckets_keep_a_sparse_history_complete() {
        let changes = history(&[0, 10, 20, 30]);
        let view = View::new(0.0, 10.0); //10 px per change
        let mut drawn = Vec::new();
        for_each_pixel_change(&changes, &view, 1000.0, |_, change| drawn.push(change.0));
        assert_eq!(drawn, vec![0, 10, 20, 30]);
    }

    #[test]
    fn pixel_buckets_handle_empty_and_zero_width() {
        let changes = history(&[0, 10]);
        let view = View::default();
        let mut count = 0;
        for_each_pixel_change(&[], &view, 100.0, |_, _| count += 1);
        for_each_pixel_change(&changes, &view, 0.0, |_, _| count += 1);
        assert_eq!(count, 0);
    }

    #[test]
    fn time_conversions_saturate() {
        assert_eq!(clamp_i32(-5.0), 0);
        assert_eq!(clamp_i32(f64::NAN), 0);
        assert_eq!(clamp_i32(1e30), i32::MAX);
        assert_eq!(clamp_time(-1.0), 0);
        assert_eq!(clamp_time(f64::NAN), 0);
        assert_eq!(clamp_time(f64::INFINITY), u64::MAX);
        assert_eq!(clamp_time(42.9), 42);
    }

    #[test]
    fn legacy_pairs_are_sliced_like_backend_changes() {
        use wave_insight_lib::data_struct::BoolData;
        let legacy: Vec<(i32, BoolData)> = vec![
            (0, BoolData::Zero),
            (10, BoolData::One),
            (20, BoolData::Zero),
        ];
        //the same inclusive window semantics as the backend rows
        assert_eq!(slice_window_stamped(&legacy, 0, 20).len(), 3);
        assert_eq!(slice_window_stamped(&legacy, 10, 10).len(), 1);
        assert!(slice_window_stamped(&legacy, 11, 19).is_empty());
        assert_eq!(value_at_stamped(&legacy, 15).unwrap().1, BoolData::One);
        //nothing is in effect before the first change of an empty history
        let empty: Vec<(i32, BoolData)> = Vec::new();
        assert!(value_at_stamped(&empty, 0).is_none());

        //negative legacy time stamps are clamped, they cannot index a window
        let negative: Vec<(i32, BoolData)> = vec![(-5, BoolData::One)];
        assert_eq!(negative[0].stamp(), 0);

        //and the decimation works on them too
        let view = View::new(0.0, 10.0);
        let mut drawn = Vec::new();
        for_each_pixel_stamped(&legacy, &view, 1000.0, |_, item| drawn.push(item.0));
        assert_eq!(drawn, vec![0, 10, 20]);
    }
}
