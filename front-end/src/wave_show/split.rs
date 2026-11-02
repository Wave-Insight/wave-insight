//! The read-only right pane of the split view (docs/design/multi-dump.md §5).
//!
//! It draws one dump with the same canvas machinery as the interactive pane,
//! but it owns no editing surface: no row selection/settings/drag, no dump tab
//! strip, no debug hook (`window.__waveDebug` keeps describing the *active*
//! dump's surface). When the split is linked it maps the left pane's committed
//! window onto this dump's own time range (fraction of the trace, clamped) and
//! draws the active cursors at the mapped times.

use std::cell::{Cell, RefCell};

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use yew::prelude::*;

use wave_insight_lib::waveform::TimeRange;

use crate::state::{self, select, Action, DumpHandle, RowsSlice, Store, Subscription, Viewport};

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
    /// Pixels the row list is scrolled up (unlinked mode; the linked pane still
    /// follows the left window but keeps its own row scroll).
    scroll_top: f64,
    /// The measured surface height of the last paint (wheel scroll clamp).
    surface_h: f64,
    /// Drag-pan in progress: pointer x at the press and the view axis then.
    drag: Option<(f64, f64)>,
    /// Live view while a drag-pan runs (tier A: no store write until mouseup).
    preview: Option<View>,
    /// The inputs of the last paint, published on the pane as `title`/`data-frame`
    /// so a browser session can assert what window the pane shows.
    painted_view: Cell<Option<(f64, f64)>>,
    painted_frame: RefCell<Option<String>>,
    /// The surface size of the last paint; `rendered` asks for one more render
    /// when the box has moved since (the layout can settle after the paint).
    painted_w: Cell<f64>,
    painted_h: Cell<f64>,
    /// Watches the canvas box: the window can resize (or the divider move)
    /// without any props changing, and the backing store must follow.
    resize_observer: Option<web_sys::ResizeObserver>,
    resize_callback: Option<Closure<dyn FnMut()>>,
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
    /// Wheel over the pane: Ctrl+wheel zooms, a plain wheel scrolls the rows.
    Wheel(WheelEvent),
    /// Drag-pan started.
    DragStart(MouseEvent),
    /// Pointer moved while the pane is dragged.
    DragMove(MouseEvent),
    /// Drag-pan finished: commit the preview viewport to the store.
    DragEnd,
    /// The canvas box changed (window resize, divider drag): repaint at the
    /// new size, which also resizes the backing store.
    Resize,
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
            scroll_top: 0.0,
            surface_h: 0.0,
            drag: None,
            preview: None,
            painted_view: Cell::new(None),
            painted_frame: RefCell::new(None),
            painted_w: Cell::new(0.0),
            painted_h: Cell::new(0.0),
            resize_observer: None,
            resize_callback: None,
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
            Msg::LeftViewport(viewport) => {
                self.left_viewport = viewport;
                // an unlinked pane must not repaint when the left window moves:
                // the toggle really unlinks it (the tier-A/manual gate checks the
                // right canvas hash stays put while the left pane pans)
                if !self.linked {
                    return false;
                }
            }
            Msg::Wheel(event) => {
                // the linked pane follows the left window: gestures would be
                // overwritten on the next commit, so they are ignored
                if self.linked {
                    return false;
                }
                event.prevent_default();
                let width = self.surface_width();
                if width <= 1.0 {
                    return false;
                }
                if event.ctrl_key() {
                    let factor = if event.delta_y() < 0.0 {
                        1.15
                    } else {
                        1.0 / 1.15
                    };
                    let mut view = self.effective_view(width);
                    view.zoom_at_px(event.offset_x() as f64, factor, width);
                    self.commit_view(view);
                } else {
                    let max = self.max_scroll();
                    self.scroll_top = (self.scroll_top + event.delta_y()).clamp(0.0, max);
                }
                return true;
            }
            Msg::DragStart(event) => {
                if self.linked || event.button() != 0 {
                    return false;
                }
                let view = self.effective_view(self.surface_width());
                self.drag = Some((event.client_x() as f64, view.x_axis));
                self.preview = None;
                return false;
            }
            Msg::DragMove(event) => {
                let Some((start_x, start_axis)) = self.drag else {
                    return false;
                };
                let view = self.effective_view(self.surface_width());
                let size = if view.size.is_finite() && view.size > 1e-9 {
                    view.size
                } else {
                    1e-9
                };
                let dx = event.client_x() as f64 - start_x;
                self.preview = Some(View::new((start_axis - dx / size).max(0.0), size));
                return true;
            }
            Msg::DragEnd => {
                let preview = self.preview.take();
                self.drag = None;
                if let Some(view) = preview {
                    self.commit_view(view);
                }
                return true;
            }
            Msg::Resize => return true,
        }
        true
    }

    /// The linked toggle must reach the pane: on linked -> unlinked, freeze the
    /// window the pane is showing right now as its own committed viewport, so it
    /// does not jump to a freshly fitted default on the next redraw.
    fn changed(&mut self, ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
        let linked = ctx.props().linked;
        if linked != self.linked {
            if !linked {
                let width = self.surface_width();
                self.fitted.set(true);
                if width > 1.0 {
                    let frozen = self.effective_view(width);
                    // use it for the immediate repaint too: the store message
                    // that carries it back arrives after this render
                    self.viewport.view = frozen;
                    self.preview = None;
                    self.commit_view(frozen);
                }
            }
            self.linked = linked;
        }
        true
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let Some(observer) = self.resize_observer.take() {
            observer.disconnect();
        }
        self.resize_callback = None;
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first: bool) {
        // the canvas box can change with no props changing (window resize, the
        // divider drag writes a css variable), so watch it like WaveShow does
        self.observe_canvas(ctx);
        self.redraw();
        // the box can settle after the paint (a divider drag or a layout pass):
        // ask for exactly one more render when the surface moved, so the backing
        // store never keeps the previous size
        if let Some(canvas) = self.canvas_ref.cast::<web_sys::HtmlCanvasElement>() {
            let rect = canvas.get_bounding_client_rect();
            if (rect.width() - self.painted_w.get()).abs() > 0.5
                || (rect.height() - self.painted_h.get()).abs() > 0.5
            {
                ctx.link().send_message(Msg::Resize);
            }
        }
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        let name = self
            .store
            .select(move |state| select::dump_name_of(state, self.dump))
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "dump".to_string());
        html! {
            <div class="split-pane" aria-label={format!("Split pane: {name}")}
                title={self.painted_view.get().map(|(axis, size)| format!("window {axis:.2} @ {size:.6}")).unwrap_or_default()}
                data-frame={self.painted_frame.borrow().clone().unwrap_or_default()}>
                <div class="split-pane-head">
                    <span class="split-pane-name" title={name.clone()}>{name.clone()}</span>
                    <span class="split-pane-mode">{"read only"}</span>
                </div>
                <div class="split-canvas-wrap"
                    onwheel={_ctx.link().callback(Msg::Wheel)}
                    onmousedown={_ctx.link().callback(Msg::DragStart)}
                    onmousemove={_ctx.link().callback(Msg::DragMove)}
                    onmouseup={_ctx.link().callback(|_| Msg::DragEnd)}
                    onmouseleave={_ctx.link().callback(|_| Msg::DragEnd)}>
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
        self.surface_h = height;
        // the same virtualized window the left pane uses: only the rows that
        // intersect the surface are drawn/decoded
        let (draw, fetch) = visible_rows(self.scroll_top, height, rows);
        self.decode_pending(fetch);

        let view = self.effective_view(width);
        let axis = build_axis(view.x_axis, view.size, width);
        let visible: Vec<(usize, &super::util::signal_things::SignalItem)> = (draw.start..draw.end)
            .filter_map(|row| self.signal_things.iter().nth(row).map(|item| (row, item)))
            .collect();
        let (cursor1, cursor2) = self.mapped_cursors();
        let frame = canvas::Frame {
            view,
            width,
            height,
            scroll_top: self.scroll_top,
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
        self.painted_w.set(width);
        self.painted_h.set(height);
        self.painted_view.set(Some((view.x_axis, view.size)));
        *self.painted_frame.borrow_mut() = Some(format!(
            "view={:.4}:{:.6} cursors={:?}/{:?} scroll={:.1} dpr={:.2} rows={} first={} end={:?} w={:.1}",
            view.x_axis,
            view.size,
            cursor1,
            cursor2,
            self.scroll_top,
            frame.dpr,
            visible.len(),
            draw.start,
            frame.trace_end,
            width
        ));
    }

    /// The window this pane shows: the mapped left window when linked, its own
    /// committed viewport otherwise (fitted to the trace on the first paint).
    fn effective_view(&self, width: f64) -> View {
        // a drag-pan in progress shows its live view without a store write
        if !self.linked {
            if let Some(preview) = self.preview {
                return preview;
            }
        }
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
                // the first unlinked paint fits the trace; commit it, otherwise
                // the next render would fall back to the default 1 px/time view
                self.fitted.set(true);
                let view = View::new(0.0, View::fit_size(end, width));
                self.commit_view(view);
                return view;
            }
        }
        own
    }

    /// Repaint whenever the canvas box changes, whatever changed it.
    ///
    /// `rendered` only runs when the component re-renders; a window resize (or
    /// the divider drag, which is tier-A) changes the box with no props change,
    /// and without this the backing store keeps the old size and the trace is
    /// stretched/blurry until some unrelated store event repaints it.
    fn observe_canvas(&mut self, ctx: &Context<Self>) {
        if self.resize_observer.is_some() {
            return;
        }
        let Some(target) = self.canvas_ref.cast::<web_sys::Element>() else {
            return;
        };
        let link = ctx.link().clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            link.send_message(Msg::Resize);
        });
        let Ok(observer) = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&target);
        self.resize_observer = Some(observer);
        self.resize_callback = Some(callback);
    }

    /// The measured canvas width (css pixels), `0` until the first paint.
    fn surface_width(&self) -> f64 {
        self.canvas_ref
            .cast::<web_sys::HtmlCanvasElement>()
            .map(|canvas| canvas.get_bounding_client_rect().width())
            .unwrap_or(0.0)
    }

    /// The largest row scroll offset that still shows a full surface.
    fn max_scroll(&self) -> f64 {
        state::split::max_row_scroll(self.signal_things.len(), self.surface_h)
    }

    /// Commit one viewport to this dump's own entry (the unlinked pane's path).
    fn commit_view(&self, view: View) {
        self.store.dispatch(Action::ViewCommittedFor {
            dump: self.dump,
            view,
        });
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
