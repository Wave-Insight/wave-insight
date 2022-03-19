use yew::prelude::*;

use wave_insight_lib::data_struct::Module;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleStructProps {
    pub module: Module,
}

pub struct ModuleStruct {
    module: Vec<String>,//TODO:
}

impl Component for ModuleStruct {
    type Message = ();
    type Properties = ModuleStructProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            module: vec![],
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let module = &ctx.props().module;
        self.module = (&module.sub_module).iter().map(|x| x.0.to_string()).collect();
        true
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                {
                    for (self.module).iter().map(|x| {
                        html! {<p>{x}</p>}
                    })
                }
                <p>{ self.module.len() }</p>//TODO:for debug
                <p>{ "module struct" }</p>
            </div>
        }
    }
}
