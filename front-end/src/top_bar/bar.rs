use yew::prelude::*;
use material_yew::{MatTopAppBarFixed, 
    top_app_bar_fixed::{MatTopAppBarNavigationIcon, MatTopAppBarTitle},
    MatIconButton};

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct TopBarProps {
    #[prop_or_default]
    pub onnavigationiconclick: Callback<()>,
}

pub struct TopBar {
}

impl Component for TopBar {
    type Message = ();
    type Properties = TopBarProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let props = ctx.props();
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                <MatTopAppBarFixed onnavigationiconclick={props.onnavigationiconclick.clone()}>
                    <MatTopAppBarNavigationIcon>
                        <MatIconButton icon="menu"></MatIconButton>
                    </MatTopAppBarNavigationIcon>
                    <MatTopAppBarTitle>
                        <div class="app-title">
                            <h1>{"Wave Insight"}</h1>
                        </div>
                    </MatTopAppBarTitle>
                </MatTopAppBarFixed>
            </div>
        }
    }
}
