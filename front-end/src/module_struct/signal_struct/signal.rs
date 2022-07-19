use std::rc::Rc;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use wave_insight_lib::data_struct::{Module, Signal};
use super::SignalComponent;

pub enum Msg {
    GetClick((String,Rc<Signal>)),
    SetFilter(String),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalStructProps {
    pub module: Rc<Module>,
    #[prop_or_default]
    pub signaladd: Callback<(String,Rc<Signal>)>,
}

pub struct SignalStruct {
    signal_show: Vec<bool>,
}

impl Component for SignalStruct {
    type Message = Msg;
    type Properties = SignalStructProps;

    fn create(ctx: &Context<Self>) -> Self {
        let size = ctx.props().module.signal.len();
        Self {
            signal_show: vec![true; size],
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::GetClick(s) => {
                props.signaladd.emit(s);
                true
            },
            Msg::SetFilter(s) => {
                let signals = ctx.props()
                    .module
                    .signal
                    .iter()
                    .map(|x| x.0.contains(&s));
                self.signal_show = signals.collect();
                true
            }
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        self.signal_show = vec![true;ctx.props().module.signal.len()];
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {

        let signals = ctx.props()
            .module
            .signal
            .iter();
        
        html! {
            <div style="display:block;height:50%;overflow-y:auto">
                <div style="display:block;height:90%;overflow-y:auto">
                {
                    html! {
                    {for signals.zip(&self.signal_show).map(|(s,&v)| if v { html!{
                        <SignalComponent name={s.0.clone()} signal={Rc::new(s.1.clone())} onclick={ctx.link().callback(Msg::GetClick)}/>
                    }}else {
                        html!{}
                    })}
                    }
                }
                </div>
                <input type="text" oninput={ctx.link().callback(|e: InputEvent| Msg::SetFilter(e.target_unchecked_into::<HtmlInputElement>().value()))} />
            </div>
        }
    }
}
