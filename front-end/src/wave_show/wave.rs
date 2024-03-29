use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wave_insight_lib::data_struct::Module;
use wave_insight_lib::data_struct::ModuleValue;
//use web_sys::console;
use yew::prelude::*;
use wasm_bindgen::prelude::*;

use wave_insight_lib::data_struct::Signal;

use super::time::Time;
use super::ctrl::Ctrl;
use super::settings::Settings;
use super::signal::SignalName;
use super::signal::SignalValue;
use super::util::signal_things::SignalThings;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct WaveShowProps {
    pub signaladd: (String,Rc<Signal>),
    pub module: Rc<Module>,
    pub signal_value: Rc<RefCell<ModuleValue>>,
    pub end_clock: i32,
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
}

pub struct WaveShow {
    signal_things: SignalThings,
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
}

impl Component for WaveShow {
    type Message = Msg;
    type Properties = WaveShowProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            signal_things: SignalThings::new(),
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
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Wheel(e) => {
                e.prevent_default();
                let delta_y = e.delta_y();
                if *self.key_press.borrow().get(17).unwrap() {
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
                true
            }
            Msg::KeyEvent(idx) => {
                if idx == 90 {//z
                    if *self.key_press.borrow().get(16).unwrap() {
                        self.size *= 1.25*1.25;
                    }else {
                        self.size *= 0.64;
                    }
                }else if idx == 70 {//f
                    self.x_axis = 0.0;
                    self.size = *self.min_size.borrow();
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
                    self.x_axis += (start as f64 - *self.mouse_offset.borrow()) / self.size;
                    self.size *= *self.mouse_total.borrow() / (end - start) as f64;
                    //console::log_1(&format!("x:{}",x).into());
                }
                true
            }
            //TODO:MouseOut

            Msg::NameMouseDown(e) => {
                self.name_choose_valid = true;
                self.name_choose = ((e.y() as f64 - *self.name_offset.borrow()) / 30.0) as usize;
                //console::log_1(&format!("x:{}",self.name_choose).into());
                e.prevent_default();
                true
            }
            Msg::NameMouseUp(e) => {
                self.name_choose_valid = false;
                let set_location = (e.y() as f64 - *self.name_offset.borrow()) / 30.0;
                let dest_idx = if set_location > self.name_choose as f64 + 0.5 {
                    (set_location - 0.5) as usize
                }else {
                    (set_location + 0.5) as usize
                };
                self.signal_things.exchange(self.name_choose, dest_idx);
                true
            }
            //TODO:show the destiny when is dragging
            //TODO:drag to the button not work
            //TODO:drag out of the area may cause bug
            Msg::NameClick(idx) => {
                self.signal_things.onchoose(idx,
                    *self.key_press.borrow().get(17).unwrap(),
                    *self.key_press.borrow().get(16).unwrap()
                );
                true
            }
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let (signal_name,signal) = &ctx.props().signaladd;
        self.signal_things.push(signal_name, signal);
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let end_clock = ctx.props().end_clock;

        self.key_event(ctx);

        let window = web_sys::window().expect("should have a window in this context");
        let win_width = window.inner_width().unwrap().as_f64().unwrap();
        let win_height = window.inner_height().unwrap().as_f64().unwrap();
        let wave_show_width = win_width * 0.8 * 0.9;//TODO:0.8 and 0.9 should be auto set
        *self.min_size.borrow_mut() = wave_show_width / ((end_clock + 1) as f64);
        *self.mouse_offset.borrow_mut() = win_width * (1.0 - 0.8*0.9);
        *self.mouse_total.borrow_mut() = wave_show_width;

        *self.name_offset.borrow_mut() = (win_height-64.0)*0.5+64.0+30.0;//TODO:this should be auto calculate
        //console::log_1(&format!("width {}",win_width).into());
        
        html! {
            <div style="display:block;height:50%;overflow-y:auto">
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
                <div style="height:90%;overflow-y:auto">//TODO:height of time is 30 px, this should be calculated
                    <div onmousedown={link.callback(Msg::NameMouseDown)}
                        onmouseup={link.callback(Msg::NameMouseUp)}
                        style="float:left;width:10%">
                        {
                            for self.signal_things.iter().enumerate().map(|(idx,s)| {
                                html!{<SignalName
                                    name={s.name.clone()}
                                    choose={s.choose}
                                    menu={link.callback(move |()| Msg::ShowMenu(idx))}
                                    onclick={link.callback(move |()| Msg::NameClick(idx))}/>}
                            })
                        }
                    </div>
                    <div onwheel={link.callback(Msg::Wheel)}
                        onmousedown={link.callback(Msg::MouseDown)}
                        onmouseup={link.callback(Msg::MouseUp)}
                        onmousemove={link.callback(Msg::MouseMove)}
                        style="float:right;width:90%;background-color:#202020">
                        {
                            for self.signal_things.iter().enumerate().map(|(idx,s)| {
                                html!{<SignalValue
                                    signal_value={Rc::clone(&ctx.props().signal_value)}
                                    signal={Rc::clone(&s.signal)} bool_signal={s.is_bool}
                                    x_axis={self.x_axis} size={self.size}
                                    width = {wave_show_width}
                                    setting={self.signal_things[idx].setting.clone()}
                                    cursor1={self.cursor1}
                                    cursor2={self.cursor2} />}
                            })
                        }
                    </div>
                </div>
                /*<input id="slider" type="range"
                    min="0" max={end_clock.to_string()} step="1" style="margin:0px;width:99%;height:9%"
                    oninput={link.callback(|e: InputEvent| Msg::SetX(e.target_unchecked_into::<HtmlInputElement>().value_as_number()))}
                />*/
            </div>
        }
    }
}

impl WaveShow {
    fn key_event(&self, ctx: &Context<Self>) {
        let callback = ctx.link().callback(Msg::KeyEvent);
        let window = web_sys::window().expect("should have a window in this context");
        let key_press = self.key_press.clone();
        let keydown = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            //17:ctrl, 18:alt, 16:shift, 65:a, 48:0, 49:1
            *key_press.borrow_mut().get_mut(e.key_code() as usize).unwrap() = true;
            callback.emit(e.key_code());
            //console::log_1(&format!("key down {}",e.key_code()).into());
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        window.set_onkeydown(Some(keydown.as_ref().unchecked_ref()));
        keydown.forget();

        let key_press2 = self.key_press.clone();
        let keyup = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            //17:ctrl, 18:alt, 16:shift, 65:a, 48:0, 49:1
            *key_press2.borrow_mut().get_mut(e.key_code() as usize).unwrap() = false;
            //console::log_1(&format!("key up {}",e.key_code()).into());
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        window.set_onkeyup(Some(keyup.as_ref().unchecked_ref()));
        keyup.forget();
    }
}
