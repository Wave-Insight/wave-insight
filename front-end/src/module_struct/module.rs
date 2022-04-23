use std::rc::Rc;

use yew::prelude::*;

use wave_insight_lib::data_struct::{Module, Signal};
use crate::module_struct::ModuleComponent;

pub enum Msg {
    GetClick((String,Rc<Signal>)),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleStructProps {
    pub module: Rc<Module>,
    #[prop_or_default]
    pub signaladd: Callback<(String,Rc<Signal>)>,
}

pub struct ModuleStruct {
}

impl Component for ModuleStruct {
    type Message = Msg;
    type Properties = ModuleStructProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::GetClick(s) => {
                props.signaladd.emit(s);
                true
            },
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let module = &ctx.props().module;
        let link = ctx.link();
        
        let callback = link.callback(Msg::GetClick);
        fn show_module(m:&Module, level: i32, callback: &Callback<(String,Rc<Signal>)>) -> Html {
            html! {
                for (m.sub_module).iter().map(|x| {
                    let space = (0..level*2).map(|_| " ").fold("".to_string(),|a,b| a+b);
                    html! {
                        <div>
                            <ModuleComponent space={space.clone()} name={x.0.clone()} module={Rc::new(x.1.clone())} onclick={callback} />
                            {show_module(x.1,level+1,callback)}
                        </div>
                    }
                })
            }
        }
        
        html! {
            <div>
                {
                    show_module(module,0,&callback)
                }
            </div>
        }
    }
}
