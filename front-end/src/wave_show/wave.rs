use num::BigUint;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use wave_insight_lib::data_struct::Signal;

use super::ctrl::Ctrl;
use super::settings::Settings;
use super::signal::SignalName;
use super::signal::SignalValue;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct WaveShowProps {
    pub signaladd: (String,Signal),
    pub end_clock: i32,
}

pub enum Msg {
    SetX(f64),
    Wheel(WheelEvent),
    ShowMenu(usize),
    SetSignal((bool,Settings)),
}

pub struct WaveShow {
    signal_name: Vec<String>,
    signal_show: Vec<Vec<(i32,BigUint)>>,
    bool_signal: Vec<bool>,
    signal_setting: Vec<Settings>,
    signal_bitcount: Vec<usize>,
    load_and_drive: Vec<(Vec<String>,Vec<String>)>,
    x_axis: f64,
    size: f64,

    menu_show: bool,
    on_show_idx: usize,
}

impl Component for WaveShow {
    type Message = Msg;
    type Properties = WaveShowProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            signal_name: vec![],
            signal_show: vec![],
            bool_signal: vec![],
            signal_setting: vec![],
            signal_bitcount: vec![],
            load_and_drive: vec![],
            x_axis: 0f64,
            size: 1f64,

            menu_show: false,
            on_show_idx: 0,
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
                if delta_y < 0.0 {
                    self.size *= 1.25;
                }else {
                    self.size *= 0.8;
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
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let (signal_name,signal) = &ctx.props().signaladd;
        if !signal_name.is_empty() {
            let bool_signal = signal.size==1;
            self.signal_name.push(
                if bool_signal {signal_name.clone()}
                else {signal_name.clone()+"["+&(signal.size-1).to_string()+":0]"});
            self.signal_show.push(signal.value_change.clone());
            self.bool_signal.push(bool_signal);
            self.signal_setting.push(Settings::new());
            self.signal_bitcount.push(signal.size);
            self.load_and_drive.push((signal.load.clone(),signal.drive.clone()));
        }
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let end_clock = ctx.props().end_clock;
        
        html! {
            <div style="display:block;height:50%;overflow-y:auto">
                if self.menu_show {
                    <Ctrl name={self.signal_name[self.on_show_idx as usize].clone()}
                        setting={self.signal_setting[self.on_show_idx as usize].clone()}
                        load={self.load_and_drive[self.on_show_idx as usize].0.clone()}
                        drive={self.load_and_drive[self.on_show_idx as usize].1.clone()}
                        onset={link.callback(Msg::SetSignal)} />
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
                            for (&self.signal_show).iter().zip(&self.bool_signal).enumerate().map(|(idx,(s,b))| {
                                html!{<SignalValue value={s.clone()} bool_signal={*b} x_axis={self.x_axis} size={self.size} setting={self.signal_setting[idx].clone()} bitcount={self.signal_bitcount[idx]} />}
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
