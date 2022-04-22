use yew::prelude::*;
use monaco::{api::CodeEditorOptions, sys::editor::BuiltinTheme, yew::CodeEditor};

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CodeBlockProps {
    pub text: String,
}

pub struct CodeBlock {
}

impl Component for CodeBlock {
    type Message = ();
    type Properties = CodeBlockProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let text = &ctx.props().text;

        fn get_options(text: &str) -> CodeEditorOptions {
            CodeEditorOptions::default()
            .with_language("verilog".to_owned())
            .with_value(text.to_owned())
            .with_builtin_theme(BuiltinTheme::VsDark)
        }

        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div style="background-color:#dadada;display:block;height:100%;overflow-y:auto">//;height:400px
                <CodeEditor options={ get_options(text).to_sys_options() } />
            </div>
        }
    }
}
