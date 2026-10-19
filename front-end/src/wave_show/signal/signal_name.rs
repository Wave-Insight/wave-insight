use yew::prelude::*;

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
    /// Open the declaration of this signal in the source pane.
    #[prop_or_default]
    pub onjump: Callback<()>,
}

pub struct SignalName {
}

impl Component for SignalName {
    type Message = Msg;
    type Properties = SignalNameProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
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
        let class = if ctx.props().choose {"wave-name-cell chosen"} else {"wave-name-cell"};
        html! {
            <div class={class}>
                <p class="wave-name-text" title={ctx.props().name.clone()}
                    oncontextmenu={link.callback(Msg::ContextMenu)}
                    onclick={ctx.props().onclick.reform(|_: MouseEvent| ())}>
                    {&ctx.props().name}
                </p>
                <span class="wave-more" title="Go to declaration"
                    onclick={ctx.props().onjump.reform(|_: MouseEvent| ())}>{"↗"}</span>
                <span class="wave-more" title="Signal settings"
                    onclick={ctx.props().menu.reform(|_: MouseEvent| ())}>{"⋯"}</span>
            </div>
        }
    }
}
