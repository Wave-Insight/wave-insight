use yew::prelude::*;

use wave_insight_lib::data_struct::Module;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleStructProps {
    pub module: Module,
}

pub struct ModuleStruct {
}

impl Component for ModuleStruct {
    type Message = ();
    type Properties = ModuleStructProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let module = &ctx.props().module;
        
        fn show_module(m:&Module, level: i32) -> Html {
            html! {
                for (m.sub_module).iter().map(|x| {
                    let space = (0..level*2).map(|_| " ").fold("".to_string(),|a,b| a+b);
                    let signals = x.1.signal.iter();
                    html! {
                        <div>
                            <h3 style="line-height:0.4;white-space:pre">{space.clone()+x.0}</h3>
                            {for signals.map(|s| html!{
                                <p style="line-height:0.2;white-space:pre">{space.clone()+s.0}</p>
                            })}
                            {show_module(x.1,level+1)}
                        </div>
                    }
                })
            }
        }
        
        html! {
            <div>
                {
                    show_module(module,0)
                }
            </div>
        }
    }
}
