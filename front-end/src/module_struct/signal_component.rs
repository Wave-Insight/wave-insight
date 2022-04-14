use std::rc::Rc;

use yew::prelude::*;

use wave_insight_lib::data_struct::Signal;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalComponentProps {
    pub name: String,
    pub signal: Rc<Signal>,
    pub space: String,
    #[prop_or_default]
    pub onclick: Callback<(String,Rc<Signal>)>,
}

pub enum Msg {
    Click,
}

pub struct SignalComponent {
    space: String,
    show_name: String,
    signal: Rc<Signal>,
}

impl Component for SignalComponent {
    type Message = Msg;
    type Properties = SignalComponentProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        Self {
            space: props.space.clone(),
            show_name: props.name.clone(),//TODO: with size
            signal: Rc::clone(&props.signal),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::Click => {
                props.onclick.emit((self.show_name.clone(),Rc::clone(&self.signal)));
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                <p style="line-height:0.2;white-space:pre"
                    onclick={link.callback(|_| Msg::Click)}>{
                        self.space.clone()+&self.show_name
                    }
                </p>
            </div>
        }
    }
}
