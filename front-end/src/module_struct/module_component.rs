use yew::prelude::*;

use wave_insight_lib::data_struct::Module;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleComponentProps {
    pub space: String,
    pub name: String,
    pub module: Module,
    #[prop_or_default]
    pub onclick: Callback<Vec<String>>,
}

pub struct ModuleComponent {
    space: String,
    name: String,
}

impl Component for ModuleComponent {
    type Message = ();
    type Properties = ModuleComponentProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        Self {
            space: props.space.clone(),
            name: props.name.clone(),
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                <h3 style="line-height:0.4;white-space:pre">{self.space.clone()+&self.name}</h3>
            </div>
        }
    }
}
