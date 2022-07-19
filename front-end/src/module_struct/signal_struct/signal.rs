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
    filter: String,
}

impl Component for SignalStruct {
    type Message = Msg;
    type Properties = SignalStructProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            filter: "".to_string(),
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
                self.filter = s;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        
        html! {
            <div style="display:block;height:50%;overflow-y:auto">
                <div style="display:block;height:90%;overflow-y:auto">
                {
                    html! {
                    {for ctx.props().module.signal.iter()
                        .filter(|x| x.0.to_ascii_lowercase().contains(&self.filter.to_ascii_lowercase()))
                        .map(|s| { html!{
                        <SignalComponent name={s.0.to_string()} signal={Rc::new(s.1.clone())} onclick={ctx.link().callback(Msg::GetClick)}/>
                    }})}
                    }
                }
                </div>
                <input type="text" oninput={ctx.link().callback(|e: InputEvent| Msg::SetFilter(e.target_unchecked_into::<HtmlInputElement>().value()))} />
            </div>
        }
    }
}
