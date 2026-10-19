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
    /// What this row was expanded from; a new tree (another dump, another
    /// elaboration) reuses the component position, so the expansion state has to
    /// be dropped with it.
    module: Rc<Module>,
    show_submodule: bool,
}

impl Component for ModuleComponent {
    type Message = Msg;
    type Properties = ModuleComponentProps;

    fn create(ctx: &Context<Self>) -> Self {
        Self {
            module: Rc::clone(&ctx.props().module),
            show_submodule: false,
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        if !Rc::ptr_eq(&self.module, &ctx.props().module) {
            self.module = Rc::clone(&ctx.props().module);
            self.show_submodule = false;
        }
        true
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
        // read the props, not a copy taken in `create`: the row is reused when
        // the app hands over another tree
        let props = ctx.props();
        let empty = props.module.sub_module.is_empty();
        let caret = if empty {
            ""
        }else if self.show_submodule {
            "▾"
        }else {
            "▸"
        };
        html! {
            <div>
                <div class="tree-row" onclick={ctx.link().callback(|_| Msg::ClickThis)}>
                    <span class={if self.show_submodule {"tree-caret open"} else {"tree-caret"}}>{caret}</span>
                    <span class="tree-name">{props.space.clone()+&props.name}</span>
                </div>
                {if self.show_submodule {html!{
                    for (props.module.sub_module).iter().map(|x| {
                        let space = props.space.clone() + "  ";
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
