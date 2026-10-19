use web_sys::HtmlElement;
use yew::prelude::*;

use crate::code_reader::monaco_glue::MonacoEditor;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CodeBlockProps {
    pub text: String,
    pub line: u32,
}

pub struct CodeBlock {
    /// The element monaco mounts itself into; it survives re-renders, so the
    /// editor is created once and only its content is updated afterwards.
    host: NodeRef,
    editor: Option<MonacoEditor>,
    /// What the editor currently shows, so prop changes can be diffed.
    shown_text: Option<String>,
    shown_line: Option<u32>,
}

impl Component for CodeBlock {
    type Message = ();
    type Properties = CodeBlockProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            host: NodeRef::default(),
            editor: None,
            shown_text: None,
            shown_line: None,
        }
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        let props = ctx.props();

        // The editor can only be mounted once the host div exists.
        if self.editor.is_none() {
            self.mount_editor(props);
            return;
        }

        if self.shown_text.as_deref() != Some(props.text.as_str()) {
            // Another file tab was activated.
            if let Some(editor) = &self.editor {
                editor.set_value(&props.text);
                editor.reveal_line_in_center(props.line);
            }
            self.shown_text = Some(props.text.clone());
            self.shown_line = Some(props.line);
        } else if self.shown_line != Some(props.line) {
            // The user clicked another signal: jump to its definition.
            if let Some(editor) = &self.editor {
                editor.reveal_line_in_center(props.line);
            }
            self.shown_line = Some(props.line);
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

        // `monaco` has no plain `verilog` language; its SystemVerilog grammar
        // covers Verilog as well and is what gives the pane its colouring.
        let editor = MonacoEditor::create(&host, &props.text, "systemverilog", "vs-dark");
        // The handle remembers the line until monaco finished downloading.
        editor.reveal_line_in_center(props.line);

        self.shown_text = Some(props.text.clone());
        self.shown_line = Some(props.line);
        self.editor = Some(editor);
    }
}
