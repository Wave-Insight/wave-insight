//! Windowed decoding for drawing: memory proportional to the *pixels*, not to
//! the dump.
//!
//! Before Wave 13 the renderer asked the backend for the whole history of every
//! visible row ([`decoded_changes_many`]). On a large FST one drawn clock can
//! have hundreds of millions of changes (`40 B each), so a handful of rows blew
//! past the wasm memory limit and crashed the tab.
//!
//! The renderer now asks for one *window* per frame: the visible time range plus
//! a margin, with a per-signal cap derived from the surface width (a few changes
//! per pixel column). A dense signal therefore stops at [`WindowRequest::cap`]
//! changes instead of materialising its whole history, and a sparse signal is
//! simply reported in full. Rows cache their window and reuse it while the view
//! only pans; a zoom (or leaving the cached range) refetches.
//!
//! Everything the drawing code needs from a row is in [`RowWindow`]: the
//! changes inside the visible range, a value at any time inside the fetched
//! range (so the left edge carries the value from before the window), and the
//! change count for the memory budget. The library calls live in [`fetch`]; when
//! the library's bounded [`window_changes_many`] lands, only that function
//! changes.

use std::collections::HashMap;
use std::sync::Arc;

use wave_insight_lib::waveform::{BitValue, Bits, Change, WaveValue, Waveform};

use super::geometry::{self, Time, View};

/// Changes kept per pixel column, per signal. Two would keep a clean step shape
/// but lose narrow pulses; four keeps them and still bounds a full-trace view of
/// a dense clock to a few thousand changes.
pub const CHANGES_PER_PIXEL: usize = 4;
/// Lower bound for a window cap: a very narrow surface still gets enough changes
/// to draw something.
pub const MIN_CAP: usize = 64;
/// Upper bound for one signal's window, whatever the surface width says.
pub const MAX_CAP: usize = 32_768;
/// Margin fetched on both sides of the visible range, as a fraction of it, so a
/// small pan does not refetch and the left edge has a value to carry.
pub const MARGIN_FRACTION: f64 = 0.25;
/// Approximate bytes one cached change costs (the change plus its value payload),
/// for the memory budget. The library uses the same order of magnitude.
pub const BYTES_PER_CHANGE: usize = 40;

/// What one frame asks the backend for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowRequest {
    /// First fetched time stamp (inclusive), already padded by the margin.
    pub start: Time,
    /// Last fetched time stamp (inclusive).
    pub end: Time,
    /// Visible range (inclusive) inside the fetched range.
    pub visible_start: Time,
    pub visible_end: Time,
    /// Changes kept per signal.
    pub cap: usize,
}

impl WindowRequest {
    /// The request for a frame: the visible range plus a margin, capped by the
    /// surface width.
    pub fn for_view(view: View, width: f64) -> Self {
        let width = if width.is_finite() && width > 0.0 {
            width
        } else {
            1.0
        };
        let (visible_start, visible_end) = view.window(width);
        let span = visible_end.saturating_sub(visible_start).max(1);
        let margin = ((span as f64 * MARGIN_FRACTION).ceil() as u64).max(1);
        Self {
            start: visible_start.saturating_sub(margin),
            end: visible_end.saturating_add(margin),
            visible_start,
            visible_end,
            cap: window_cap(width),
        }
    }

    /// Whether a cached window can answer this visible range without a refetch.
    ///
    /// A pan inside the fetched margin reuses the cache; a zoom in needs a
    /// larger cap (more detail), a zoom out a wider range, so both refetch.
    pub fn covers(&self, visible_start: Time, visible_end: Time, cap: usize) -> bool {
        self.start <= visible_start && self.end >= visible_end && self.cap >= cap
    }
}

/// The per-signal change cap for a surface of `width` css pixels.
pub fn window_cap(width: f64) -> usize {
    let width = if width.is_finite() && width > 0.0 {
        width
    } else {
        1.0
    };
    ((width.ceil() as usize).saturating_mul(CHANGES_PER_PIXEL)).clamp(MIN_CAP, MAX_CAP)
}

/// The decoded window of one row.
#[derive(Clone, Default)]
pub struct RowWindow {
    /// Changes inside the fetched range, in time order (delta cycles kept).
    changes: Arc<[Change]>,
    /// Value just before the fetched range, when the backend could provide it
    /// cheaply (the library's bounded API will fill it; the streaming adapter
    /// leaves it [`None`]).
    before: Option<WaveValue>,
    /// The request this window answers.
    pub req: WindowRequest,
    /// The cap stopped this signal: there were more changes in the window.
    pub capped: bool,
}

