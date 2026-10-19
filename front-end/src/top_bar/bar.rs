use yew::prelude::*;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct TopBarProps {
    #[prop_or_default]
    pub onnavigationiconclick: Callback<()>,
    /// One line of feedback: the last navigation target or an error.
    #[prop_or_default]
    pub status: Option<String>,
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
        html! {
            <header class="topbar">
                <button class="icon-btn" type="button" title="Show / hide the sidebar"
                    onclick={props.onnavigationiconclick.reform(|_: MouseEvent| ())}>
                    <svg viewBox="0 0 24 24" width="18" height="18" aria-hidden="true">
                        <path d="M4 7h16M4 12h16M4 17h16" fill="none" stroke="currentColor"
                            stroke-width="1.8" stroke-linecap="round" />
                    </svg>
                </button>
                <div class="brand">
                    <span class="brand-mark">
                        <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true">
                            <path d="M2 15h4l2-8 4 13 3-9 2 4h5" fill="none" stroke="currentColor"
                                stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
                        </svg>
                    </span>
                    <span class="brand-name">{"Wave Insight"}</span>
                    <span class="brand-tag">{"VCD / VERILOG"}</span>
                </div>
                <div class="topbar-hint">
                    {
                        props.status.as_ref().map(|status| {
                            html!{<span class="brand-tag"
                                style="color:var(--accent);border-color:var(--accent-line);margin-right:10px">
                                {status.clone()}</span>}
                        }).unwrap_or_default()
                    }
                    <kbd>{"ctrl"}</kbd>{" + wheel to zoom · "}
                    <kbd>{"click a name"}</kbd>{" adds it · "}
                    <kbd>{"↗"}</kbd>{" opens its declaration · "}
                    <kbd>{"z"}</kbd>{" "}<kbd>{"f"}</kbd>{" · left / right click sets a cursor"}
                </div>
            </header>
        }
    }
}
