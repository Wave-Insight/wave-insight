use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wave_insight_lib::waveform::TimeRange;
use yew::prelude::*;

use crate::state::DumpHandle;
use std::cell::Cell;

use crate::state::{self, select, Cursors, RowsSlice, Store, Subscription, View, Viewport};

use super::canvas;
use super::ctrl::Ctrl;
use super::geometry::{self, content_height, visible_rows, ROW_HEIGHT, Time};
use super::settings::Settings;
use super::signal::SignalName;
use super::time::{build_axis, Time as TimeBar};
use super::util::signal_things::{RowValues, SignalItem, SignalThings};

/// Decoded histories kept in memory before rows outside the viewport are
/// dropped again. The backend charges ~40 bytes per decoded change.
const CACHE_BUDGET_BYTES: usize = 64 * 1024 * 1024;
/// Rows around the viewport that are kept decoded once they were shown, so a
/// scroll back and forth does not decode again. In row units.
const KEEP_ROWS: usize = geometry::PREFETCH_ROWS * 4;
/// Rows outside the viewport that are rendered into the DOM name column, so a
/// fast scroll does not pop.
const NAME_OVERSCAN: usize = 4;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct WaveShowProps {
    /// The app store: the rows (order, settings, selection) are subscribed from
    /// here, and every row change is dispatched back as an action (R2 step 2 B1).
    /// The opened dump lives in the store too (R2 step 3), so the pane takes no
    /// waveform prop any more.
    pub store: Store,

    /// The selected signal, for the driver/load panel.
    #[prop_or_default]
    pub onselect: Callback<Option<String>>,
    /// Open the declaration of one signal in the source pane.
    #[prop_or_default]
    pub onjump: Callback<String>,
    /// The settings dialog asked for a driver/load: `(load?, index, path)`.
    /// The app resolves the span and decides between "jump to the one target"
    /// and "the several are listed in the inspector".
    #[prop_or_default]
    pub onlocate: Callback<(bool, usize, String)>,
}

pub enum Msg {
    /// The store's rows changed (add/remove/reorder/settings/selection).
    Rows(RowsSlice),
    /// The committed viewport/cursors changed (this is where a gesture lands).
    Viewport(Viewport),
    /// The opened dump's time range changed.
    TimeRange(Option<TimeRange>),
    /// The opened dump changed (the store owns the handle).
    Dump(Option<DumpHandle>),
    Wheel(WheelEvent),
    /// the row list was scrolled
    Scroll,
    /// the label-column splitter was grabbed
    NamesDown(MouseEvent),
    /// the splitter moved (tier A: writes the css variable, no dispatch)
    NamesMove(MouseEvent),
    NamesUp,
    ShowMenu(usize),
    SetSignal((bool, Settings)),
    DeleteSig,
    KeyEvent(u32),
    MouseDown(MouseEvent),
    MouseMove(MouseEvent),
    MouseUp(MouseEvent),
    NameClick(usize),
    /// the little arrow on a name cell: open its declaration
    Jump(usize),
    /// the settings dialog asked for a load (`true`) or a driver (`false`)
    Locate((bool, usize)),
    /// the layout moved, everything has to be measured again
    Resize,
    /// rows were decoded after the last render: the value column has values to
    /// show now, so re-render the name column once (see `rendered`)
    ValuesReady,
}

/// Geometry of the wave area, measured from the dom.
///
/// It is measured instead of guessed, so it stays correct with a fixed width
/// sidebar, a zoomed page, a scrollbar appearing/disappearing or a high dpi
/// screen. `surface_*` is the wave canvas, `body_top` is the top of the
/// scrolling row list. The canvas itself is placed by css (`--names-w`,
/// `--timebar-h`, `--scrollbar-w`), so there is nothing to position here.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Metrics {
    surface_w: f64,
    surface_h: f64,
    surface_left: f64,
    body_top: f64,
    dpr: f64,
}

impl Metrics {
    fn is_usable(&self) -> bool {
        self.surface_w > 1.0 && self.surface_h > 1.0
    }

    fn approximately(&self, other: &Metrics) -> bool {
        (self.surface_w - other.surface_w).abs() <= 0.5
            && (self.surface_h - other.surface_h).abs() <= 0.5
            && (self.surface_left - other.surface_left).abs() <= 0.5
            && (self.body_top - other.body_top).abs() <= 0.5
            && (self.dpr - other.dpr).abs() <= 0.001
    }
}

/// Which cursor a press that landed on a cursor line is dragging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorDrag {
    One,
    Two,
}

pub struct WaveShow {
    /// The app store the rows are read from and dispatched to.
    store: Store,
    signal_things: SignalThings,
    /// The dump the rows are decoded from.
    waveform: Option<DumpHandle>,
    /// The rows the store owns, as the last snapshot the pane saw.
    rows: RowsSlice,
    /// The committed cursors, mirrored here for drawing.
    cursors: Cursors,
    /// Cursors being dragged: a local preview, committed on mouseup (tier A).
    cursor_preview: Option<Cursors>,
    /// Which cursor the current press grabbed, if the press drags one.
    cursor_drag: Option<CursorDrag>,
    /// The value-change edges on screen when the press started; cursors snap to
    /// these. Collected once per gesture, never per mouse move.
    snap_edges: Vec<Time>,
    /// Rows were decoded after the last render, so the value column has new
    /// cell values: one extra render is sent from `rendered`.
    values_need_render: bool,
    /// The opened dump's range, mirrored here for the ruler and the fit zoom.
    time_range: Option<TimeRange>,
    /// Kept so the subscriptions live exactly as long as the component.
    _rows_sub: Subscription,
    _viewport_sub: Subscription,
    _dump_sub: Subscription,
    _waveform_sub: Subscription,
    _layout_sub: Subscription,
    /// The pane's own re-render count, published for the tier-A QA gate.
    pane_render_count: Cell<u32>,

