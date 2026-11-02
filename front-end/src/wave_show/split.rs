//! The read-only right pane of the split view (docs/design/multi-dump.md §5).
//!
//! It draws one dump with the same canvas machinery as the interactive pane,
//! but it owns no editing surface: no row selection/settings/drag, no dump tab
//! strip, no debug hook (`window.__waveDebug` keeps describing the *active*
//! dump's surface). When the split is linked it maps the left pane's committed
//! window onto this dump's own time range (fraction of the trace, clamped) and
//! draws the active cursors at the mapped times.

use std::cell::Cell;

use yew::prelude::*;

use wave_insight_lib::waveform::TimeRange;

use crate::state::{self, select, DumpHandle, RowsSlice, Store, Subscription, Viewport};

use super::canvas;
use super::geometry::{visible_rows, View};
use super::time::build_axis;
use super::util::signal_things::{derive_bit, SignalThings};

#[derive(Debug, Properties, PartialEq)]
pub struct SplitPaneProps {
    pub store: Store,
    /// Index into `State.dumps.items` of the dump this pane shows.
    pub dump: usize,
    /// Linked: follow the left pane's committed time window.
    pub linked: bool,
}

pub struct SplitPane {
    store: Store,
    dump: usize,
    linked: bool,
    rows: RowsSlice,
    viewport: Viewport,
    waveform: Option<DumpHandle>,
    time_range: Option<TimeRange>,
    /// The active dump's committed viewport/cursors (what a linked pane follows).
    left_viewport: Viewport,
    signal_things: SignalThings,
    canvas_ref: NodeRef,
    /// The pane fitted itself once (unlinked mode): the first paint shows the
    /// whole trace instead of the default 1 px/time.
    fitted: Cell<bool>,
    _rows_sub: Subscription,
    _viewport_sub: Subscription,
    _waveform_sub: Subscription,
    _range_sub: Subscription,
    _left_sub: Subscription,
}

pub enum Msg {
    Rows(RowsSlice),
    Viewport(Viewport),
    Waveform(Option<DumpHandle>),
    TimeRange(Option<TimeRange>),
    LeftViewport(Viewport),
}

impl Component for SplitPane {
    type Message = Msg;
    type Properties = SplitPaneProps;

    fn create(ctx: &Context<Self>) -> Self {
        let store = ctx.props().store.clone();
        let dump = ctx.props().dump;
        let rows_sub = store.subscribe(
            move |state| select::rows_of(state, dump),
            ctx.link().callback(Msg::Rows),
        );
        let viewport_sub = store.subscribe(
            move |state| select::viewport_of(state, dump),
            ctx.link().callback(Msg::Viewport),
        );
        let waveform_sub = store.subscribe(
            move |state| select::dump_waveform_of(state, dump),
            ctx.link().callback(Msg::Waveform),
        );
        let range_sub = store.subscribe(
            move |state| select::time_range_of(state, dump),
            ctx.link().callback(Msg::TimeRange),
        );
        // the linked window follows the active dump's committed viewport
        let left_sub = store.subscribe(select::viewport, ctx.link().callback(Msg::LeftViewport));
        Self {
            rows: store.select(move |state| select::rows_of(state, dump)),
            viewport: store.select(move |state| select::viewport_of(state, dump)),
            waveform: store.select(move |state| select::dump_waveform_of(state, dump)),
            time_range: store.select(move |state| select::time_range_of(state, dump)),
            left_viewport: store.select(select::viewport),
            store,
            dump,
            linked: ctx.props().linked,
            signal_things: SignalThings::new(),
            canvas_ref: NodeRef::default(),
            fitted: Cell::new(false),
            _rows_sub: rows_sub,
            _viewport_sub: viewport_sub,
            _waveform_sub: waveform_sub,
            _range_sub: range_sub,
            _left_sub: left_sub,
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Rows(rows) => self.rows = rows,
            Msg::Viewport(viewport) => self.viewport = viewport,
            Msg::Waveform(waveform) => self.waveform = waveform,
            Msg::TimeRange(range) => self.time_range = range,
            Msg::LeftViewport(viewport) => self.left_viewport = viewport,
        }
        true
    }

