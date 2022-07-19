use std::rc::Rc;

use yew::prelude::*;

use wave_insight_lib::data_struct::Signal;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalComponentProps {
    pub name: String,
    pub signal: Rc<Signal>,
    #[prop_or_default]
    pub onclick: Callback<(String,Rc<Signal>)>,
}

pub enum Msg {
    Click,
}

pub struct SignalComponent {
}

impl Component for SignalComponent {
    type Message = Msg;
    type Properties = SignalComponentProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::Click => {
                props.onclick.emit((props.name.clone(),Rc::clone(&props.signal)));
                true
            }
        }
    }

    fn changed(&mut self, _ctx: &Context<Self>) -> bool {
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                <p style="line-height:0.2;white-space:pre"
                    onclick={link.callback(|_| Msg::Click)}>{
                        &ctx.props().name
                    }
                </p>
            </div>
        }
    }
}
