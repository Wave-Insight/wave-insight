use std::rc::Rc;

use wave_insight_lib::data_struct::{Module, Signal};
use super::SignalComponent;
use yew::prelude::*;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleComponentProps {
    pub space: String,
    pub name: String,
    pub module: Rc<Module>,
    #[prop_or_default]
    pub onclick: Callback<(String,Rc<Signal>)>,
}

pub enum Msg {
    ClickModule,
    ClickSignal((String,Rc<Signal>)),
}

pub struct ModuleComponent {
    space: String,
    name: String,
    show_signal: bool,
}

impl Component for ModuleComponent {
    type Message = Msg;
    type Properties = ModuleComponentProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        Self {
            space: props.space.clone(),
            name: props.name.clone(),
            show_signal: false,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::ClickModule => {
                self.show_signal = !self.show_signal;
                true
            }
            Msg::ClickSignal(input) => {
                ctx.props().onclick.emit(input);
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        let props = ctx.props();
        let signals = props.module.signal.iter();
        html! {
            <div>
                <h3 style="line-height:0.4;white-space:pre" onclick={ctx.link().callback(|_| Msg::ClickModule)}>{self.space.clone()+&self.name}</h3>
                {if self.show_signal {
                    html! {
                    {for signals.map(|s| html!{
                        <SignalComponent space={self.space.clone()} name={s.0.clone()} signal={Rc::new(s.1.clone())} onclick={ctx.link().callback(Msg::ClickSignal)}/>
                    })}
                    }
                }else {
                    html!{}
                }}
            </div>
        }
    }
}