    fn rendered(&mut self, _ctx: &Context<Self>, _first: bool) {
        self.redraw();
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        let name = self
            .store
            .select(move |state| select::dump_name_of(state, self.dump))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "dump".to_string());
        html! {
            <div class="split-pane" aria-label={format!("Split pane: {name}")}>
                <div class="split-pane-head">
                    <span class="split-pane-name" title={name.clone()}>{name.clone()}</span>
                    <span class="split-pane-mode">{"read only"}</span>
                </div>
                <div class="split-canvas-wrap">
                    <canvas class="wave-canvas split-canvas" ref={self.canvas_ref.clone()}></canvas>
                </div>
            </div>
        }
    }
}

impl SplitPane {
    /// Draw the pane: measure, decode the visible rows, map the window, paint.
    fn redraw(&mut self) {
        let Some(canvas) = self.canvas_ref.cast::<web_sys::HtmlCanvasElement>() else {
            return;
        };
        let rect = canvas.get_bounding_client_rect();
        let width = rect.width();
        let height = rect.height();
        if width <= 1.0 || height <= 1.0 {
            return;
        }
        let waveform = self.waveform.as_ref().map(|handle| handle.waveform());
        // map the store rows onto the render model (keeps decoded values for the
        // rows that survived)
        let unload = self.signal_things.sync(&self.rows.items, waveform);
        if let Some(waveform) = waveform {
            for index in unload {
                let _ = waveform.unload_signal(index);
            }
        }
        let rows = self.signal_things.len();
        let (draw, fetch) = visible_rows(0.0, height, rows);
        self.decode_pending(fetch);

        let view = self.effective_view(width, height);
        let axis = build_axis(view.x_axis, view.size, width);
        let visible: Vec<(usize, &super::util::signal_things::SignalItem)> = (draw.start..draw.end)
            .filter_map(|row| self.signal_things.iter().nth(row).map(|item| (row, item)))
            .collect();
        let (cursor1, cursor2) = self.mapped_cursors();
        let frame = canvas::Frame {
            view,
            width,
            height,
            scroll_top: 0.0,
            rows: &visible,
            grid: &axis.labels,
            cursor1,
            cursor2,
            drag: None,
            trace_end: self.time_range.map(|range| range.end as i64),
            dpr: web_sys::window()
                .map(|window| window.device_pixel_ratio())
                .unwrap_or(1.0),
        };
        let _ = canvas::paint(&canvas, &frame);
    }

    /// The window this pane shows: the mapped left window when linked, its own
    /// committed viewport otherwise (fitted to the trace on the first paint).
    fn effective_view(&self, width: f64, height: f64) -> View {
        let own = View::new(self.viewport.view.x_axis, self.viewport.view.size);
        let right_range = self.time_range;
        if self.linked {
            if let Some(right_range) = right_range {
                let left = View::new(self.left_viewport.view.x_axis, self.left_viewport.view.size);
                let left_range = self.store.select(select::time_range);
                if let Some(left_range) = left_range {
                    return state::split::linked_view(left, left_range, right_range, width);
                }
            }
        }
        if !self.fitted.get() {
            let end = right_range.map(|range| range.end as i64).unwrap_or(0);
            if end > 0 {
                self.fitted.set(true);
                return View::new(0.0, View::fit_size(end, width));
            }
            let _ = height;
        }
        own
    }

    /// The active cursors mapped onto this dump (linked mode only).
    fn mapped_cursors(&self) -> (Option<i32>, Option<i32>) {
        if !self.linked {
            return (None, None);
        }
        let (Some(left_range), Some(right_range)) =
            (self.store.select(select::time_range), self.time_range)
        else {
            return (None, None);
        };
        let map = |time: Option<i32>| {
            time.and_then(|time| state::split::linked_cursor(time, left_range, right_range))
        };
        (
            map(self.left_viewport.cursors.one),
            map(self.left_viewport.cursors.two),
        )
    }

    /// Decode the rows that are (or are about to be) on screen, in one pass.
    fn decode_pending(&mut self, fetch: std::ops::Range<usize>) {
        let Some(waveform) = self.waveform.as_ref().map(|handle| handle.waveform()) else {
            return;
        };
        let pending = self.signal_things.pending(fetch);
        if pending.is_empty() {
            return;
        }
        let indices: Vec<usize> = pending.iter().map(|(_, index)| *index).collect();
        if let Ok(decoded) = waveform.decoded_changes_many(&indices) {
            for ((row, _), handle) in pending.into_iter().zip(decoded) {
                let bit = self.signal_things.iter().nth(row).and_then(|item| item.bit);
                match bit {
                    Some(bit) => self
                        .signal_things
                        .set_backend(row, derive_bit(&handle, bit as usize)),
                    None => self.signal_things.set_backend(row, handle),
                }
            }
        }
    }
}