    /// Zoom/pan state of the wave body.
    view: View,
    /// `true` once the fit-to-trace zoom was applied for this dump.
    fitted: bool,
    /// `true` once the computed fit zoom was also committed to the store.
    ///
    /// The fit is applied locally in [`Self::redraw`] (it needs the measured
    /// width), but the store owns the committed viewport: without this commit a
    /// later cursor commit would hand back the store's default view and reset
    /// the zoom, and the time bar would draw a ruler for a view nobody sees.
    fit_committed: bool,
    /// Vertical scroll offset of the row list, in css pixels.
    scroll_top: f64,

    /// Width of the label column, dragged by the splitter at its right edge.
    names_w: f64,
    /// Splitter drag: pointer x and width at the press, plus the live width.
    names_drag: Option<(f64, f64)>,
    names_preview: Cell<f64>,

    menu_show: bool,
    on_setting_idx: Vec<usize>,

    key_press: Rc<RefCell<Vec<bool>>>,

    /// Mouse drag in the wave area: `mouse_start_x` is where the button went
    /// down, `mouse_current_x` follows the pointer for the selection band.
    mouse_press: bool,
    mouse_start_x: f64,
    mouse_current_x: f64,

    name_choose_valid: bool,
    name_choose: usize,
    /// where the mouse went down on the names column, so a plain click (which
    /// also fires mousedown/mouseup) is not mistaken for a reorder drag
    name_start_y: f64,


    /// what the last `view` was measured with, so `rendered` can notice a moved layout
    used: RefCell<Metrics>,
    /// rows that were decoded by the last redraw, for the debug hook
    last_decoded: usize,
    draws: u64,

    /// the listeners are created once and dropped in `destroy`; forgetting them
    /// on every render leaked one set of them per render
    keydown: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    keyup: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    onresize: Option<Closure<dyn FnMut(JsValue)>>,
}

impl Component for WaveShow {
    type Message = Msg;
    type Properties = WaveShowProps;

    fn create(ctx: &Context<Self>) -> Self {
        let store = ctx.props().store.clone();
        let rows_sub = store.subscribe(select::rows, ctx.link().callback(Msg::Rows));
        let viewport_sub =
            store.subscribe(select::viewport, ctx.link().callback(Msg::Viewport));
        let dump_sub = store.subscribe(select::time_range, ctx.link().callback(Msg::TimeRange));
        // the dump handle is store state now: no prop, one subscription
        let waveform_sub =
            store.subscribe(select::dump_waveform, ctx.link().callback(Msg::Dump));
        // The pane's height depends on the editor ratio: when the source pane
        // expands (for example a dump was loaded before any source), the wave
        // pane shrinks without any props changing, so without this subscription
        // the canvas keeps its old backing store size and the drawing is drawn
        // squashed until something else forces a redraw.
        let layout_sub = store.subscribe(select::layout, ctx.link().callback(|_| Msg::Resize));
        let mut this = Self {
            store: store.clone(),
            signal_things: SignalThings::new(),
            waveform: store.select(select::dump_waveform),
            rows: store.select(select::rows),
            cursors: store.select(select::cursors),
            cursor_preview: None,
            cursor_drag: None,
            snap_edges: Vec::new(),
            values_need_render: false,
            time_range: store.select(select::time_range),
            _rows_sub: rows_sub,
            _viewport_sub: viewport_sub,
            _dump_sub: dump_sub,
            _waveform_sub: waveform_sub,
            _layout_sub: layout_sub,
            pane_render_count: Cell::new(0),

            view: View::default(),
            fitted: false,
            fit_committed: false,
            scroll_top: 0.0,

            menu_show: false,
            on_setting_idx: Vec::new(),

            key_press: Rc::new(RefCell::new(vec![false; 256])),

            mouse_press: false,
            mouse_start_x: 0.0,
            mouse_current_x: 0.0,

            name_choose_valid: false,
            name_choose: 0,
            name_start_y: 0.0,


            used: RefCell::new(Metrics::default()),
            last_decoded: 0,
            draws: 0,
            names_w: 330.0,
            names_drag: None,
            names_preview: Cell::new(330.0),

            keydown: None,
            keyup: None,
            onresize: None,
        };
        this.sync_rows();
        this.attach_listeners(ctx);
        this
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let changed = self.handle(ctx, msg);
        if changed {
            self.pane_render_count
                .set(self.pane_render_count.get().wrapping_add(1));
        }
        changed
    }

