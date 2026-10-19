use yew::prelude::*;

use crate::code_reader::code_block::CodeBlock;
use crate::nav::{Goto, SourceClick};

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CodeReaderProps {
    /// `(file name, text)` of every loaded source file, in load order.
    pub file: Vec<(String, String)>,
    /// A request to bring one position into view; the token makes repeats work.
    #[prop_or_default]
    pub goto: Option<Goto>,
    /// A plain left click on an identifier inside the editor.
    #[prop_or_default]
    pub onwordclick: Callback<SourceClick>,
}

pub enum Msg {
    SetActive(usize),
}

pub struct CodeReader {
    activate_index: usize,
    /// Token of the jump request that was already applied, so a re-render that
    /// keeps the same request does not steal the tab back from the user.
    shown_token: Option<u32>,
}

impl Component for CodeReader {
    type Message = Msg;
    type Properties = CodeReaderProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            activate_index: 0,
            shown_token: None,
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

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        if let Some(goto) = &ctx.props().goto {
            if self.shown_token != Some(goto.token) {
                self.shown_token = Some(goto.token);
                // activate the tab that contains the requested position; a file
                // that is not open is filtered out by the app
                if let Some(index) = ctx
                    .props()
                    .file
                    .iter()
                    .position(|(name, _)| name == &goto.file)
                {
                    self.activate_index = index;
                }
            }
        }
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let props = ctx.props();

        let (file_name, text) = props
            .file
            .get(self.activate_index)
            .map(|(name, text)| (name.clone(), text.clone()))
            .unwrap_or_else(|| (String::new(), String::new()));

        html! {
            <>
                <nav class="tabs">
                {
                    for props.file.iter().enumerate().map(|(idx,(name,_text))| {
                        let class = if idx == self.activate_index {"tab active"} else {"tab"};
                        html! {
                            <button type="button" class={class}
                                onclick={link.callback(move |_| Msg::SetActive(idx))}>
                                {name.clone()}
                            </button>
                        }
                    })
                }
                </nav>
                <div class="code-host">
                    <CodeBlock file={file_name} text={text}
                        goto={props.goto.clone()}
                        onwordclick={props.onwordclick.clone()} />
                </div>
            </>
        }
    }
}
