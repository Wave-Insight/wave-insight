use yew::prelude::*;

use material_yew::{MatTabBar, MatTab};
use crate::code_reader::code_block::CodeBlock;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CodeReaderProps {
    pub file: Vec<(String,String)>,
}

pub enum Msg {
    SetActive(usize),
}

pub struct CodeReader {
    activate_index: usize,
}

impl Component for CodeReader {
    type Message = Msg;
    type Properties = CodeReaderProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            activate_index: 0,
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::SetActive(idx) => {
                self.activate_index = idx;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let props = ctx.props();

        let text = &props.file.get(self.activate_index)
            .map(|(_s,o)| o.clone())
            .unwrap_or_else(|| ("".to_string()));

        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div style="height:50%;overflow-y:auto">
                <MatTabBar onactivated={link.callback(Msg::SetActive)}>
                {
                    if props.file.is_empty() {
                        html! {<MatTab label="" />}
                    } else {
                        html! {}
                    }
                }
                {
                    for (&props.file).iter().map(|(file_name,_text)| {
                        html! {<MatTab label={file_name.clone()} />}
                    })
                }
                </MatTabBar>
                <CodeBlock text={ text.clone() }/>
                //{ <Highlighted code={props.file[0].1} /> }
            </div>
        }
    }
}