impl RowWindow {
    /// Build a window from parts (the fetch path and the tests use this).
    pub fn new(
        changes: impl Into<Arc<[Change]>>,
        before: Option<WaveValue>,
        req: WindowRequest,
        capped: bool,
    ) -> Self {
        Self {
            changes: changes.into(),
            before,
            req,
            capped,
        }
    }

    /// The first value the window can show: the first change in the fetched
    /// range, or the carried value from before it.
    pub fn first_value(&self) -> Option<&WaveValue> {
        self.changes
            .first()
            .map(|(_, value)| value)
            .or(self.before.as_ref())
    }

    /// Changes of the whole fetched range (margin included).
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// Approximate bytes this window holds, for the memory budget.
    pub fn bytes(&self) -> usize {
        self.changes.len().saturating_mul(BYTES_PER_CHANGE)
    }

    /// The value in effect at `time`, inside the fetched range.
    ///
    /// At the left edge of the visible range this is the value carried from
    /// before the window (the change just before it, from the margin), which is
    /// what makes a step drawn from the left edge correct.
    pub fn value_at(&self, time: Time) -> Option<&WaveValue> {
        let position = self.changes.partition_point(|(stamp, _)| *stamp <= time);
        if position == 0 {
            self.before.as_ref()
        } else {
            Some(&self.changes[position - 1].1)
        }
    }

    /// The changes inside `start ..= end` (the visible range).
    pub fn window(&self, start: Time, end: Time) -> &[Change] {
        geometry::slice_window(&self.changes, start, end)
    }

    /// Visit the changes of the visible range, thinned to one per pixel.
    pub fn for_each_change<'a>(
        &'a self,
        start: Time,
        end: Time,
        view: &View,
        width: f64,
        mut visit: impl FnMut(f64, &'a WaveValue),
    ) {
        geometry::for_each_pixel_change(self.window(start, end), view, width, |x, (_, value)| {
            visit(x, value)
        });
    }

    /// Last time stamp this window has a *real* change for.
    pub fn span_end(&self) -> Option<Time> {
        self.changes.last().map(|(time, _)| *time)
    }

    /// Whether this row has anything to draw.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.before.is_none()
    }
}

/// Whether a row needs a fetch for this visible range: no window yet, the view
/// left the fetched range, or the zoom needs more changes than the cache holds.
pub fn needs_fetch(cached: Option<&RowWindow>, req: WindowRequest) -> bool {
    match cached {
        None => true,
        Some(window) => !window
            .req
            .covers(req.visible_start, req.visible_end, req.cap),
    }
}

/// Fetch one window for each requested signal, in one streaming pass.
///
/// Adapter over [`Waveform::stream_changes`] (many signals, time window, early
/// exit) until the library's bounded [`window_changes_many`] lands; the per
/// signal cap and the carry bookkeeping are done here so the call site does not
/// care which one runs.
///
/// `signals` are dump signal indices in row order; the result has one entry per
/// requested signal (a signal requested twice is decoded once and cloned, which
/// is one [`Arc`] bump). `None` means "no window" (the row has no signal).
pub fn fetch(
    waveform: &Waveform,
    signals: &[usize],
    req: WindowRequest,
) -> Result<Vec<Option<RowWindow>>, String> {
    if signals.is_empty() {
        return Ok(Vec::new());
    }
    // the backend deduplicates the request, keeps the first slot per signal and
    // caps every signal fairly: one streaming pass, O(signals * cap) memory
    let windowed = waveform
        .window_changes_many(signals, req.start, Some(req.end), req.cap)
        .map_err(|error| format!("{error}"))?;
    // map the deduplicated result back onto the requested rows
    let mut slot_of_signal: HashMap<usize, usize> = HashMap::new();
    let mut slot = 0usize;
    for &signal in signals {
        if let std::collections::hash_map::Entry::Vacant(entry) = slot_of_signal.entry(signal) {
            entry.insert(slot);
            slot += 1;
        }
    }
    let shared: Vec<(Arc<[Change]>, Option<WaveValue>, bool)> = windowed
        .into_iter()
        .map(|row| (Arc::<[Change]>::from(row.changes), row.before, row.capped))
        .collect();
    Ok(signals
        .iter()
        .map(|signal| {
            let slot = slot_of_signal.get(signal).copied()?;
            let (changes, before, capped) = shared.get(slot)?;
            Some(RowWindow::new(
                Arc::clone(changes),
                before.clone(),
                req,
                *capped,
            ))
        })
        .collect())
}

