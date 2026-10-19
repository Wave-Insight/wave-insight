use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wave_insight_lib::data_struct::BoolData;
use wave_insight_lib::data_struct::ModuleValue;
use wave_insight_lib::waveform::WaveValue;
//use web_sys::console;
use yew::prelude::*;
use wasm_bindgen::prelude::*;

use wave_insight_lib::data_struct::Signal;

use crate::nav::SharedWaveform;

use super::time::{build_axis, Time};
use super::ctrl::Ctrl;
use super::settings::Settings;
use super::signal::SignalName;
use super::signal::SignalValue;
use super::util::signal_things::{SignalItem, SignalThings};

///one signal row is `ROW_HEIGHT` pixels high, keep in sync with `.wave-row`
///and `.wave-name-cell` in styles.css
const ROW_HEIGHT: f64 = 30.0;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct WaveShowProps {
    pub signaladd: (String,Rc<Signal>),
    /// The opened dump; values are decoded from it per row.
    #[prop_or_default]
    pub waveform: Option<SharedWaveform>,
    /// Pre-expanded values for the transports that still send them (tauri /
    /// server); unused in the browser.
    #[prop_or_default]
    pub signal_value: Option<Rc<RefCell<ModuleValue>>>,
    pub end_clock: i32,
    /// The selected signal, for the driver/load panel.
    #[prop_or_default]
    pub onselect: Callback<Option<String>>,
    /// Open the declaration of one signal in the source pane.
    #[prop_or_default]
    pub onjump: Callback<String>,
}

pub enum Msg {
    Wheel(WheelEvent),
    ShowMenu(usize),
    SetSignal((bool,Settings)),
    DeleteSig,
    KeyEvent(u32),
    MouseDown(MouseEvent),
    MouseMove(MouseEvent),
    MouseUp(MouseEvent),
    NameMouseDown(MouseEvent),
    NameMouseUp(MouseEvent),
    NameClick(usize),
    ///the little arrow on a name cell: open its declaration
    Jump(usize),
    ///the layout moved, everything has to be measured again
    Resize,
}

///Geometry of the wave area.
///
///It is measured from the dom instead of guessed from the window size, so it
///stays correct with a fixed width sidebar, a zoomed page, a scrollbar
///appearing/disappearing or a high dpi screen.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Metrics {
    canvas_width: f64,
    canvas_left: f64,
    names_top: f64,
}

pub struct WaveShow {
    signal_things: SignalThings,
    /// The dump the rows were decoded from.
    waveform: Option<SharedWaveform>,
    x_axis: f64,
    size: f64,
    min_size: RefCell<f64>,

    menu_show: bool,
    on_setting_idx: Vec<usize>,

    key_press: Rc<RefCell<Vec<bool>>>,

    mouse_press: bool,
    mouse_start: i32,
    mouse_offset: RefCell<f64>,
    mouse_total: RefCell<f64>,

    name_choose_valid: bool,
    name_choose: usize,
    name_offset: RefCell<f64>,

    cursor1: Option<i32>,
    cursor2: Option<i32>,

    ///what the last `view` was drawn with, so `rendered` can notice the layout moved
    used: RefCell<Metrics>,
    ///the listeners are created once and dropped in `destroy`, forgetting them on
    ///every render leaked one set of them per render
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
            x_axis: 0f64,
            size: 1f64,
            min_size: RefCell::new(0f64),

            menu_show: false,
            on_setting_idx: Vec::new(),

            key_press: Rc::new(RefCell::new(vec![false;256])),

            mouse_press: false,
            mouse_start: 0,
            mouse_offset: RefCell::new(0f64),
            mouse_total: RefCell::new(0f64),

            name_choose_valid: false,
            name_choose: 0,
            name_offset: RefCell::new(0f64),

            cursor1: None,
            cursor2: None,

