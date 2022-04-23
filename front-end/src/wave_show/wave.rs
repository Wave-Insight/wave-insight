use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::JsCast;
use wave_insight_lib::data_struct::Module;
use web_sys::HtmlInputElement;
//use web_sys::console;
use yew::prelude::*;
use wasm_bindgen::prelude::*;

use wave_insight_lib::data_struct::Signal;

use super::ctrl::Ctrl;
use super::settings::Settings;
use super::signal::SignalName;
use super::signal::SignalValue;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct WaveShowProps {
    pub signaladd: (String,Rc<Signal>),
    pub module: Rc<Module>,
    pub end_clock: i32,
}

pub enum Msg {
    SetX(f64),
    Wheel(WheelEvent),
    ShowMenu(usize),
    SetSignal((bool,Settings)),
    DeleteSig,
}

pub struct WaveShow {
    signal_name: Vec<String>,
    signal: Vec<Rc<Signal>>,
    bool_signal: Vec<bool>,
    signal_setting: Vec<Settings>,
    load_and_drive: Vec<(Vec<String>,Vec<String>)>,
    x_axis: f64,
    size: f64,

    menu_show: bool,
    on_show_idx: usize,

    ctrl_key_press: Rc<RefCell<bool>>,
}

impl Component for WaveShow {
    type Message = Msg;
    type Properties = WaveShowProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            signal_name: vec![],
            signal: vec![],
            bool_signal: vec![],
            signal_setting: vec![],
            load_and_drive: vec![],
            x_axis: 0f64,
            size: 1f64,

            menu_show: false,
            on_show_idx: 0,

            ctrl_key_press: Rc::new(RefCell::new(false)),
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::SetX(x) => {
                self.x_axis = x;
                true
            }
            Msg::Wheel(e) => {
                e.prevent_default();
                let delta_y = e.delta_y();
                if *self.ctrl_key_press.borrow() {
                    if delta_y < 0.0 {
                        self.size *= 1.25;
                    }else {
                        self.size *= 0.8;
                    }
                }else if delta_y > 0.0 {
                    self.x_axis += 10.0;
                }else if self.x_axis >= 10.0 {
                    self.x_axis -= 10.0;
                }else {
                    self.x_axis = 0.0;
                }
                true
            }
            Msg::ShowMenu(idx) => {
                self.menu_show = true;
                self.on_show_idx = idx;
                true
            }
            Msg::SetSignal((close,set)) => {
                if close {
                    self.menu_show = false;
                }
                self.signal_setting[self.on_show_idx] = set;
                true
            }
            Msg::DeleteSig => {
                self.menu_show = false;
                let idx = self.on_show_idx;
                self.signal.remove(idx);
                self.signal_name.remove(idx);
                self.bool_signal.remove(idx);
                self.signal_setting.remove(idx);
                self.load_and_drive.remove(idx);
                true
            }
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let (signal_name,signal) = &ctx.props().signaladd;
        if !signal_name.is_empty() {
            let bool_signal = signal.size==1;
            self.signal_name.push(
                if bool_signal {signal_name.clone()}
                else {signal_name.clone()+"["+&(signal.size-1).to_string()+":0]"});
            self.signal.push(Rc::clone(signal));
            self.bool_signal.push(bool_signal);
            self.signal_setting.push(Settings::new());
            self.load_and_drive.push((signal.load.clone(),signal.drive.clone()));
        }
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let end_clock = ctx.props().end_clock;

        let window = web_sys::window().expect("should have a window in this context");
        let ctrl_key_press = self.ctrl_key_press.clone();
        let keydown = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            //17:ctrl, 18:alt, 16:shift, 65:a, 48:0, 49:1
            if e.key_code() == 17 {
                *ctrl_key_press.borrow_mut() = true;
            };
            //console::log_1(&format!("key down {}",e.key_code()).into());
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        window.set_onkeydown(Some(keydown.as_ref().unchecked_ref()));
        keydown.forget();

        let ctrl_key_press2 = self.ctrl_key_press.clone();
        let keyup = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            //17:ctrl, 18:alt, 16:shift, 65:a, 48:0, 49:1
            if e.key_code() == 17 {
                *ctrl_key_press2.borrow_mut() = false;
            };
            //console::log_1(&format!("key up {}",e.key_code()).into());
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        window.set_onkeyup(Some(keyup.as_ref().unchecked_ref()));
        keyup.forget();
        
        html! {
            <div style="display:block;height:50%;overflow-y:auto">
                if self.menu_show {
                    <Ctrl name={self.signal_name[self.on_show_idx as usize].clone()}
                        setting={self.signal_setting[self.on_show_idx as usize].clone()}
                        load={self.load_and_drive[self.on_show_idx as usize].0.clone()}
                        drive={self.load_and_drive[self.on_show_idx as usize].1.clone()}
                        onset={link.callback(Msg::SetSignal)}
                        delete={link.callback(|_| Msg::DeleteSig)} />
                }
                <div style="height:90%;overflow-y:auto">
                    <div style="float:left;width:10%">
                        {
                            for (&self.signal_name).iter().enumerate().map(|(idx,s)| {
                                html!{<SignalName name={s.clone()} menu={link.callback(move |()| Msg::ShowMenu(idx))} />}
                            })
                        }
                    </div>
                    <div onwheel={link.callback(Msg::Wheel)} style="float:right;width:90%;background-color:#202020">
                        {
                            for (&self.signal).iter().zip(&self.bool_signal).enumerate().map(|(idx,(s,b))| {
                                html!{<SignalValue module={Rc::clone(&ctx.props().module)} signal={s} bool_signal={*b} x_axis={self.x_axis} size={self.size} setting={self.signal_setting[idx].clone()} />}
                            })
                        }
                    </div>
                </div>
                <input id="slider" type="range"
                    min="0" max={end_clock.to_string()} step="1" style="margin:0px;width:99%;height:9%"
                    oninput={link.callback(|e: InputEvent| Msg::SetX(e.target_unchecked_into::<HtmlInputElement>().value_as_number()))}
                />
            </div>
        }
    }
}
