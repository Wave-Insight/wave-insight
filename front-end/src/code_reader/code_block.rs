use std::cell::RefCell;
use std::rc::Rc;

use web_sys::HtmlElement;
use yew::prelude::*;

use crate::code_reader::monaco_glue::{EditorOptions, MonacoEditor};
use crate::lsp::{document_uri, file_of_uri, install, LspBridge, LANGUAGE_ID};
use crate::nav::{Goto, SourceClick};

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CodeBlockProps {
    /// Name of the file `text` belongs to; it is reported with every click so
    /// the design database can resolve the position.
    pub file: String,
    pub text: String,
    /// Where to put the cursor; `None` until the user asked for a position.
    #[prop_or_default]
    pub goto: Option<Goto>,
    /// A plain left click on an identifier.
    #[prop_or_default]
    pub onwordclick: Callback<SourceClick>,
    /// The page's language server client: every document the editor shows is
    /// opened (and kept in sync) here, and the monaco providers answer from it.
    pub lsp: LspBridge,
}

pub struct CodeBlock {
    /// The element monaco mounts itself into; it survives re-renders, so the
    /// editor is created once and only its model is switched afterwards.
    host: NodeRef,
    editor: Option<MonacoEditor>,
    /// The document the editor is showing, so prop changes can be diffed.
    shown_file: Option<String>,
    /// The text the app last handed to that document.
    shown_text: Option<String>,
    /// Token of the jump request that was already applied.
    shown_token: Option<u32>,
    /// File name and click callback as the JS listener must see them *now*:
    /// the same editor is reused when another tab is activated.
    file: Rc<RefCell<String>>,
    click: Rc<RefCell<Callback<SourceClick>>>,
}

impl Component for CodeBlock {
    type Message = ();
    type Properties = CodeBlockProps;

    fn create(ctx: &Context<Self>) -> Self {
        Self {
            host: NodeRef::default(),
            editor: None,
            shown_file: None,
            shown_text: None,
            shown_token: None,
            file: Rc::new(RefCell::new(ctx.props().file.clone())),
            click: Rc::new(RefCell::new(ctx.props().onwordclick.clone())),
        }
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        let props = ctx.props();

        // keep the values the JS callback reads in sync with the active tab
        *self.file.borrow_mut() = props.file.clone();
        *self.click.borrow_mut() = props.onwordclick.clone();

        // The editor can only be mounted once the host div exists.
        if self.editor.is_none() {
            self.mount_editor(props);
            return;
        }

        let Some(editor) = &self.editor else {
            return;
        };

        // Another file tab was activated, or the app re-read the file. A user
        // edit is *not* a prop change, so typing never resets the buffer.
        let file_changed = self.shown_file.as_deref() != Some(props.file.as_str());
        let text_changed = self.shown_text.as_deref() != Some(props.text.as_str());
        if !props.file.is_empty() && (file_changed || text_changed) {
            editor.show_document(&document_uri(&props.file), LANGUAGE_ID, &props.text);
            self.shown_file = Some(props.file.clone());
            self.shown_text = Some(props.text.clone());
        }

        if let Some(goto) = &props.goto {
            if self.shown_token != Some(goto.token) {
                // The user clicked another declaration or driver/load.
                editor.reveal_position_in_center(goto.line, goto.col);
                self.shown_token = Some(goto.token);
            }
        }
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        // `MonacoEditor` disposes the JS editor when dropped.
        self.editor = None;
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        html! {
            <div class="monaco-wrap">
                <div ref={self.host.clone()} style="width: 100%; height: 100%;"></div>
            </div>
        }
    }
}

impl CodeBlock {
    /// Create the editor for the current props; called from the first `rendered`.
    fn mount_editor(&mut self, props: &CodeBlockProps) {
        let Some(host) = self.host.cast::<HtmlElement>() else {
            return;
        };

        // the listener outlives this call, so it must not borrow from `self`
        let file = Rc::clone(&self.file);
        let click = Rc::clone(&self.click);
        let on_click = Box::new(move |line: u32, col: u32, word: String| {
            let callback = click.borrow().clone();
            let file = file.borrow().clone();
            if file.is_empty() || word.is_empty() {
                return;
            }
            callback.emit(SourceClick {
                file,
                line,
                col,
                word,
            });
        });

        // `monaco` has no plain `verilog` language; its SystemVerilog grammar
        // covers Verilog as well and is what gives the pane its colouring.
        // The buffer starts empty: the text lives in the per-file document
        // model that `show_document` attaches right below.
        let options = EditorOptions::new(String::new(), LANGUAGE_ID, "vs-dark");
        let editor = MonacoEditor::create_interactive(&host, options, on_click);

        // Every buffer change (a user edit, or the app replacing the text)
        // goes to the language server as a `didChange`.
        let lsp = props.lsp.clone();
        editor.on_content_change(Box::new(move |uri, text| {
            if let Some(file) = file_of_uri(&uri) {
                lsp.open(&file, &text);
            }
        }));

        // definition / references / hover / documentSymbol on `systemverilog`
        install(&editor, props.lsp.clone());

        if !props.file.is_empty() {
            editor.show_document(&document_uri(&props.file), LANGUAGE_ID, &props.text);
            self.shown_file = Some(props.file.clone());
            self.shown_text = Some(props.text.clone());
        }

        // The handle remembers the position until monaco finished downloading.
        if let Some(goto) = &props.goto {
            editor.reveal_position_in_center(goto.line, goto.col);
            self.shown_token = Some(goto.token);
        }

        self.editor = Some(editor);
    }
}
