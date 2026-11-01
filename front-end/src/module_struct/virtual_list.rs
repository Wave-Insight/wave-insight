//! Windowing math for the two sidebar lists (the module tree and the signal
//! list).
//!
//! A 105 MB dump declares tens of thousands of signals; drawing one DOM row per
//! signal made the sidebar jank and the browser work through hundreds of
//! thousands of nodes. The lists are virtualized now: only the rows inside the
//! scroll viewport (plus a small overscan) exist in the DOM, and a spacer above
//! and below keeps the scrollbar the real length.
//!
//! The DOM contract the QA matrix reads is unchanged for every list it actually
//! looks at: lists up to `VIRTUALIZE_ABOVE` rows are rendered whole, exactly as
//! before (small fixtures keep the same DOM). Longer lists render a window and
//! publish the logical list through the debug seam (`super::seam`), so an
//! assertion can still see the full list and check that the DOM stays bounded.
//!
//! Everything here is plain arithmetic, so it is unit tested on the host target
//! (cargo test -p front-end --bins).

/// Height of one sidebar row in css pixels: styles.css gives both .tree-row and
/// .sig-row a height of 24px. The spacer math has to agree with it or the
/// scrollbar drifts.
pub const ROW_H: f64 = 24.0;

/// Rows drawn outside the viewport, so a fast scroll does not flash blank space
/// before the next frame fills the window.
pub const OVERSCAN: usize = 10;

/// Lists up to this length are rendered whole. The QA matrix (and the eye)
/// looks at small dumps; keeping them unvirtualized keeps the DOM, the selection
/// behaviour and every .sig-row/.tree-row count identical to before this change.
pub const VIRTUALIZE_ABOVE: usize = 80;

/// Viewport height assumed until the scroll container has been measured (the
/// first paint, before the browser has laid the sidebar out).
pub const FALLBACK_VIEWPORT: f64 = 480.0;

/// The slice of a list one render draws, plus the spacer heights around it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Window {
    /// Rows in the whole list.
    pub total: usize,
    /// First rendered row (inclusive).
    pub first: usize,
    /// One past the last rendered row.
    pub last: usize,
    /// Height of the spacer above the rendered rows, in px.
    pub top: f64,
    /// Height of the spacer below the rendered rows, in px.
    pub bottom: f64,
    /// Whether rows are being skipped (the list is longer than the window).
    pub windowed: bool,
}

impl Window {
    /// Nothing to draw: an empty list.
    pub const EMPTY: Window = Window {
        total: 0,
        first: 0,
        last: 0,
        top: 0.0,
        bottom: 0.0,
        windowed: false,
    };

    /// Number of rows this window renders.
    pub fn len(&self) -> usize {
        self.last.saturating_sub(self.first)
    }

