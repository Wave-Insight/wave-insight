use serde::{Deserialize, Serialize};
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct SignalName {
    padding: String,
    menu_show: String,
}

pub enum Msg {
    ContextMenu(MouseEvent),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalNameProps {
    pub name: String,
    pub choose: bool,
    #[prop_or_default]
    pub menu: Callback<()>,
    #[prop_or_default]
    pub onclick: Callback<()>,
}

impl Component for SignalName {
    type Message = Msg;
    type Properties = SignalNameProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            padding: "0px".to_string(),
            menu_show: "hidden".to_string(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::ContextMenu(event) => {
                event.prevent_default();
                props.menu.emit(());
                true
            },
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let background = if ctx.props().choose {"#aaaaff"} else {"#ffffff"};
        html! {
            <div style={"padding:".to_owned()+&self.padding+";background-color:"+background}>
                <p oncontextmenu={link.callback(Msg::ContextMenu)}
                    onclick={ctx.props().onclick.reform(|_| ())}
                    style={"font-size:16px;margin:0px;height:30px"}>
                    {&ctx.props().name}
                </p>
            </div>
        }
    }
}
