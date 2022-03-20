use yew::prelude::*;

use wave_insight_lib::data_struct::{Module};
use crate::module_struct::{SignalComponent, ModuleComponent};
use web_sys::console;//TODO:for debug

pub enum Msg {
    GetClick(String),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleStructProps {
    pub module: Module,
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

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::GetClick(s) => {
                console::log_1(&format!("click,{}",s).into());
                true
            },
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let module = &ctx.props().module;
        let link = ctx.link();
        
        let callback = link.callback(move |p:(Vec<String>,String)| Msg::GetClick(p.1));
        fn show_module(m:&Module, level: i32, callback: &Callback<(Vec<String>,String)>) -> Html {
            html! {
                for (m.sub_module).iter().map(|x| {
                    let space = (0..level*2).map(|_| " ").fold("".to_string(),|a,b| a+b);
                    let signals = x.1.signal.iter();
                    html! {
                        <div>
                            <ModuleComponent space={space.clone()} name={x.0.clone()} module={x.1.clone()} />
                            {for signals.map(|s| html!{
                                <SignalComponent space={space.clone()} name={s.0.clone()} signal={s.1.clone()} onclick={callback}/>
                            })}
                            {show_module(x.1,level+1,callback)}//,&callback
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