            used: RefCell::new(Metrics::default()),
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
                e.prevent_default();
                let delta_y = e.delta_y();
                if self.key(17) {//ctrl
                    if delta_y < 0.0 {
                        self.size *= 1.25;
                    }else {
                        self.size *= 0.8;
                    }
                }else if delta_y > 0.0 {
                    self.x_axis += 30.0/self.size;
                }else if self.x_axis >= 30.0/self.size {
                    self.x_axis -= 30.0/self.size;
                }else {
                    self.x_axis = 0.0;
                }
                true
            }
            Msg::ShowMenu(idx) => {
                self.menu_show = true;
                let choosed_idx = self.signal_things.get_choose_idx();
                if choosed_idx.is_empty() {
                    self.on_setting_idx = vec![idx];
                }else {
                    self.on_setting_idx = choosed_idx;
                }
                true
            }
            Msg::SetSignal((close,set)) => {
                if close {
                    self.menu_show = false;
                }
                self.on_setting_idx.iter().for_each(|&x| {
                    self.signal_things[x].setting = set.clone();
                });
                true
            }
            Msg::DeleteSig => {
                self.menu_show = false;
                self.signal_things.remove(&self.on_setting_idx);
                ctx.props().onselect.emit(None);
                true
            }
            Msg::KeyEvent(idx) => {
                if idx == 90 {//z
                    if self.key(16) {//shift
                        self.size *= 1.25*1.25;
                    }else {
                        self.size *= 0.64;
                    }
                }else if idx == 70 {//f
                    self.x_axis = 0.0;
                    self.size = (*self.min_size.borrow()).max(1e-6);
                }
                true
            }

            Msg::MouseDown(e) =>{
                e.prevent_default();
                self.mouse_press = true;
                let x = e.x();
                self.mouse_start = x;
                //console::log_1(&format!("x:{}",x).into());
                e.prevent_default();
                true
            }
            Msg::MouseMove(_e) => {
                //let x = e.x();
                //if self.mouse_press {
                //    console::log_1(&format!("x:{}",x).into());
                //}
                //TODO:to show the range
                true
            }
            Msg::MouseUp(e) => {
                e.prevent_default();
                self.mouse_press = false;
                let x = e.x();
                if (self.mouse_start - x).abs() <= 2 {
                    let cursor_time = ((x as f64 - *self.mouse_offset.borrow()) / self.size + self.x_axis) as i32;
                    if e.button() == 0 {
                        if self.cursor1.map(|c| c==cursor_time).unwrap_or(false) {
                            self.cursor1 = None;
                        }else {
                            self.cursor1 = Some(cursor_time);//TODO:attach to the edge
                        }
                    }else if self.cursor2.map(|c| c==cursor_time).unwrap_or(false) {
                        self.cursor2 = None;
                    }else {
                        self.cursor2 = Some(cursor_time);//TODO:attach to the edge
                    }
                }else {
                    let (start,end) = if self.mouse_start < x {
                        (self.mouse_start, x)
                    }else {
                        (x, self.mouse_start)
                    };
                    let total = *self.mouse_total.borrow();
                    //the canvas is not measured yet on the very first frame
                    if total > 1.0 && (end - start) > 1 {
                        self.x_axis += (start as f64 - *self.mouse_offset.borrow()) / self.size;
                        self.size *= total / (end - start) as f64;
                        //console::log_1(&format!("x:{}",x).into());
                    }
                }
                true
            }
            //TODO:MouseOut

            Msg::NameMouseDown(e) => {
                self.name_choose_valid = true;
                self.name_choose = ((e.y() as f64 - *self.name_offset.borrow()) / ROW_HEIGHT) as usize;
                //console::log_1(&format!("x:{}",self.name_choose).into());
                e.prevent_default();
                true
            }
            Msg::NameMouseUp(e) => {
                self.name_choose_valid = false;
                let set_location = (e.y() as f64 - *self.name_offset.borrow()) / ROW_HEIGHT;
                let dest_idx = if set_location > self.name_choose as f64 + 0.5 {
                    (set_location - 0.5) as usize
                }else {
                    (set_location + 0.5) as usize
                };
                if self.name_choose < self.signal_things.len() && dest_idx < self.signal_things.len() {
                    self.signal_things.exchange(self.name_choose, dest_idx);
                }
                true
            }
            //TODO:show the destiny when is dragging
            //TODO:drag to the button not work
            //TODO:drag out of the area may cause bug
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
        // a new dump means every row has to be decoded again
        if self.waveform != ctx.props().waveform {
            self.waveform = ctx.props().waveform.clone();
            self.signal_things
                .refresh(self.waveform.as_ref().map(|waveform| waveform.as_waveform()));
        }
        let (signal_name,signal) = &ctx.props().signaladd;
        self.signal_things.push(
            signal_name,
            signal,
            self.waveform.as_ref().map(|waveform| waveform.as_waveform()),
        );
        true
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        //the first `view` runs before the canvas exists, so measure again once it
        //is in the document and redraw if the guess was off
        let measured = measure();
        if measured.canvas_width > 1.0 && !approximately(measured, *self.used.borrow()) {
            ctx.link().send_message(Msg::Resize);
        }
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
        let end_clock = ctx.props().end_clock;

        let metrics = measure();
        let wave_show_width = if metrics.canvas_width > 1.0 {
            metrics.canvas_width
        }else {
            //nothing to measure before the first paint, `rendered` fixes it right
            //after; 500 is just --sidebar-w + --names-w as a starting guess
            let win_width = web_sys::window()
                .and_then(|w| w.inner_width().ok())
                .and_then(|v| v.as_f64())
                .unwrap_or(1024.0);
            (win_width - 500.0).max(100.0)
        };
        *self.used.borrow_mut() = Metrics {
            canvas_width: wave_show_width,
            canvas_left: metrics.canvas_left,
            names_top: metrics.names_top,
        };
        *self.min_size.borrow_mut() = wave_show_width / ((end_clock + 1) as f64);
        *self.mouse_offset.borrow_mut() = metrics.canvas_left;
        *self.mouse_total.borrow_mut() = wave_show_width;
        *self.name_offset.borrow_mut() = metrics.names_top;

        //the bar above and the grid below are built from the same ruler, so a
        //grid line always sits exactly under its number
        let axis = build_axis(self.x_axis, self.size, wave_show_width);

        let has_signal = !self.signal_things.is_empty();

        html! {
            <div class="wave-pane">
                if self.menu_show {
                    <Ctrl name={self.signal_things[self.on_setting_idx[0]].name.clone()}//TODO:not [0] but all
                        setting={self.signal_things[self.on_setting_idx[0]].setting.clone()}
                        load={self.signal_things[self.on_setting_idx[0]].load.clone()}
                        drive={self.signal_things[self.on_setting_idx[0]].driver.clone()}
                        onset={link.callback(Msg::SetSignal)}
                        delete={link.callback(|_| Msg::DeleteSig)} />
                }
                <Time x_axis={self.x_axis}
                    size={self.size}
                    width={wave_show_width}
                    cursor1={self.cursor1}
                    cursor2={self.cursor2} />
                <div class="wave-body">
                    <div class="wave-names" id="wave-names"
                        onmousedown={link.callback(Msg::NameMouseDown)}
                        onmouseup={link.callback(Msg::NameMouseUp)}>
                        {
                            for self.signal_things.iter().enumerate().map(|(idx,s)| {
                                html!{<SignalName
                                    name={s.name.clone()}
                                    choose={s.choose}
                                    menu={link.callback(move |()| Msg::ShowMenu(idx))}
                                    onjump={link.callback(move |()| Msg::Jump(idx))}
                                    onclick={link.callback(move |()| Msg::NameClick(idx))}/>}
                            })
                        }
                    </div>
                    <div class="wave-canvas" id="wave-canvas"
                        onwheel={link.callback(Msg::Wheel)}
                        onmousedown={link.callback(Msg::MouseDown)}
                        onmouseup={link.callback(Msg::MouseUp)}
                        onmousemove={link.callback(Msg::MouseMove)}>
                        <svg class="wave-grid">
                            {
                                for axis.labels.iter().map(|&(x, _time)| {
                                    html!{<line class="grid-line" x1={format!("{x:.2}")} x2={format!("{x:.2}")}
                                        y1="0" y2="100%" />}
                                })
                            }
                        </svg>
                        {
                            for self.signal_things.iter().enumerate().map(|(idx,s)| {
                                html!{<SignalValue
                                    series={s.series.clone()}
                                    legacy={ctx.props().signal_value.clone()}
                                    signal={Rc::clone(&s.signal)} bool_signal={s.is_bool}
                                    x_axis={self.x_axis} size={self.size}
                                    width = {wave_show_width}
                                    setting={self.signal_things[idx].setting.clone()}
                                    cursor1={self.cursor1}
                                    cursor2={self.cursor2}
                                    cursor_value={self.cursor_value(s)} />}
                            })
                        }
                        {if has_signal {html!{}} else {html!{
                            <div class="empty-hint">
                                <div>{"No signals yet"}</div>
                                <div>{"Pick one from the signal list, drag a range to zoom,"}<br/>
                                    {"click a name in the source to add it here."}</div>
                            </div>
                        }}}
                    </div>
                </div>
            </div>
        }
    }
}

