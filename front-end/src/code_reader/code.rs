use yew::prelude::*;

use crate::code_reader::code_block::CodeBlock;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CodeReaderProps {
    pub file: Vec<(String,String)>,
    pub line: u32,
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
            <div class="code-pane">
                <nav class="tabs">
                {
                    for props.file.iter().enumerate().map(|(idx,(file_name,_text))| {
                        let class = if idx == self.activate_index {"tab active"} else {"tab"};
                        html! {
                            <button type="button" class={class}
                                onclick={link.callback(move |_| Msg::SetActive(idx))}>
                                {file_name.clone()}
                            </button>
                        }
                    })
                }
                </nav>
                <div class="code-host">
                    <CodeBlock text={ text.clone() } line={ props.line }/>
                </div>
            </div>
        }
    }
}
