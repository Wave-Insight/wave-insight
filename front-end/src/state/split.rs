//! The side-by-side split view (docs/design/multi-dump.md step 3).
//!
//! The store owns *which* dump the right pane shows and whether the time axis is
//! linked; the panes themselves stay components. This module holds the pure
//! mapping between the left (active) dump's time window and the right dump's
//! own range, so it can be unit tested on the host.

use wave_insight_lib::waveform::TimeRange;

use super::types::View;

/// Height of one wave row, in css pixels (kept in sync with `--row-h` and with
/// `wave_show::geometry::ROW_HEIGHT`; the state module cannot import the
/// renderer, so the value is repeated here for the scroll clamp).
pub const ROW_HEIGHT: f64 = 30.0;

/// Which dump the right pane shows and whether the panes share a time window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Split {
    /// Index into `Dumps.items`; always a dump other than the active one.
    pub right: usize,
    /// Linked: the right pane shows the same relative window as the left one.
    pub linked: bool,
}

impl Default for Split {
    fn default() -> Self {
        Self {
            right: 0,
            linked: true,
        }
    }
}

/// Map the left pane's visible time window onto the right dump's range.
///
/// The window is mapped by *fraction of the trace*, not by absolute time: two
/// runs of the same testbench usually have the same shape but different time
/// stamps, so "the same slice of the trace" is what a comparison wants. The
/// result keeps the same pixel scale for that slice and is clamped into the
/// right dump's range.
pub fn linked_view(left: View, left_range: TimeRange, right_range: TimeRange, width: f64) -> View {
    let width = if width.is_finite() && width > 1.0 {
        width
    } else {
        1.0
    };
    let left_span = left_range.end.saturating_sub(left_range.start).max(1) as f64;
    let right_span = right_range.end.saturating_sub(right_range.start).max(1) as f64;
    let left_size = if left.size.is_finite() && left.size > 1e-9 {
        left.size
    } else {
        1e-9
    };
    let visible = (width / left_size).max(1.0);
    let left_start = left_range.start as f64;
    // a non-finite axis (a corrupted committed view) degrades to the trace start
    let left_axis = if left.x_axis.is_finite() {
        left.x_axis
    } else {
        left_start
    };
    let f0 = ((left_axis - left_start) / left_span).clamp(0.0, 1.0);
    let f1 = ((left_axis + visible - left_start) / left_span).clamp(0.0, 1.0);
    // at least one time step, so the pane never divides by zero
    let span = ((f1 - f0) * right_span).max(1.0);
    let right_start = right_range.start as f64;
    let axis = (right_start + f0 * right_span).clamp(right_start, right_range.end as f64);
    View::new(axis, width / span)
}

/// The largest row scroll offset that still shows a full surface.
///
/// The right pane scrolls its row window with the same virtualized slice logic
/// as the left pane; this is the clamp (never negative, never past the last
/// row).
pub fn max_row_scroll(rows: usize, surface_h: f64) -> f64 {
    if !surface_h.is_finite() || surface_h <= 0.0 {
        return 0.0;
    }
    (rows as f64 * ROW_HEIGHT - surface_h).max(0.0)
}

/// Map one committed cursor time of the left dump onto the right dump.
///
/// Returns `None` when the cursor lies outside the left trace (there is no
/// meaningful position for it on the other trace).
pub fn linked_cursor(time: i32, left_range: TimeRange, right_range: TimeRange) -> Option<i32> {
    let left_span = left_range.end.saturating_sub(left_range.start).max(1) as f64;
    let fraction = (time as f64 - left_range.start as f64) / left_span;
    if !(0.0..=1.0).contains(&fraction) {
        return None;
    }
    let right_span = right_range.end.saturating_sub(right_range.start).max(1) as f64;
    let mapped = right_range.start as f64 + fraction * right_span;
    Some(mapped.round() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start: u64, end: u64) -> TimeRange {
        TimeRange { start, end }
    }

    #[test]
    fn linked_view_maps_the_same_fraction_of_both_traces() {
        // the left pane shows the middle half of its trace
        let left = View::new(100.0, 2.0);
        let mapped = linked_view(left, range(0, 200), range(0, 400), 100.0);
        // the left window covers 50 of 200 steps from the halfway point: the same
        // quarter of the right trace starts at 200 and covers 100 steps
        assert_eq!(mapped.x_axis, 200.0);
        assert!((mapped.size - 1.0).abs() < 1e-9, "{}", mapped.size);
    }

    #[test]
    fn linked_view_clamps_into_the_right_range() {
        // a window past the end of the left trace clamps to the right end
        let left = View::new(900.0, 1.0);
        let mapped = linked_view(left, range(0, 200), range(0, 400), 100.0);
        assert_eq!(mapped.x_axis, 400.0);
        assert!(mapped.size.is_finite() && mapped.size > 0.0);
        // a window before the start clamps to the start
        let left = View::new(0.0, 100.0);
        let mapped = linked_view(left, range(50, 250), range(10, 30), 100.0);
        assert_eq!(mapped.x_axis, 10.0);
    }

    #[test]
    fn linked_view_survives_degenerate_inputs() {
        let mapped = linked_view(View::new(0.0, 0.0), range(0, 0), range(0, 0), 0.0);
        assert!(mapped.size.is_finite() && mapped.size > 0.0);
        let mapped = linked_view(View::new(f64::NAN, 1.0), range(0, 10), range(0, 10), 100.0);
        assert!(mapped.x_axis.is_finite() && mapped.size.is_finite());
    }

    #[test]
    fn the_row_scroll_clamps_to_the_row_count() {
        // 10 rows of 30 px in a 100 px surface: the last row can reach the top
        assert_eq!(max_row_scroll(10, 100.0), 200.0);
        // fewer rows than fit: no scrolling at all
        assert_eq!(max_row_scroll(2, 100.0), 0.0);
        // a degenerate surface is not a panic
        assert_eq!(max_row_scroll(10, f64::NAN), 0.0);
        assert_eq!(max_row_scroll(10, 0.0), 0.0);
    }

    #[test]
    fn linked_cursor_maps_by_fraction_and_clamps_outside_the_trace() {
        assert_eq!(linked_cursor(50, range(0, 100), range(0, 200)), Some(100));
        assert_eq!(linked_cursor(0, range(0, 100), range(0, 200)), Some(0));
        assert_eq!(linked_cursor(100, range(0, 100), range(0, 200)), Some(200));
        assert_eq!(linked_cursor(-5, range(0, 100), range(0, 200)), None);
        assert_eq!(linked_cursor(150, range(0, 100), range(0, 200)), None);
    }
}