    fn changed(&mut self, _ctx: &Context<Self>) -> bool {
        // every slice the pane reads (rows, viewport, dump, time range) arrives
        // through its own subscription; a prop change alone never redraws
        false
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        let measured = self.measure();
        if measured.is_usable() && !measured.approximately(&self.used.borrow()) {
            ctx.link().send_message(Msg::Resize);
        }
        *self.used.borrow_mut() = measured;
        // Publish *before* the repaint: `redraw` copies the counter into
        // `window.__waveDebug`, and publishing afterwards made the tier-A gate
        // see every render one repaint late (the count moved inside the next
        // gesture window instead of at its own commit).
        self.publish_pane_render_count();
        self.redraw();
        // the fit zoom is applied on the first real measurement; commit it once
        // so the store, the ruler and every later commit agree on the viewport
        if self.fitted && !self.fit_committed {
            self.fit_committed = true;
            self.dispatch(state::Action::ViewCommitted(self.view));
        }
        // Rows decoded by that redraw now have values for the value column. A
        // render happens *before* `rendered`, so the column would show blanks
        // after a scroll until the next unrelated render; ask for exactly one
        // more render (never one per mouse move).
        if self.values_need_render {
            self.values_need_render = false;
            ctx.link().send_message(Msg::ValuesReady);
        }
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let Some(window) = web_sys::window() {
            if let Some(keydown) = self.keydown.as_ref() {
                let _ = window.remove_event_listener_with_callback(
                    "keydown",
                    keydown.as_ref().unchecked_ref(),
                );
            }
            if let Some(keyup) = self.keyup.as_ref() {
                let _ = window.remove_event_listener_with_callback(
                    "keyup",
                    keyup.as_ref().unchecked_ref(),
                );
            }
            if let Some(onresize) = self.onresize.as_ref() {
                let _ = window.remove_event_listener_with_callback(
                    "resize",
                    onresize.as_ref().unchecked_ref(),
                );
            }
        }
        self.keydown = None;
        self.keyup = None;
        self.onresize = None;
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let metrics = self.measure();
        let rows = self.signal_things.len();
        let (draw, _fetch) = visible_rows(self.scroll_top, metrics.surface_h, rows);
        let has_signal = rows > 0;

        //virtual name column: only the rows around the viewport exist in the dom
        let name_first = draw.start.saturating_sub(NAME_OVERSCAN);
        let name_last = (draw.end + NAME_OVERSCAN).min(rows);

        let rows_height = format!("{:.1}px", content_height(rows));

        html! {
            <div class="wave-pane" id="wave-pane"
                // The value column lives in the label area, so the shared label
                // width (used by the name cells, the canvas and the ruler) is
                // adjustable: the splitter writes it during the drag and it is
                // committed here on mouseup.
                style={format!("--names-w: {:.0}px", self.names_w)}>
                if self.menu_show && !self.on_setting_idx.is_empty() {
                    <Ctrl name={self.signal_things[self.on_setting_idx[0]].name.clone()}
                        setting={self.signal_things[self.on_setting_idx[0]].setting.clone()}
                        load={self.signal_things[self.on_setting_idx[0]].load.clone()}
                        drive={self.signal_things[self.on_setting_idx[0]].driver.clone()}
                        onset={link.callback(Msg::SetSignal)}
                        onlocate={link.callback(Msg::Locate)}
                        delete={link.callback(|_| Msg::DeleteSig)} />
                }
                <TimeBar store={self.store.clone()} width={metrics.surface_w} />
                <div class="wave-body" id="wave-body"
                    onscroll={link.callback(|_| Msg::Scroll)}
                    onwheel={link.callback(Msg::Wheel)}
                    onmousedown={link.callback(Msg::MouseDown)}
                    onmouseup={link.callback(Msg::MouseUp)}
                    onmousemove={link.callback(Msg::MouseMove)}
                    // right click sets the second cursor; the browser menu
                    // must not open on top of the wave area
                    oncontextmenu={Callback::from(|event: MouseEvent| event.prevent_default())}>
                    <div class="wave-rows" style={format!("height:{rows_height}")}>
                    <div class="wave-names-splitter"
                        onmousedown={link.callback(Msg::NamesDown)} />
                    {if self.names_drag.is_some() {
                        html!{
                            <div class="splitter-shield"
                                onmousemove={link.callback(Msg::NamesMove)}
                                onmouseup={link.callback(|_| Msg::NamesUp)}
                                onmouseout={link.callback(|_| Msg::NamesUp)} />
                        }
                    }else {
                        html!{}
                    }}
                        {
                            for (name_first..name_last).map(|idx| {
                                let item = &self.signal_things[idx];
                                // the cursor values live in the label column now,
                                // not on the canvas next to the cursor line
                                let value = self.value_text_at(item, self.cursors.one);
                                let value2 = self
                                    .cursors
                                    .two
                                    .map(|_| self.value_text_at(item, self.cursors.two).unwrap_or_default());
                                html!{<SignalName
                                    name={item.name.clone()}
                                    choose={item.choose}
                                    top={idx as f64 * ROW_HEIGHT}
                                    value={value}
                                    value2={value2}
                                    menu={link.callback(move |()| Msg::ShowMenu(idx))}
                                    onjump={link.callback(move |()| Msg::Jump(idx))}
                                    onclick={link.callback(move |()| Msg::NameClick(idx))}/>}
                            })
                        }
                    </div>
                </div>
                <div class="wave-canvas" id="wave-canvas"
                    // the canvas overlay has `pointer-events: none`, so a real
                    // right click lands on .wave-body; this covers synthetic
                    // events and any future change to that, and keeps the
                    // browser menu off the waveform either way
                    oncontextmenu={Callback::from(|event: MouseEvent| event.prevent_default())}>
                    <canvas id="wave-surface" class="wave-surface"></canvas>
                    {if has_signal {html!{}} else {html!{
                        <div class="empty-hint">
                            <div>{"No signals yet"}</div>
                            <div>{"Pick one from the signal list, drag a range to zoom, "}<br/>
                                {"click a name in the source to add it here."}</div>
                        </div>
                    }}}
                </div>
            </div>
        }
    }
}

