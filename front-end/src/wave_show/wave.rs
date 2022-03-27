use num::BigUint;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use wave_insight_lib::data_struct::Signal;

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
}

pub struct WaveShow {
    signal_name: Vec<String>,
    signal_show: Vec<Vec<(i32,BigUint)>>,
    bool_signal: Vec<bool>,
    x_axis: f64,
    size: f64,
}

impl Component for WaveShow {
    type Message = Msg;
    type Properties = WaveShowProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            signal_name: vec![],
            signal_show: vec![],
            bool_signal: vec![],
            x_axis: 0f64,
            size: 1f64,
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
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let (signal_name,signal) = &ctx.props().signaladd;
        if !signal_name.is_empty() {
            self.signal_name.push(signal_name.clone());
            self.signal_show.push(signal.value_change.clone());
            self.bool_signal.push(signal.size==1);
        }
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let end_clock = ctx.props().end_clock;
        
        html! {
            <div>
                <div style="display:block;height:400px;overflow-y:auto">
                    <div style="float:left;width:10%">
                        {
                            for (&self.signal_name).iter().map(|s| {
                                html!{<SignalName name={s.clone()}/>}
                            })
                        }
                    </div>
                    <div onwheel={link.callback(Msg::Wheel)} style="float:right;width:90%;background-color:#202020">
                        {
                            for (&self.signal_show).iter().zip(&self.bool_signal).map(|(s,b)| {
                                html!{<SignalValue value={s.clone()} bool_signal={*b} x_axis={self.x_axis} size={self.size} />}
                            })
                        }
                    </div>
                </div>
                <input id="slider" type="range"
                min="0" max={end_clock.to_string()} step="1" style="width:99%"
                onchange={link.callback(|e: Event| Msg::SetX(e.target_unchecked_into::<HtmlInputElement>().value_as_number()))} />
            </div>
        }
    }
}
