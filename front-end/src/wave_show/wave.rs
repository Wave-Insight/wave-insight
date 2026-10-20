use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wave_insight_lib::data_struct::Signal;
use wave_insight_lib::waveform::TimeRange;
use yew::prelude::*;

use crate::nav::SharedWaveform;

use super::canvas;
use super::ctrl::Ctrl;
use super::geometry::{self, content_height, visible_rows, View, ROW_HEIGHT};
use super::settings::Settings;
use super::signal::SignalName;
use super::time::{build_axis, Time};
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
    pub signaladd: (String, Rc<Signal>),
    /// The opened dump; values are decoded from it on demand.
    #[prop_or_default]
    pub waveform: Option<SharedWaveform>,
    /// Start and end of the trace, from
    /// [`wave_insight_lib::waveform::Waveform::time_range_hint`]. The ruler and
    /// the fit-to-trace zoom use it; `None` means "unknown" (GHW).
    #[prop_or_default]
    pub time_range: Option<TimeRange>,
    /// Bumped by the app for every add; the row is pushed once per bump, so a
    /// row the user deleted is not re-added by the next unrelated re-render.
    #[prop_or_default]
    pub added_seq: u32,
    /// The selected signal, for the driver/load panel.
    #[prop_or_default]
    pub onselect: Callback<Option<String>>,
    /// Open the declaration of one signal in the source pane.
    #[prop_or_default]
    pub onjump: Callback<String>,
}

pub enum Msg {
    Wheel(WheelEvent),
    /// the row list was scrolled
    Scroll,
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
    /// the layout moved, everything has to be measured again
    Resize,
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

pub struct WaveShow {
    signal_things: SignalThings,
    /// The dump the rows are decoded from.
    waveform: Option<SharedWaveform>,
    /// Value of [`WaveShowProps::added_seq`] that was already turned into a row.
    added_seq: u32,

    /// Zoom/pan state of the wave body.
    view: View,
    /// `true` once the fit-to-trace zoom was applied for this dump.
    fitted: bool,
    /// Vertical scroll offset of the row list, in css pixels.
    scroll_top: f64,

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

