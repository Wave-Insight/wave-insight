use yew::prelude::*;

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
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                <pre>
                <code style="background-color:#dadada;display:block;height:400px;overflow-y:auto">
                    //<span style="color:#945eb8">{"    abcde"}</span>
                    {
                        for text.lines().map(|l| {
                            html!{
                                <span style="color:#945eb8">{l.to_owned()+"\n"}</span>
                            }
                        })
                    }
                </code>
                </pre>
            </div>
        }
    }
}