impl WaveShow {
    fn key(&self, code: usize) -> bool {
        self.key_press.borrow().get(code).copied().unwrap_or(false)
    }

    /// The value of one row at the first cursor, for the readout next to it.
    fn cursor_value(&self, item: &SignalItem) -> Option<String> {
        let cursor = self.cursor1?;
        if cursor < 0 {
            return None;
        }
        let waveform = self.waveform.as_ref()?.as_waveform();
        let info = waveform.signal_by_path(&item.signal.value_key)?;
        let value = waveform.value_at(info.index(), cursor as u64).ok().flatten()?;
        format_value(&value, &item.setting, item.signal.size)
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
            on_key.emit(code);
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

/// Format one value with the row's `ShowType`, reusing the legacy formatters.
fn format_value(value: &WaveValue, setting: &Settings, size: usize) -> Option<String> {
    if let Some(value) = value.to_bool_data() {
        return Some(
            match value {
                BoolData::Zero => "0",
                BoolData::One => "1",
                BoolData::X => "x",
                BoolData::Z => "z",
            }
            .to_string(),
        );
    }
    value
        .to_bits_data()
        .map(|bits| bits.to_string(size.max(1), &setting.show_type))
}

///read the geometry back from the dom
fn measure() -> Metrics {
    let window = match web_sys::window() {
        Some(window) => window,
        None => return Metrics::default(),
    };
    let document = match window.document() {
        Some(document) => document,
        None => return Metrics::default(),
    };

    let canvas = document.get_element_by_id("wave-canvas");
    let names = document.get_element_by_id("wave-names");

    let (canvas_width, canvas_left) = match &canvas {
        Some(canvas) => {
            let rect = canvas.get_bounding_client_rect();
            (rect.width(), rect.left())
        },
        None => (0.0, 0.0),
    };
    let names_top = match &names {
        Some(names) => names.get_bounding_client_rect().top(),
        None => 0.0,
    };

    Metrics {
        canvas_width,
        canvas_left,
        names_top,
    }
}

fn approximately(a: Metrics, b: Metrics) -> bool {
    (a.canvas_width - b.canvas_width).abs() <= 0.5
        && (a.canvas_left - b.canvas_left).abs() <= 0.5
        && (a.names_top - b.names_top).abs() <= 0.5
}
