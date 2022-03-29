use serde::{Deserialize, Serialize};
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct SignalName {
    name: String,
    height: String,
    menu_show: String,
}

pub enum Msg {
    ContextMenu(MouseEvent),
    Setting(String),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalNameProps {
    pub name: String,
}

impl Component for SignalName {
    type Message = Msg;
    type Properties = SignalNameProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        Self {
            name: props.name.clone(),
            height: "34px".to_string(),
            menu_show: "hidden".to_string(),
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::ContextMenu(event) => {
                event.prevent_default();
                if self.menu_show == "hidden" {
                    self.menu_show = "visible".to_string();
                }else {
                    self.menu_show = "hidden".to_string();
                }
                true
            },
            Msg::Setting(set_what) => {
                self.menu_show = "hidden".to_string();
                if set_what == "Analog" {
                    if self.height == "34px" {
                        self.height = "60px".to_string();
                    }else {
                        self.height = "34px".to_string();
                    }
                }
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        html! {
            <div>
                <p oncontextmenu={link.callback(Msg::ContextMenu)} style={"margin:0px;height:".to_owned()+&self.height}>{&self.name}</p>
                <div style={"border:1px solid #ddd;position:absolute;background-color:#fff;visibility:".to_owned()+&self.menu_show}>
                    <a onclick={link.callback(|_| Msg::Setting("Hex".to_string()))}>{"Hex"}</a>
                    <hr/>
                    <a onclick={link.callback(|_| Msg::Setting("Decimal".to_string()))}>{"Decimal"}</a>
                    <hr/>
                    <a onclick={link.callback(|_| Msg::Setting("Signed Decimal".to_string()))}>{"Signed Decimal"}</a>
                    <hr/>
                    <a onclick={link.callback(|_| Msg::Setting("Binary".to_string()))}>{"Binary"}</a>
                    <hr/>
                    <a onclick={link.callback(|_| Msg::Setting("Octal".to_string()))}>{"Octal"}</a>
                    <hr/>
                    <a onclick={link.callback(|_| Msg::Setting("ASCII".to_string()))}>{"ASCII"}</a>
                    <hr/>
                    <a onclick={link.callback(|_| Msg::Setting("Analog".to_string()))}>{"Analog"}</a>
                    <hr/>
                    <a onclick={link.callback(|_| Msg::Setting("Load".to_string()))}>{"Load"}</a>
                    <hr/>
                    <a onclick={link.callback(|_| Msg::Setting("Drive".to_string()))}>{"Drive"}</a>
                </div>
            </div>
        }
    }
}
