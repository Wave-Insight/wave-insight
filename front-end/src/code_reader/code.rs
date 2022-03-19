use yew::prelude::*;

pub struct CodeReader {
}

impl Component for CodeReader {
    type Message = ();
    type Properties = ();

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                <p>{ "code reader" }</p>
            </div>
        }
    }
}
