use serde::{Deserialize, Serialize};
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct SignalName {
    name: String,
    padding: String,
    menu_show: String,
}

pub enum Msg {
    ContextMenu(MouseEvent),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalNameProps {
    pub name: String,
    #[prop_or_default]
    pub menu: Callback<()>,
}

impl Component for SignalName {
    type Message = Msg;
    type Properties = SignalNameProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        Self {
            name: props.name.clone(),
            padding: "9px".to_string(),
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
        html! {
            <div style={"padding:".to_owned()+&self.padding}>
                <p oncontextmenu={link.callback(Msg::ContextMenu)} style={"font-size:16px;margin:0px;height:16px"}>{&self.name}</p>
            </div>
        }
    }
}