/// One bit of a parent window, for the expanded-bus rows.
///
/// The parent window may be capped on a very dense bus: the derived bit then
/// only shows the changes the parent window kept (documented approximation; the
/// same cap applies to a bit row requested on its own, because the dump only has
/// the bus signal).
pub fn derive_bit_window(parent: &RowWindow, bit: usize) -> RowWindow {
    let mut out: Vec<Change> = Vec::new();
    let mut last: Option<BitValue> = None;
    for (time, value) in parent.changes() {
        let next = value.as_bits().and_then(|bits| bits.get(bit));
        if next == last {
            continue;
        }
        last = next;
        out.push((
            *time,
            WaveValue::Bits(Bits::new_msb_first(vec![next.unwrap_or(BitValue::X)])),
        ));
    }
    let before = parent.before.as_ref().map(|value| {
        let bit_value = value
            .as_bits()
            .and_then(|bits| bits.get(bit))
            .unwrap_or(BitValue::X);
        WaveValue::Bits(Bits::new_msb_first(vec![bit_value]))
    });
    RowWindow::new(out, before, parent.req, parent.capped)
}

/// Bytes all cached windows hold together (the tests and the budget check).
#[cfg(test)]
pub fn cache_bytes<'a>(windows: impl IntoIterator<Item = Option<&'a RowWindow>>) -> usize {
    windows
        .into_iter()
        .flatten()
        .map(RowWindow::bytes)
        .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wave_insight_lib::waveform::BitValue;

    fn request(start: Time, end: Time) -> WindowRequest {
        WindowRequest {
            start,
            end,
            visible_start: start,
            visible_end: end,
            cap: 4096,
        }
    }

    fn bit_change(time: Time, bit: u8) -> Change {
        (
            time,
            WaveValue::Bits(Bits::new_msb_first(vec![if bit == 0 {
                BitValue::Zero
            } else {
                BitValue::One
            }])),
        )
    }

    fn history(times: &[Time]) -> Vec<Change> {
        times
            .iter()
            .enumerate()
            .map(|(index, time)| bit_change(*time, (index % 2) as u8))
            .collect()
    }

    #[test]
    fn the_request_pads_the_visible_range_and_caps_by_pixels() {
        let view = View::new(1000.0, 2.0);
        let req = WindowRequest::for_view(view, 800.0);
        // 800 px at 2 px/time shows 400 time steps; a quarter margin each side
        assert_eq!(req.visible_start, 1000);
        assert_eq!(req.visible_end, 1400);
        assert_eq!(req.start, 900);
        assert_eq!(req.end, 1500);
        assert_eq!(req.cap, 800 * CHANGES_PER_PIXEL);
    }

    #[test]
    fn the_request_never_pans_before_zero() {
        let req = WindowRequest::for_view(View::new(0.0, 1.0), 100.0);
        assert_eq!(req.start, 0);
        assert!(req.end >= req.visible_end);
    }

    #[test]
    fn the_cap_is_bounded_for_tiny_and_huge_surfaces() {
        assert_eq!(window_cap(0.0), MIN_CAP);
        assert_eq!(window_cap(1.0), MIN_CAP);
        assert_eq!(window_cap(800.0), 800 * CHANGES_PER_PIXEL);
        assert_eq!(window_cap(1e9), MAX_CAP);
        assert_eq!(window_cap(f64::NAN), MIN_CAP);
    }

    #[test]
    fn a_pan_inside_the_margin_reuses_the_window() {
        let cached = request(900, 1500);
        let visible = (1000, 1400);
        assert!(cached.covers(visible.0, visible.1, 3200));
        // leaving the fetched range refetches
        assert!(!cached.covers(850, 1400, 3200));
        assert!(!cached.covers(1000, 1600, 3200));
        // zooming in needs a bigger cap than the cache holds
        assert!(!cached.covers(visible.0, visible.1, 20000));
    }

    #[test]
    fn needs_fetch_is_true_without_a_window_and_false_when_covered() {
        let req = WindowRequest::for_view(View::new(1000.0, 2.0), 800.0);
        assert!(needs_fetch(None, req));
        let window = RowWindow::new(history(&[1000, 1200]), None, req, false);
        assert!(!needs_fetch(Some(&window), req));
        // a zoom in invalidates the cache
        let mut zoomed = req;
        zoomed.cap = req.cap * 2;
        zoomed.visible_start = req.visible_start + 100;
        zoomed.visible_end = req.visible_end - 100;
        assert!(needs_fetch(Some(&window), zoomed));
        // a pan inside the margin does not
        let mut panned = req;
        panned.visible_start = req.visible_start + 10;
        panned.visible_end = req.visible_end + 10;
        assert!(!needs_fetch(Some(&window), panned));
    }

    #[test]
    fn the_window_carries_the_value_from_before_the_visible_range() {
        // changes at 0, 10, 20; fetched range 0..=100, visible 15..=100
        let window = RowWindow::new(history(&[0, 10, 20]), None, request(0, 100), false);
        // the value at the left edge is the last change at/before it
        assert!(window.value_at(15).is_some());
        assert_eq!(window.value_at(5), window.value_at(0));
        // nothing at/before the fetched range: no carry (the renderer draws from
        // the first in-window change and documents the approximation)
        let empty = RowWindow::new(history(&[30]), None, request(20, 100), false);
        assert!(empty.value_at(25).is_none());
        // before is used when the backend could provide it
        let carried = RowWindow::new(
            history(&[30]),
            Some(bit_change(0, 1).1),
            request(20, 100),
            false,
        );
        assert!(carried.value_at(25).is_some());
    }

    #[test]
    fn the_visible_slice_ignores_the_margin_changes() {
        let window = RowWindow::new(history(&[0, 100, 200, 300]), None, request(0, 400), false);
        let visible = window.window(100, 300);
        assert_eq!(visible.len(), 3);
        assert_eq!(visible.first().map(|(time, _)| *time), Some(100));
        assert_eq!(visible.last().map(|(time, _)| *time), Some(300));
        assert!(window.window(400, 500).is_empty());
    }

    #[test]
    fn the_window_reports_its_bytes_for_the_budget() {
        let window = RowWindow::new(history(&[0, 10, 20]), None, request(0, 30), false);
        assert_eq!(window.bytes(), 3 * BYTES_PER_CHANGE);
        let empty = RowWindow::new(Vec::new(), None, request(0, 30), false);
        assert_eq!(empty.bytes(), 0);
        assert!(empty.is_empty());
        assert_eq!(
            cache_bytes([Some(&window), None, Some(&empty)]),
            3 * BYTES_PER_CHANGE
        );
    }

    #[test]
    fn an_expanded_bit_is_derived_from_the_parent_window() {
        // a 2-bit bus: 00, 01, 11, 10
        let values = [(0u64, 0u8), (10, 1), (20, 3), (30, 2)];
        let changes: Vec<Change> = values
            .iter()
            .map(|(time, raw)| {
                let bits = Bits::new_msb_first(vec![
                    if raw & 2 != 0 {
                        BitValue::One
                    } else {
                        BitValue::Zero
                    },
                    if raw & 1 != 0 {
                        BitValue::One
                    } else {
                        BitValue::Zero
                    },
                ]);
                (*time, WaveValue::Bits(bits))
            })
            .collect();
        let parent = RowWindow::new(changes, None, request(0, 40), false);
        // bit 0: 0, 1, 1, 0 -> changes at 0, 10, 30
        let bit0 = derive_bit_window(&parent, 0);
        let times: Vec<Time> = bit0.changes().iter().map(|(time, _)| *time).collect();
        assert_eq!(times, vec![0, 10, 30]);
        // bit 1: 0, 0, 1, 1 -> changes at 0 and 20
        let bit1 = derive_bit_window(&parent, 1);
        let times: Vec<Time> = bit1.changes().iter().map(|(time, _)| *time).collect();
        assert_eq!(times, vec![0, 20]);
        // a bit the bus does not have is an empty history, never a panic
        let missing = derive_bit_window(&parent, 40);
        assert!(missing.changes().is_empty());
        assert!(missing.capped == parent.capped);
    }
}