    cursor1: Option<i32>,
    cursor2: Option<i32>,

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
        let mut this = Self {
            signal_things: SignalThings::new(),
            waveform: ctx.props().waveform.clone(),
            added_seq: ctx.props().added_seq,

            view: View::default(),
            fitted: false,
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

            cursor1: None,
            cursor2: None,

            used: RefCell::new(Metrics::default()),
            last_decoded: 0,
            draws: 0,

            keydown: None,
            keyup: None,
            onresize: None,
        };
        this.attach_listeners(ctx);
        this
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Wheel(e) => {
                let metrics = self.measure();
                // over the name column the wheel scrolls the rows, like any list
                if (e.x() as f64) < metrics.surface_left {
                    return false;
                }
                e.prevent_default();
                let delta_y = e.delta_y();
                if self.key(17) {
                    //ctrl
                    if delta_y < 0.0 {
                        self.view.zoom(1.25);
                    } else {
                        self.view.zoom(0.8);
                    }
                } else if delta_y > 0.0 {
                    self.view.pan_px(30.0);
                } else {
                    self.view.pan_px(-30.0);
                }
                true
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
                    self.signal_things.onchoose(idx, false, false);
                    self.on_setting_idx = vec![idx];
                }
                true
            }
            Msg::SetSignal((close, set)) => {
                if close {
                    self.menu_show = false;
                }
                self.on_setting_idx.iter().for_each(|&x| {
                    if let Some(item) = self.signal_things.iter_mut().nth(x) {
                        item.setting = set.clone();
                    }
                });
                true
            }
            Msg::DeleteSig => {
                self.menu_show = false;
                let indices = std::mem::take(&mut self.on_setting_idx);
                self.signal_things.remove(&indices);
                ctx.props().onselect.emit(None);
                true
            }
            Msg::KeyEvent(idx) => {
                if idx == 90 {
                    //z
                    if self.key(16) {
                        //shift
                        self.view.zoom(1.25 * 1.25);
                    } else {
                        self.view.zoom(0.64);
                    }
                } else if idx == 70 {
                    //f
                    let metrics = self.measure();
                    let end = self.trace_end(ctx.props().time_range);
                    self.view = View::new(0.0, View::fit_size(end, metrics.surface_w));
                    self.fitted = true;
                }
                true
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
                self.mouse_press = true;
                self.mouse_start_x = x - metrics.surface_left;
                self.mouse_current_x = self.mouse_start_x;
                true
            }
            Msg::MouseMove(e) => {
                if !self.mouse_press {
                    return false;
                }
                let metrics = self.measure();
                self.mouse_current_x = e.x() as f64 - metrics.surface_left;
                true
            }
            Msg::MouseUp(e) => {
                e.prevent_default();
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
                    if current < self.signal_things.len()
                        && dest < self.signal_things.len()
                        && dest != current
                    {
                        self.signal_things.exchange(current, dest);
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
                    //a click, not a drag: set (or clear) one of the cursors
                    let cursor_time = self.view.time_at(x);
                    if e.button() == 0 {
                        if self.cursor1 == Some(cursor_time) {
                            self.cursor1 = None;
                        } else {
                            self.cursor1 = Some(cursor_time);
                        }
                    } else if self.cursor2 == Some(cursor_time) {
                        self.cursor2 = None;
                    } else {
                        self.cursor2 = Some(cursor_time);
                    }
                } else {
                    //drag a range to zoom into it
                    self.view.zoom_to_px(self.mouse_start_x, x, metrics.surface_w);
                }
                true
            }
            Msg::NameClick(idx) => {
                let ctrl = self.key(17);
                let shift = self.key(16);
                self.signal_things.onchoose(idx, ctrl, shift);
                if let Some(item) = self.signal_things.iter().nth(idx) {
                    let path = item.signal.value_key.clone();
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
                    let path = item.signal.value_key.clone();
                    ctx.props().onselect.emit(Some(path.clone()));
                    ctx.props().onjump.emit(path);
                }
                true
            }
            Msg::Resize => true,
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let mut redraw = false;
        // a new dump means every row has to be resolved and decoded again
        if self.waveform != ctx.props().waveform {
            self.waveform = ctx.props().waveform.clone();
            self.fitted = false;
            self.signal_things
                .refresh(self.waveform.as_ref().map(|waveform| waveform.as_waveform()));
            self.scroll_top = scroll_top_from_dom();
            redraw = true;
        }
        // only a real "add" pushes a row; `changed` also runs for every other
        // prop update (status, selection, a new dump), and pushing then would
        // bring back a row the user just deleted
        if self.added_seq != ctx.props().added_seq {
            self.added_seq = ctx.props().added_seq;
            let (signal_name, signal) = &ctx.props().signaladd;
            self.signal_things.push(
                signal_name,
                signal,
                self.waveform.as_ref().map(|waveform| waveform.as_waveform()),
            );
            redraw = true;
        }
        redraw
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        let measured = self.measure();
        if measured.is_usable() && !measured.approximately(&self.used.borrow()) {
            ctx.link().send_message(Msg::Resize);
        }
        *self.used.borrow_mut() = measured;
        self.redraw(ctx.props().time_range);
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let Some(window) = web_sys::window() {
            window.set_onkeydown(None);
            window.set_onkeyup(None);
            window.set_onresize(None);
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
            <div class="wave-pane">
                if self.menu_show && !self.on_setting_idx.is_empty() {
                    <Ctrl name={self.signal_things[self.on_setting_idx[0]].name.clone()}
                        setting={self.signal_things[self.on_setting_idx[0]].setting.clone()}
                        load={self.signal_things[self.on_setting_idx[0]].load.clone()}
                        drive={self.signal_things[self.on_setting_idx[0]].driver.clone()}
                        onset={link.callback(Msg::SetSignal)}
                        delete={link.callback(|_| Msg::DeleteSig)} />
                }
                <Time x_axis={self.view.x_axis}
                    size={self.view.size}
                    width={metrics.surface_w}
                    cursor1={self.cursor1}
                    cursor2={self.cursor2} />
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
                        {
                            for (name_first..name_last).map(|idx| {
                                let item = &self.signal_things[idx];
                                html!{<SignalName
                                    name={item.name.clone()}
                                    choose={item.choose}
                                    top={idx as f64 * ROW_HEIGHT}
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
        let Some(waveform) = self.waveform.as_ref().map(|waveform| waveform.as_waveform()) else {
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
        let Some(waveform) = self.waveform.as_ref().map(|waveform| waveform.as_waveform()) else {
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
    fn redraw(&mut self, time_range: Option<TimeRange>) {
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
        self.trim_cache(draw.clone());

        if !self.fitted {
            //first real measurement (and the first decoded rows): show the whole
            //trace. GHW has no cheap time range, so fall back to the span of the
            //histories we have.
            let end = self.trace_end(time_range);
            self.view = View::new(0.0, View::fit_size(end, metrics.surface_w));
            self.fitted = true;
        }

        let axis = build_axis(self.view.x_axis, self.view.size, metrics.surface_w);
        let visible: Vec<(usize, &SignalItem)> = (draw.start..draw.end)
            .filter_map(|row| self.signal_things.iter().nth(row).map(|item| (row, item)))
            .collect();

        let frame = canvas::Frame {
            view: self.view,
            width: metrics.surface_w,
            height: metrics.surface_h,
            scroll_top: self.scroll_top,
            rows: &visible,
            grid: &axis.labels,
            cursor1: self.cursor1,
            cursor2: self.cursor2,
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
            .map(|waveform| waveform.as_waveform().cache_stats().decoded_bytes())
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
            cursor1: self.cursor1,
            cursor2: self.cursor2,
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
        window.set_onkeydown(Some(keydown.as_ref().unchecked_ref()));

        let key_press = Rc::clone(&self.key_press);
        let keyup = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            if let Some(slot) = key_press.borrow_mut().get_mut(e.key_code() as usize) {
                *slot = false;
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        window.set_onkeyup(Some(keyup.as_ref().unchecked_ref()));

        let on_resize = ctx.link().callback(|_: ()| Msg::Resize);
        let onresize = Closure::wrap(Box::new(move |_: JsValue| {
            on_resize.emit(());
        }) as Box<dyn FnMut(JsValue)>);
        window.set_onresize(Some(onresize.as_ref().unchecked_ref()));

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
