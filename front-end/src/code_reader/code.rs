use yew::prelude::*;

use crate::code_reader::code_block::CodeBlock;
use crate::lsp::LspBridge;
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
    /// The page's language server client; the editor opens its documents here.
    pub lsp: LspBridge,
    /// A tab was closed; the app drops the source and the LSP gets `didClose`.
    #[prop_or_default]
    pub onclose: Callback<usize>,
}

pub enum Msg {
    SetActive(usize),
    Close(usize),
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

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::SetActive(idx) => {
                self.activate_index = idx;
                true
            }
            Msg::Close(idx) => {
                ctx.props().onclose.emit(idx);
                // the parent re-renders with one tab less; keep the index sane
                // even if it does not
                if self.activate_index >= ctx.props().file.len().saturating_sub(1) {
                    self.activate_index = self.activate_index.saturating_sub(1);
                }
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

        // the tab list can shrink under us when a tab is closed
        let active = self.activate_index.min(props.file.len().saturating_sub(1));
        let (file_name, text) = props
            .file
            .get(active)
            .map(|(name, text)| (name.clone(), text.clone()))
            .unwrap_or_else(|| (String::new(), String::new()));

        html! {
            <>
                <nav class="tabs">
                {
                    for props.file.iter().enumerate().map(|(idx,(name,_text))| {
                        let class = if idx == active {"tab active"} else {"tab"};
                        html! {
                            <button type="button" class={class}
                                onclick={link.callback(move |_| Msg::SetActive(idx))}>
                                <span class="tab-name">{name.clone()}</span>
                                <span class="tab-close" title="Close the tab"
                                    style="margin-left:10px;opacity:.55;padding:0 2px;"
                                    onclick={link.callback(move |event: MouseEvent| {
                                        // closing must not also activate the tab
                                        event.stop_propagation();
                                        Msg::Close(idx)
                                    })}>{"×"}</span>
                            </button>
                        }
                    })
                }
                </nav>
                <div class="code-host">
                    <CodeBlock file={file_name} text={text}
                        goto={props.goto.clone()}
                        onwordclick={props.onwordclick.clone()}
                        lsp={props.lsp.clone()} />
                </div>
            </>
        }
    }
}
