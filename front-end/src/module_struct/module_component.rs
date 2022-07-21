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
    ClickThis,
    ClickModule(Rc<Module>),
}

pub struct ModuleComponent {
    space: String,
    name: String,
    show_submodule: bool,
}

impl Component for ModuleComponent {
    type Message = Msg;
    type Properties = ModuleComponentProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        Self {
            space: props.space.clone(),
            name: props.name.clone(),
            show_submodule: false,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::ClickThis => {
                self.show_submodule = !self.show_submodule;
                ctx.props().onclick.emit(ctx.props().module.clone());
                true
            }
            Msg::ClickModule(m) => {
                ctx.props().onclick.emit(m);
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let fold = if ctx.props().module.sub_module.is_empty() {
            ""
        }else if self.show_submodule {
            "| "
        }else {
            "> "
        };
        html! {
            <div>
                <h3 style="line-height:0.4;white-space:pre" onclick={ctx.link().callback(|_| Msg::ClickThis)}>
                    {self.space.clone()+fold+&self.name}
                </h3>
                {if self.show_submodule {html!{
                    for (ctx.props().module.sub_module).iter().map(|x| {
                        let space = self.space.clone() + "  ";
                        html! {
                            <div>
                                <ModuleComponent
                                    space={space.clone()}
                                    name={x.0.clone()}
                                    module={Rc::new(x.1.clone())}
                                    onclick={ctx.link().callback(Msg::ClickModule)} />
                            </div>
                        }
                    })
                }} else {html!{}}}
            </div>
        }
    }
}
