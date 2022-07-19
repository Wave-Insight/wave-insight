use std::rc::Rc;

use wave_insight_lib::data_struct::Module;
use yew::prelude::*;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleComponentProps {
    pub space: String,
    pub name: String,
    pub module: Rc<Module>,
    #[prop_or_default]
    pub onclick: Callback<Rc<Module>>,
}

pub enum Msg {
    ClickModule,
}

pub struct ModuleComponent {
    space: String,
    name: String,
}

impl Component for ModuleComponent {
    type Message = Msg;
    type Properties = ModuleComponentProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        Self {
            space: props.space.clone(),
            name: props.name.clone(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::ClickModule => {
                ctx.props().onclick.emit(ctx.props().module.clone());
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        html! {
            <div>
                <h3 style="line-height:0.4;white-space:pre" onclick={ctx.link().callback(|_| Msg::ClickModule)}>{self.space.clone()+&self.name}</h3>
            </div>
        }
    }
}