impl WaveShow {
    /// The real message handling; the trait's `update` only wraps it so the
    /// QA seam can count the pane's re-renders (R2 step 2 B2).
    fn handle(&mut self, ctx: &Context<Self>, msg: Msg) -> bool {
        match msg {
            Msg::Rows(rows) => {
                self.rows = rows;
                self.sync_rows();
                // the name column, the settings and the canvas all follow the store
                true
            }
            Msg::Viewport(viewport) => {
                // a committed gesture landed: mirror it and repaint the canvas.
                // A zoom/pan needs no Yew render — the time bar has its own
                // subscription and the row list did not change (tier A/C) — but
                // the value column shows the cursor values, so a *cursor* commit
                // has to re-render the name column once.
                let cursors_changed = viewport.cursors != self.cursors;
                self.view = viewport.view;
                self.cursors = viewport.cursors;
                if cursors_changed {
                    return true;
                }
                self.redraw();
                false
            }
            Msg::TimeRange(time_range) => {
                self.time_range = time_range;
                // the ruler and the fit zoom read it
                true
            }
            Msg::Dump(waveform) => {
                // a new dump means every row has to be resolved and decoded
                // again; the handle itself lives in the store now
                if self.waveform != waveform {
                    self.waveform = waveform;
                    self.fitted = false;
                    self.fit_committed = false;
                    self.signal_things
                        .refresh(self.waveform.as_ref().map(|waveform| waveform.waveform()));
                    self.scroll_top = scroll_top_from_dom();
                    self.sync_rows();
                    true
                } else {
                    false
                }
            }
            Msg::Wheel(e) => {
                let metrics = self.measure();
                // over the name column the wheel scrolls the rows, like any list
                if (e.x() as f64) < metrics.surface_left {
                    return false;
                }
                e.prevent_default();
                let delta_y = e.delta_y();
                let mut view = self.view;
                if self.key(17) {
                    //ctrl
                    if delta_y < 0.0 {
                        view.zoom(1.25);
                    } else {
                        view.zoom(0.8);
                    }
                } else if delta_y > 0.0 {
                    view.pan_px(30.0);
                } else {
                    view.pan_px(-30.0);
                }
                // one dispatch per notch; the canvas is repainted from the local
                // frame, the time bar re-renders from its own subscription
                self.commit_view(view);
                false
            }
            Msg::Scroll => {
                self.scroll_top = scroll_top_from_dom();
                true
            }
            Msg::ShowMenu(idx) => {
                self.menu_show = true;
                let choosed_idx = self.signal_things.get_choose_idx();
                // The button belongs to one row: unless that row is part of the
                // selection, its settings are what the dialog must show (the
                // dialog used to open for whatever row happened to be selected).
                if choosed_idx.contains(&idx) {
                    self.on_setting_idx = choosed_idx;
                } else {
                    self.dispatch(state::Action::RowSelected { index: idx, ctrl: false, shift: false });
                    self.on_setting_idx = vec![idx];
                    // the snapshot arrives through the subscription; settings read
                    // the next frame, exactly like before
                    self.sync_rows();
                }
                true
            }
            Msg::SetSignal((close, set)) => {
                if close {
                    self.menu_show = false;
                }
                self.dispatch(state::Action::RowSettingChanged {
                    indices: self.on_setting_idx.clone(),
                    setting: set,
                });
                true
            }
            Msg::DeleteSig => {
                self.menu_show = false;
                let indices = std::mem::take(&mut self.on_setting_idx);
                self.dispatch(state::Action::RowsRemoved { indices });
                ctx.props().onselect.emit(None);
                true
            }
            Msg::NamesDown(e) => {
                e.prevent_default();
                self.names_drag = Some((e.x() as f64, self.names_w));
                self.names_preview.set(self.names_w);
                true
            }
            Msg::NamesMove(e) => {
                if let Some((start_x, start_w)) = self.names_drag {
                    let width = (start_w + (e.x() as f64 - start_x)).clamp(200.0, 640.0);
                    self.names_preview.set(width);
                    // tier A: write the css variable directly, no re-render
                    if let Some(element) = web_sys::window()
                        .and_then(|window| window.document())
                        .and_then(|document| document.get_element_by_id("wave-pane"))
                    {
                        if let Ok(style) = js_sys::Reflect::get(&element, &JsValue::from_str("style")) {
                            if let Ok(set_property) =
                                js_sys::Reflect::get(&style, &JsValue::from_str("setProperty"))
                                    .and_then(|function| function.dyn_into::<js_sys::Function>())
                            {
                                let args = js_sys::Array::new();
                                args.push(&JsValue::from_str("--names-w"));
                                args.push(&JsValue::from_str(&format!("{width:.0}px")));
                                let _ = set_property.apply(&style, &args);
                            }
                        }
                    }
                }
                false
            }
            Msg::NamesUp => {
                if self.names_drag.take().is_some() {
                    self.names_w = self.names_preview.get();
                }
                true
            }
            Msg::KeyEvent(idx) => {
                let mut view = self.view;
                if idx == 90 {
                    //z
                    if self.key(16) {
                        //shift
                        view.zoom(1.25 * 1.25);
                    } else {
                        view.zoom(0.64);
                    }
                } else if idx == 70 {
                    //f
                    let metrics = self.measure();
                    let end = self.trace_end(self.time_range);
                    view = View::new(0.0, View::fit_size(end, metrics.surface_w));
                    self.fitted = true;
                } else if idx == 46 || idx == 8 {
                    //delete / backspace removes the selected rows
                    let indices: Vec<usize> = self
                        .rows
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.chosen)
                        .map(|(index, _)| index)
                        .collect();
                    if !indices.is_empty() {
                        self.dispatch(state::Action::RowsRemoved { indices });
                        ctx.props().onselect.emit(None);
                        return true;
                    }
                }
                self.commit_view(view);
                false
            }
            Msg::MouseDown(e) => {
                e.prevent_default();
                let metrics = self.measure();
                let x = e.x() as f64;
                if x < metrics.surface_left {
                    //the name column: start a reorder drag
                    self.name_choose_valid = true;
                    self.name_start_y = e.y() as f64;
                    self.name_choose = self.row_at(e.y() as f64);
                    return false;
                }
                // the edges the cursors can snap to are frozen at the press; a
                // drag of the view must not re-collect them per mouse move
                self.snap_edges = self.collect_snap_edges(&metrics);
                let local_x = x - metrics.surface_left;
                // pressing on a cursor line grabs it (the Verdi gesture: drag
                // the line to move the cursor); anywhere else starts a zoom band
                if let Some(which) = self.cursor_at(local_x) {
                    self.cursor_drag = Some(which);
                    self.cursor_preview = None;
                    self.mouse_press = false;
                    return false;
                }
                self.cursor_drag = None;
                self.cursor_preview = None;
                self.mouse_press = true;
                self.mouse_start_x = local_x;
                self.mouse_current_x = local_x;
                // tier A: the band is a local preview, never a Yew render
                self.redraw();
                false
            }
            Msg::MouseMove(e) => {
                if let Some(which) = self.cursor_drag {
                    let metrics = self.measure();
                    let x = e.x() as f64 - metrics.surface_left;
                    let time = self.snapped_time(x);
                    let mut preview = self.cursor_preview.unwrap_or(self.cursors);
                    match which {
                        CursorDrag::One => preview.one = Some(time),
                        CursorDrag::Two => preview.two = Some(time),
                    }
                    self.cursor_preview = Some(preview);
                    // tier A: repaint the dragged line, no dispatch and no render
                    self.redraw();
                    return false;
                }
                if !self.mouse_press {
                    return false;
                }
                let metrics = self.measure();
                self.mouse_current_x = e.x() as f64 - metrics.surface_left;
                // tier A: repaint the band straight on the canvas, no Yew render
                self.redraw();
                false
            }
            Msg::MouseUp(e) => {
                e.prevent_default();
                if let Some(which) = self.cursor_drag.take() {
                    let preview = self.cursor_preview.take().unwrap_or(self.cursors);
                    self.snap_edges.clear();
                    if preview == self.cursors {
                        // a press without a move is still a click: it clears
                        // the cursor it landed on
                        let mut cursors = self.cursors;
                        match which {
                            CursorDrag::One => cursors.one = None,
                            CursorDrag::Two => cursors.two = None,
                        }
                        self.commit_cursors(cursors);
                    } else {
                        // one dispatch for the whole drag, on mouseup
                        self.commit_cursors(preview);
                    }
                    // one render after the commit, so the value column follows
                    // the moved cursor; the drag itself never rendered
                    return true;
                }
                if self.name_choose_valid {
                    self.name_choose_valid = false;
                    // A click also fires mousedown and mouseup; only a real drag
                    // reorders. Without this every click on a name swapped it
                    // with its neighbour, because the drop position rounded to
                    // the next row.
                    if (e.y() as f64 - self.name_start_y).abs() < ROW_HEIGHT / 2.0 {
                        return true;
                    }
                    let metrics = self.measure();
                    let location =
                        (e.y() as f64 - metrics.body_top + self.scroll_top) / ROW_HEIGHT;
                    if location < 0.0 {
                        return true;
                    }
                    let current = self.name_choose;
                    let dest = if location > current as f64 + 0.5 {
                        (location - 0.5) as usize
                    } else {
                        (location + 0.5) as usize
                    };
                    if current < self.rows.len() && dest < self.rows.len() && dest != current {
                        self.dispatch(state::Action::RowsReordered { from: current, to: dest });
                    }
                    return true;
                }
                if !self.mouse_press {
                    return false;
                }
                self.mouse_press = false;
                let metrics = self.measure();
                let x = e.x() as f64 - metrics.surface_left;
                if (self.mouse_start_x - x).abs() <= 2.0 {
                    //a click, not a drag: set (or clear) one of the cursors,
                    //snapped to the closest value edge on screen
                    let cursor_time = self.snapped_time(x);
                    self.snap_edges.clear();
                    let mut cursors = self.cursors;
                    if e.button() == 0 {
                        cursors.one = if cursors.one == Some(cursor_time) {
                            None
                        } else {
                            Some(cursor_time)
                        };
                    } else {
                        cursors.two = if cursors.two == Some(cursor_time) {
                            None
                        } else {
                            Some(cursor_time)
                        };
                    }
                    self.commit_cursors(cursors);
                    // one render after the commit, for the value column (tier A
                    // only forbids renders while the pointer moves)
                    true
                } else {
                    //drag a range to zoom into it
                    let mut view = self.view;
                    view.zoom_to_px(self.mouse_start_x, x, metrics.surface_w);
                    self.commit_view(view);
                    false
                }
            }
            Msg::NameClick(idx) => {
                let ctrl = self.key(17);
                let shift = self.key(16);
                // the store owns the selection; read the path before it changes
                let path = self.signal_things.iter().nth(idx).map(|item| item.path.clone());
                self.dispatch(state::Action::RowSelected { index: idx, ctrl, shift });
                self.sync_rows();
                if let Some(path) = path {
                    // the inspector follows the last clicked row
                    ctx.props().onselect.emit(Some(path.clone()));
                    if !ctrl && !shift {
                        // plain click = "show me where this is declared"
                        ctx.props().onjump.emit(path);
                    }
                }
                true
            }
            Msg::Jump(idx) => {
                if let Some(item) = self.signal_things.iter().nth(idx) {
                    let path = item.path.clone();
                    ctx.props().onselect.emit(Some(path.clone()));
                    ctx.props().onjump.emit(path);
                }
                true
            }
            Msg::Locate((load, index)) => {
                // the dialog belongs to the first row of its target set; the app
                // resolves the driver/load span (or lists the several) from there
                let path = self
                    .on_setting_idx
                    .first()
                    .and_then(|idx| self.signal_things.iter().nth(*idx))
                    .map(|item| item.path.clone());
                if let Some(path) = path {
                    self.menu_show = false;
                    ctx.props().onlocate.emit((load, index, path));
                }
                true
            }
            Msg::Resize => true,
            // rows were decoded after the last render: the value column now has
            // the values to show
            Msg::ValuesReady => true,
        }
    }


    /// Commit a zoom/pan: mirror it for the current frame, dispatch it once and
    /// repaint. No Yew render — the canvas is the only thing that changes, and
    /// the time bar re-renders from its own store subscription.
    fn commit_view(&mut self, view: View) {
        self.view = view;
        self.dispatch(state::Action::ViewCommitted(view));
        self.redraw();
    }

    /// Commit the cursors (one dispatch per click, never per mousemove).
    fn commit_cursors(&mut self, cursors: Cursors) {
        self.cursors = cursors;
        self.dispatch(state::Action::CursorsChanged(cursors));
        self.redraw();
    }

    /// The cursors to draw: the drag preview while a cursor line is being moved.
    fn shown_cursors(&self) -> Cursors {
        self.cursor_preview.unwrap_or(self.cursors)
    }

    /// The formatted value of one row at a cursor, for the value column.
    ///
    /// `None` when the cursor is not set, the row has no decoded value (it is
    /// outside the decode window) or the value has no textual form (an event).
    fn value_text_at(&self, item: &SignalItem, cursor: Option<i32>) -> Option<String> {
        let cursor = cursor?;
        if cursor < 0 {
            return None;
        }
        let value = item.values.value_at(cursor as Time)?;
        canvas::value_text(value, item.format_width(), &item.setting)
    }

    /// The time a click at `x` (surface pixels) places a cursor at: the raw
    /// time, snapped onto a visible value edge when one is a few pixels away.
    fn snapped_time(&self, x: f64) -> i32 {
        let raw = self.view.time_at(x).max(0) as Time;
        let snapped = geometry::snap_to_edges(raw, &self.snap_edges, &self.view, geometry::SNAP_PX);
        snapped.min(i32::MAX as u64) as i32
    }

    /// The cursor whose line is under `x` (surface pixels), if any.
    fn cursor_at(&self, x: f64) -> Option<CursorDrag> {
        let mut best: Option<(f64, CursorDrag)> = None;
        for (cursor, which) in [
            (self.cursors.one, CursorDrag::One),
            (self.cursors.two, CursorDrag::Two),
        ] {
            let Some(cursor) = cursor else { continue };
            let distance = (self.view.x_of(cursor.max(0) as Time) - x).abs();
            if distance <= geometry::CURSOR_GRAB_PX
                && best.map_or(true, |(best_distance, _)| distance < best_distance)
            {
                best = Some((distance, which));
            }
        }
        best.map(|(_, which)| which)
    }

    /// The value-change edges of the rows on screen, sorted and de-duplicated.
    ///
    /// Collected once when a press starts, then reused for the whole gesture: a
    /// drag must not walk the decoded histories per mouse move (tier A).
    fn collect_snap_edges(&self, metrics: &Metrics) -> Vec<Time> {
        let rows = self.signal_things.len();
        let (draw, _) = visible_rows(self.scroll_top, metrics.surface_h, rows);
        let (start, end) = self.view.window(metrics.surface_w);
        let mut edges = Vec::new();
        self.signal_things.edges(draw, start, end, &mut edges);
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// Publish the pane's re-render count next to the shell's, for the QA gate.
    fn publish_pane_render_count(&self) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let count = JsValue::from_f64(f64::from(self.pane_render_count.get()));
        let _ = js_sys::Reflect::set(
            &window,
            &JsValue::from_str("__wavePaneRenderCount"),
            &count,
        );
    }

    /// One store change; the resulting snapshot arrives as `Msg::Rows`.
    fn dispatch(&self, action: state::Action) {
        let effects = self.store.dispatch(action);
        debug_assert!(effects.is_empty(), "R2 step 2 has no effects yet");
    }

    /// Map the store's rows onto the render model, keeping decoded values for
    /// rows that survived and unloading the ones that are gone.
    fn sync_rows(&mut self) {
        let waveform = self.waveform.as_ref().map(|waveform| waveform.waveform());
        let unload = self.signal_things.sync(&self.rows.items, waveform);
        if let Some(waveform) = waveform {
            for index in unload {
                let _ = waveform.unload_signal(index);
            }
        }
    }


    fn key(&self, code: usize) -> bool {
        self.key_press.borrow().get(code).copied().unwrap_or(false)
    }

    /// End of the trace for the fit zoom: the cheap range when the dump knows
    /// one, otherwise the newest decoded history (GHW has no cheap range).
    fn trace_end(&self, time_range: Option<TimeRange>) -> i64 {
        if let Some(range) = time_range {
            return range.end as i64;
        }
        self.signal_things
            .iter()
            .filter_map(|item| match &item.values {
                RowValues::Backend(decoded) => decoded.span().map(|(_, end)| end as i64),
                RowValues::Empty => None,
            })
            .max()
            .unwrap_or(0)
    }

    /// Row index under a client y coordinate (including the scroll offset).
    fn row_at(&self, client_y: f64) -> usize {
        let metrics = self.measure();
        let content_y = client_y - metrics.body_top + self.scroll_top;
        if content_y <= 0.0 {
            0
        } else {
            (content_y / ROW_HEIGHT).floor() as usize
        }
    }

    /// Decode the rows that are (or are about to be) on screen, in one call.
    ///
    /// `decoded_changes_many` reads a whole set of signals per pass over the
    /// body, so asking for the visible rows one by one would re-read the dump
    /// once per row. Rows that are already decoded are not touched.
    fn decode_pending(&mut self, fetch: std::ops::Range<usize>) {
        let Some(waveform) = self.waveform.as_ref().map(|waveform| waveform.waveform()) else {
            self.last_decoded = 0;
            return;
        };
        let pending = self.signal_things.pending(fetch);
        if pending.is_empty() {
            self.last_decoded = 0;
            return;
        }
        let indices: Vec<usize> = pending.iter().map(|(_, index)| *index).collect();
        match waveform.decoded_changes_many(&indices) {
            Ok(decoded) => {
                for ((row, _), handle) in pending.into_iter().zip(decoded) {
                    self.signal_things.set_backend(row, handle);
                }
                self.last_decoded = indices.len();
            }
            Err(_error) => {
                //the rows keep their "no data" line; a later redraw retries
                self.last_decoded = 0;
            }
        }
    }

    /// Give the memory back when the decoded histories grow past the budget.
    ///
    /// Rows that are comfortably outside the viewport drop their handle and the
    /// backend cache entry; scrolling back re-decodes them in one batch.
    fn trim_cache(&mut self, draw: std::ops::Range<usize>) {
        let Some(waveform) = self.waveform.as_ref().map(|waveform| waveform.waveform()) else {
            return;
        };
        if waveform.cache_stats().decoded_bytes() <= CACHE_BUDGET_BYTES {
            return;
        }
        let rows = self.signal_things.len();
        let keep = draw.start.saturating_sub(KEEP_ROWS)..(draw.end + KEEP_ROWS).min(rows);
        for index in self.signal_things.drop_outside(keep) {
            let _ = waveform.unload_signal(index);
        }
    }

    /// Paint the wave canvas from the current state.
    fn redraw(&mut self) {
        let redraw_started = js_sys::Date::now();
        let metrics = self.measure();
        if !metrics.is_usable() {
            return;
        }
        let Some(canvas) = surface_element() else {
            return;
        };
        let rows = self.signal_things.len();
        let (draw, fetch) = visible_rows(self.scroll_top, metrics.surface_h, rows);
        let decode_started = js_sys::Date::now();
        self.decode_pending(fetch);
        let decode_ms = js_sys::Date::now() - decode_started;
        if self.last_decoded > 0 && (self.cursors.one.is_some() || self.cursors.two.is_some()) {
            // one render is needed for the value column to show the new decode
            self.values_need_render = true;
        }
        self.trim_cache(draw.clone());

        if !self.fitted {
            //first real measurement (and the first decoded rows): show the whole
            //trace. GHW has no cheap time range, so fall back to the span of the
            //histories we have.
            let end = self.trace_end(self.time_range);
            self.view = View::new(0.0, View::fit_size(end, metrics.surface_w));
            self.fitted = true;
        }

        let axis = build_axis(self.view.x_axis, self.view.size, metrics.surface_w);
        let visible: Vec<(usize, &SignalItem)> = (draw.start..draw.end)
            .filter_map(|row| self.signal_things.iter().nth(row).map(|item| (row, item)))
            .collect();
        let cursors = self.shown_cursors();

        let frame = canvas::Frame {
            view: self.view,
            width: metrics.surface_w,
            height: metrics.surface_h,
            scroll_top: self.scroll_top,
            rows: &visible,
            grid: &axis.labels,
            cursor1: cursors.one,
            cursor2: cursors.two,
            drag: if self.mouse_press
                && (self.mouse_current_x - self.mouse_start_x).abs() > 1.0
            {
                Some((self.mouse_start_x, self.mouse_current_x))
            } else {
                None
            },
            dpr: metrics.dpr,
        };
        let stats = canvas::paint(&canvas, &frame);
        self.draws += 1;
        let decoded_bytes = self
            .waveform
            .as_ref()
            .map(|waveform| waveform.waveform().cache_stats().decoded_bytes())
            .unwrap_or(0);
        canvas::publish_debug(&canvas::DebugInfo {
            redraw_ms: js_sys::Date::now() - redraw_started,
            decode_ms,
            stats,
            visible: visible.len(),
            decoded: self.last_decoded,
            decoded_bytes,
            total_rows: rows,
            scroll_top: self.scroll_top,
            first_row: draw.start,
            cursor1: cursors.one,
            cursor2: cursors.two,
            view: self.view,
        });
    }

    /// Read the layout back from the dom.
    fn measure(&self) -> Metrics {
        let Some(document) = document() else {
            return Metrics::default();
        };
        let dpr = web_sys::window()
            .map(|window| window.device_pixel_ratio())
            .unwrap_or(1.0);
        let surface = document
            .get_element_by_id("wave-canvas")
            .map(|element| element.get_bounding_client_rect());
        let body = document
            .get_element_by_id("wave-body")
            .map(|element| element.get_bounding_client_rect());
        let (surface_w, surface_h, surface_left) = match &surface {
            Some(rect) => (rect.width(), rect.height(), rect.left()),
            None => (0.0, 0.0, 0.0),
        };
        let body_top = body.as_ref().map(|rect| rect.top()).unwrap_or(0.0);
        Metrics {
            surface_w,
            surface_h,
            surface_left,
            body_top,
            dpr,
        }
    }

    fn attach_listeners(&mut self, ctx: &Context<Self>) {
        let window = match web_sys::window() {
            Some(window) => window,
            None => return,
        };

        //key state for the ctrl/shift modifiers and for `z`/`f`
        let key_press = Rc::clone(&self.key_press);
        let on_key = ctx.link().callback(Msg::KeyEvent);
        let keydown = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            let code = e.key_code();
            //keyCode goes past 255 with some layouts, do not index blindly
            if let Some(slot) = key_press.borrow_mut().get_mut(code as usize) {
                *slot = true;
            }
            // The zoom shortcuts are page wide, but typing `z` into the source
            // editor or a filter box must not zoom the waveform.
            if !typing_in_text_field(&e) {
                on_key.emit(code);
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        //`addEventListener`, not the `onkeydown` property: monaco installs its own
        //window handlers when the editor takes focus, and a property handler is
        //simply replaced (which is why Delete stopped working once the source
        //pane existed)
        let _ = window.add_event_listener_with_callback(
            "keydown",
            keydown.as_ref().unchecked_ref(),
        );

        let key_press = Rc::clone(&self.key_press);
        let keyup = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            if let Some(slot) = key_press.borrow_mut().get_mut(e.key_code() as usize) {
                *slot = false;
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        let _ = window.add_event_listener_with_callback("keyup", keyup.as_ref().unchecked_ref());

        let on_resize = ctx.link().callback(|_: ()| Msg::Resize);
        let onresize = Closure::wrap(Box::new(move |_: JsValue| {
            on_resize.emit(());
        }) as Box<dyn FnMut(JsValue)>);
        let _ = window.add_event_listener_with_callback("resize", onresize.as_ref().unchecked_ref());

        self.keydown = Some(keydown);
        self.keyup = Some(keyup);
        self.onresize = Some(onresize);
    }
}

fn document() -> Option<web_sys::Document> {
    web_sys::window().and_then(|window| window.document())
}

/// The canvas element the wave body is painted into.
fn surface_element() -> Option<web_sys::HtmlCanvasElement> {
    document()?
        .get_element_by_id("wave-surface")?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()
}

/// Current scroll offset of the row list.
fn scroll_top_from_dom() -> f64 {
    document()
        .and_then(|document| document.get_element_by_id("wave-body"))
        .map(|element| element.scroll_top() as f64)
        .unwrap_or(0.0)
}

/// Is the key event aimed at a text field (the source editor, a filter box, a
/// number input in the settings dialog)?
///
/// The wave area listens on the window, so without this a `z` typed into a
/// comment would also zoom the waveform.
fn typing_in_text_field(event: &web_sys::KeyboardEvent) -> bool {
    let Some(target) = event.target() else {
        return false;
    };
    let Ok(element) = target.dyn_into::<web_sys::Element>() else {
        return false;
    };
    let mut current = Some(element);
    while let Some(element) = current {
        let tag = element.tag_name().to_ascii_lowercase();
        if tag == "input" || tag == "textarea" || tag == "select" {
            return true;
        }
        if element.has_attribute("contenteditable") {
            return true;
        }
        // monaco keeps its input in a hidden textarea inside `.monaco-editor`;
        // this also covers the hover card and the tab strip focus
        if element
            .get_attribute("class")
            .map(|class| class.contains("monaco-editor"))
            .unwrap_or(false)
        {
            return true;
        }
        current = element.parent_element();
    }
    false
}