    /// True when the window renders no rows at all.
    ///
    /// The windowing code draws an empty list as a window with no rows, so this
    /// is the companion of [`Window::len`] (clippy wants the pair); the drawing
    /// code uses `len()` directly.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The window one render draws: `total` rows, a viewport `height` px tall,
/// scrolled to `scroll_top` px, with the default row height, overscan and
/// "render whole below this many rows" threshold.
pub fn window(total: usize, height: f64, scroll_top: f64, row_h: f64) -> Window {
    window_with(total, height, scroll_top, row_h, OVERSCAN, VIRTUALIZE_ABOVE)
}

/// `window` with the tuning knobs exposed, so the math is testable at small
/// sizes.
///
/// The returned window always satisfies
/// `top + len() * row_h + bottom == total * row_h`, so the scrollable height of
/// the list does not depend on where it is scrolled to.
pub fn window_with(
    total: usize,
    height: f64,
    scroll_top: f64,
    row_h: f64,
    overscan: usize,
    virtualize_above: usize,
) -> Window {
    let row_h = if row_h.is_finite() && row_h > 0.0 {
        row_h
    } else {
        ROW_H
    };
    if total == 0 {
        return Window::EMPTY;
    }
    // a short list is drawn whole: this is the pre-virtualization DOM
    if total <= virtualize_above {
        return Window {
            total,
            first: 0,
            last: total,
            top: 0.0,
            bottom: 0.0,
            windowed: false,
        };
    }
    let height = if height.is_finite() && height > 0.0 {
        height
    } else {
        // unmeasured (first paint): assume a sane viewport rather than the
        // whole list, so a 20k-row sidebar still starts bounded
        FALLBACK_VIEWPORT
    };
    let scroll_top = if scroll_top.is_finite() && scroll_top > 0.0 {
        scroll_top
    } else {
        0.0
    };

    // one extra row covers a viewport that is not a whole number of rows
    let visible = (height / row_h).ceil() as usize + 1;
    let last_row = total - 1;
    let first_row = (scroll_top / row_h).floor() as usize;
    let first = first_row.saturating_sub(overscan).min(last_row);
    let last = (first + visible + 2 * overscan).min(total);
    let first = first.min(last.saturating_sub(1));
    let top = first as f64 * row_h;
    let bottom = (total - last) as f64 * row_h;
    debug_assert!(
        (top + (last - first) as f64 * row_h + bottom - total as f64 * row_h).abs() < 0.5,
        "the spacers must keep the scroll height of the whole list"
    );
    Window {
        total,
        first,
        last,
        top,
        bottom,
        windowed: first > 0 || last < total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f64 = 480.0;

    #[test]
    fn empty_list_renders_nothing() {
        let w = window(0, H, 0.0, ROW_H);
        assert_eq!(w, Window::EMPTY);
        assert_eq!(w.len(), 0);
        assert!(w.is_empty());
        assert!(!w.windowed);
    }

    #[test]
    fn short_lists_render_whole_without_spacers() {
        // seven rows: the largest sidebar list the QA fixtures select
        let w = window(7, 200.0, 0.0, ROW_H);
        assert_eq!((w.first, w.last), (0, 7));
        assert_eq!((w.top, w.bottom), (0.0, 0.0));
        assert!(!w.windowed);
        // the threshold is inclusive
        let w = window(VIRTUALIZE_ABOVE, H, 0.0, ROW_H);
        assert_eq!(w.len(), VIRTUALIZE_ABOVE);
        assert!(!w.windowed);
        // one more row and the window starts skipping
        let w = window(VIRTUALIZE_ABOVE + 1, 100.0, 0.0, ROW_H);
        assert!(w.windowed);
        assert!(w.len() < VIRTUALIZE_ABOVE + 1);
    }

    #[test]
    fn unmeasured_large_list_still_renders_a_bounded_window() {
        // first paint: the sidebar has not been laid out yet
        let w = window(20_000, 0.0, 0.0, ROW_H);
        assert_eq!(w.first, 0);
        assert!(w.windowed);
        // fallback viewport is a bounded number of rows
        assert_eq!(
            w.len(),
            (FALLBACK_VIEWPORT / ROW_H).ceil() as usize + 1 + 2 * OVERSCAN
        );
        assert!(w.len() < 100);
        assert_eq!(w.top, 0.0);
        assert_eq!(w.bottom, (20_000 - w.len()) as f64 * ROW_H);
    }

    #[test]
    fn window_at_the_top_has_no_leading_spacer() {
        let w = window(2_000, H, 0.0, ROW_H);
        assert_eq!(w.first, 0);
        assert_eq!(w.top, 0.0);
        assert!(w.windowed);
    }

    #[test]
    fn window_in_the_middle_carries_the_overscan() {
        // scrolled to row 1000: ten rows of context are kept above it
        let w = window(2_000, H, 1_000.0 * ROW_H, ROW_H);
        assert_eq!(w.first, 990);
        assert_eq!(w.top, 990.0 * ROW_H);
        assert!(w.last > 1_000);
        assert!(w.last < 2_000);
        assert_eq!(w.bottom, (2_000 - w.last) as f64 * ROW_H);
    }

    #[test]
    fn window_at_the_bottom_is_clamped_to_the_last_row() {
        let max_scroll = 2_000.0 * ROW_H - H;
        let w = window(2_000, H, max_scroll, ROW_H);
        assert_eq!(w.last, 2_000);
        assert_eq!(w.bottom, 0.0);
        // the window still starts inside the list
        assert!(w.first < 2_000);
        // scrolling past the end (a stale scrollTop, a resized container) still
        // renders at least one row instead of panicking or drawing nothing
        let w = window(2_000, H, 1e12, ROW_H);
        assert!(w.last <= 2_000);
        assert!(!w.is_empty());
    }

    #[test]
    fn spacers_keep_the_scroll_height_of_the_whole_list() {
        let total = 1_234;
        let total_px = total as f64 * ROW_H;
        for top in (0..(total as i64 * ROW_H as i64))
            .step_by(97)
            .map(|value| value as f64)
        {
            let w = window(total, H, top, ROW_H);
            let height = w.top + w.len() as f64 * ROW_H + w.bottom;
            assert!(
                (height - total_px).abs() < 0.5,
                "scroll {top}: rendered {height} px for a {total_px} px list"
            );
        }
    }

    #[test]
    fn the_rendered_window_is_always_bounded() {
        let total = 10_000;
        let visible = (H / ROW_H).ceil() as usize + 1;
        for top in (0..(total as i64 * ROW_H as i64))
            .step_by(311)
            .map(|value| value as f64)
        {
            let w = window(total, H, top, ROW_H);
            assert!(
                w.len() <= visible + 2 * OVERSCAN,
                "scroll {top} rendered {} rows",
                w.len()
            );
            assert!(w.first <= w.last);
            assert!(w.last <= total);
        }
    }

    #[test]
    fn garbage_geometry_does_not_panic() {
        for (height, top) in [
            (f64::NAN, 0.0),
            (0.0, f64::INFINITY),
            (-10.0, -20.0),
            (H, f64::NAN),
            (H, f64::NEG_INFINITY),
        ] {
            let w = window(2_000, height, top, ROW_H);
            assert_eq!(w.total, 2_000);
            assert!(!w.is_empty() && w.last <= 2_000);
            let height_px = w.top + w.len() as f64 * ROW_H + w.bottom;
            assert!((height_px - 2_000.0 * ROW_H).abs() < 0.5);
        }
        // a nonsense row height falls back to the css row height
        let w = window(200, H, 0.0, f64::NAN);
        assert_eq!(w.first, 0);
        assert!(w.len() < 200);
    }

    #[test]
    fn row_height_and_overscan_are_honoured() {
        // 1000 rows of 30 px in a 240 px viewport with a 2 row overscan
        let w = window_with(1_000, 240.0, 0.0, 30.0, 2, 10);
        let visible = (240.0_f64 / 30.0).ceil() as usize + 1;
        assert_eq!(w.first, 0);
        assert_eq!(w.last, visible + 4);
        assert_eq!(w.top, 0.0);
        assert_eq!(w.bottom, (1_000 - w.last) as f64 * 30.0);
    }
}
