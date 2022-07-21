use std::rc::Rc;

use yew::prelude::*;

use wave_insight_lib::data_struct::{Module, Signal};
use crate::module_struct::{ModuleComponent, SignalStruct};

pub enum Msg {
    GetClick(Rc<Module>),
    SignalAdd((String,Rc<Signal>)),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleStructProps {
    pub module: Rc<Module>,
    #[prop_or_default]
    pub signaladd: Callback<(String,Rc<Signal>)>,
}

pub struct ModuleStruct {
    show_module: Rc<Module>,
}

impl Component for ModuleStruct {
    type Message = Msg;
    type Properties = ModuleStructProps;

    fn create(ctx: &Context<Self>) -> Self {
        Self {
            show_module: ctx.props().module.clone(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::GetClick(s) => {
                self.show_module = s;
                true
            },
            Msg::SignalAdd(s) => {
                props.signaladd.emit(s);
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let module = &ctx.props().module;
        let link = ctx.link();
        
        let callback = link.callback(Msg::GetClick);
        fn show_module(m:&Module, level: i32, callback: &Callback<Rc<Module>>) -> Html {
            html! {
                for (m.sub_module).iter().map(|x| {
                    let space = (0..level*2).map(|_| " ").fold("".to_string(),|a,b| a+b);
                    html! {
                        <div>
                            <ModuleComponent
                                space={space.clone()}
                                name={x.0.clone()}
                                module={Rc::new(x.1.clone())}
                                onclick={callback} />
                        </div>
                    }
                })
            }
        }
        
        html! {
            <div style="height:95%">
                <div style="display:block;height:50%;overflow-y:auto">
                {
                    show_module(module,0,&callback)
                }
                </div>
                <SignalStruct module={self.show_module.clone()} signaladd={ctx.link().callback(Msg::SignalAdd)}/>
            </div>
        }
    }
}
