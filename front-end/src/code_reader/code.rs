use wasm_bindgen::JsCast;
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
    /// A page-wide key: Ctrl+W / Ctrl+F4 closes the active tab.
    KeyDown(web_sys::KeyboardEvent),
}

pub struct CodeReader {
    activate_index: usize,
    /// Token of the jump request that was already applied, so a re-render that
    /// keeps the same request does not steal the tab back from the user.
    shown_token: Option<u32>,
    /// The window keydown listener (created once, dropped in `destroy`).
    keydown: Option<wasm_bindgen::closure::Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
}

impl Component for CodeReader {
    type Message = Msg;
    type Properties = CodeReaderProps;

    fn create(ctx: &Context<Self>) -> Self {
        let mut this = Self {
            activate_index: 0,
            shown_token: None,
            keydown: None,
        };
        this.attach_key_listener(ctx);
        this
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let (Some(window), Some(listener)) = (web_sys::window(), self.keydown.take()) {
            let _ = window
                .remove_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref());
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::SetActive(idx) => {
                self.activate_index = idx;
                true
            }
            Msg::KeyDown(event) => {
                // Ctrl+W / Ctrl+F4: close the active tab, unless the user is
                // typing in the editor (the editor owns its keys, and the
                // browser would close its own tab for Ctrl+W anyway)
                if typing_in_editor(&event) {
                    return false;
                }
                let key = event.key();
                let closes = event.ctrl_key() && (key == "w" || key == "W" || key == "F4");
                if closes && !ctx.props().file.is_empty() {
                    event.prevent_default();
                    let active = self
                        .activate_index
                        .min(ctx.props().file.len().saturating_sub(1));
                    ctx.props().onclose.emit(active);
                    if self.activate_index >= ctx.props().file.len().saturating_sub(1) {
                        self.activate_index = self.activate_index.saturating_sub(1);
                    }
                    return true;
                }
                false
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
                <nav class="tabs" role="tablist" aria-label="Open sources">
                {
                    for props.file.iter().enumerate().map(|(idx,(name,_text))| {
                        let class = if idx == active {"tab active"} else {"tab"};
                        html! {
                            // A div, not a button: the closer is a real button,
                            // and a button inside a button is invalid html.
                            <div class={class}
                                onclick={link.callback(move |_| Msg::SetActive(idx))}>
                                <button type="button" class="tab-name"
                                    role="tab" aria-selected={if idx == active {"true"} else {"false"}}
                                    title={name.clone()}
                                    onclick={link.callback(move |_| Msg::SetActive(idx))}>
                                    {name.clone()}
                                </button>
                                <button type="button" class="tab-close"
                                    aria-label={format!("Close {name}")}
                                    title="Close the tab"
                                    onclick={link.callback(move |event: MouseEvent| {
                                        // closing must not also activate the tab
                                        event.stop_propagation();
                                        Msg::Close(idx)
                                    })}>{"×"}</button>
                            </div>
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

impl CodeReader {
    /// Install the page-wide key listener that owns Ctrl+W / Ctrl+F4.
    fn attach_key_listener(&mut self, ctx: &Context<Self>) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let link = ctx.link().clone();
        let listener = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                link.send_message(Msg::KeyDown(event));
            },
        );
        if window
            .add_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref())
            .is_ok()
        {
            self.keydown = Some(listener);
        }
    }
}

/// Is the key event aimed at the source editor (or any text field)?
///
/// The same rule the wave pane uses: the editor owns its keys, so a Ctrl+W
/// typed into a comment must not close the tab it is written in.
fn typing_in_editor(event: &web_sys::KeyboardEvent) -> bool {
    let Some(target) = event.target() else {
        return false;
    };
    let Ok(element) = target.dyn_into::<web_sys::Element>() else {
        return false;
    };
    let mut current = Some(element);
    while let Some(element) = current {
        let tag = element.tag_name().to_ascii_lowercase();
        if tag == "input" || tag == "textarea" || tag == "select" {
            return true;
        }
        if element.has_attribute("contenteditable") {
            return true;
        }
        if element
            .get_attribute("class")
            .map(|class| class.contains("monaco-editor"))
            .unwrap_or(false)
        {
            return true;
        }
        current = element.parent_element();
    }
    false
}
