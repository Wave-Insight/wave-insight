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
    /// The tree the sidebar was opened with; when the app hands over a new root
    /// (a dump was opened, or the design was re-elaborated) the selection is
    /// reset instead of pointing into the old tree.
    root: Rc<Module>,
}

impl Component for ModuleStruct {
    type Message = Msg;
    type Properties = ModuleStructProps;

    fn create(ctx: &Context<Self>) -> Self {
        Self {
            show_module: initial_module(&ctx.props().module),
            root: ctx.props().module.clone(),
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        if !Rc::ptr_eq(&self.root, &ctx.props().module) {
            self.root = Rc::clone(&ctx.props().module);
            self.show_module = initial_module(&ctx.props().module);
        }
        true
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
            <div class="side-stack">
                <section class="panel">
                    <div class="panel-head">{"Modules"}</div>
                    <div class="panel-body">
                        { show_module(module,0,&callback) }
                    </div>
                </section>
                <section class="panel panel-grow">
                    <div class="panel-head">
                        {"Signals"}
                        <span class="count">{self.show_module.signal.len().to_string()}</span>
                    </div>
                    <SignalStruct module={self.show_module.clone()} signaladd={ctx.link().callback(Msg::SignalAdd)}/>
                </section>
            </div>
        }
    }
}

/// The scope the signal list starts on.
///
/// The root of a dump is a virtual scope without signals, so descend through
/// single child scopes until something with signals (or a real fork) is found.
fn initial_module(root: &Rc<Module>) -> Rc<Module> {
    let mut current = Rc::clone(root);
    for _ in 0..8 {
        if !current.signal.is_empty() || current.sub_module.len() != 1 {
            break;
        }
        match current.sub_module.iter().next() {
            Some((_name, child)) => current = Rc::new(child.clone()),
            None => break,
        }
    }
    current
}
