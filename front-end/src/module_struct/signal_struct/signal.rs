use web_sys::HtmlInputElement;
use yew::prelude::*;

use super::SignalComponent;
use crate::nav::SignalRow;

pub enum Msg {
    GetClick(String),
    SetFilter(String),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalStructProps {
    /// Signals declared in the selected scope.
    pub signals: Vec<SignalRow>,
    /// A signal was clicked: its hierarchical path.
    #[prop_or_default]
    pub signaladd: Callback<String>,
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
            Msg::GetClick(path) => {
                props.signaladd.emit(path);
                true
            }
            Msg::SetFilter(s) => {
                self.filter = s;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let filter = self.filter.to_ascii_lowercase();
        html! {
            <div class="sig-panel">
                <div class="sig-scroll">
                {
                    for ctx.props().signals.iter()
                        .filter(|signal| filter.is_empty() || signal.name.to_ascii_lowercase().contains(&filter))
                        .map(|signal| { html!{
                        <SignalComponent
                            name={signal.display_name()}
                            width={signal.width}
                            path={signal.path.clone()}
                            onclick={ctx.link().callback(Msg::GetClick)}/>
                    }})
                }
                </div>
                <div class="filter-wrap">
                    <input type="text" placeholder="filter signals…"
                        oninput={ctx.link().callback(|e: InputEvent| Msg::SetFilter(e.target_unchecked_into::<HtmlInputElement>().value()))} />
                </div>
            </div>
        }
    }
}
