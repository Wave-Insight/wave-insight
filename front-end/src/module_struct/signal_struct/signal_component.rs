use yew::prelude::*;

pub enum Msg {
    Click,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalComponentProps {
    /// Name as shown in the list, with the index range for buses.
    pub name: String,
    /// Bit width, shown on the right.
    pub width: usize,
    /// Hierarchical path; the row key the wave view gets.
    pub path: String,
    #[prop_or_default]
    pub onclick: Callback<String>,
}

pub struct SignalComponent {}

impl Component for SignalComponent {
    type Message = Msg;
    type Properties = SignalComponentProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {}
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::Click => {
                props.onclick.emit(props.path.clone());
                true
            }
        }
    }

    fn changed(&mut self, _ctx: &Context<Self>) -> bool {
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        html! {
            <div class="sig-row" onclick={link.callback(|_| Msg::Click)}
                title={ctx.props().path.clone()}>
                <span class="sig-name">{&ctx.props().name}</span>
                <span class="sig-size">{ctx.props().width.to_string()}</span>
                <span class="sig-add">{"+"}</span>
            </div>
        }
    }
}
