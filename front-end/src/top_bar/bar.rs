use yew::prelude::*;

use crate::state::{select, Status, Store, Subscription};

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct TopBarProps {
    /// The app store: the bar subscribes to the status line itself, so a status
    /// change does not re-render the shell (R2 design doc §2.2).
    pub store: Store,
    #[prop_or_default]
    pub onnavigationiconclick: Callback<()>,
}

pub enum Msg {
    /// The status slice changed.
    Status(Option<Status>),
}

pub struct TopBar {
    status: Option<Status>,
    /// Dropping it unsubscribes; keep it for the lifetime of the bar.
    _status_sub: Subscription,
}

impl Component for TopBar {
    type Message = Msg;
    type Properties = TopBarProps;

    fn create(ctx: &Context<Self>) -> Self {
        let store = ctx.props().store.clone();
        let subscription = store.subscribe(select::status, ctx.link().callback(Msg::Status));
        Self {
            status: store.select(select::status),
            _status_sub: subscription,
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Status(status) => {
                self.status = status;
                true
            }
        }
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
                    // Which build this tab runs: after a deploy the first thing to
                    // check is that this number moved.
                    <span class="brand-version">{format!("v{}", env!("CARGO_PKG_VERSION"))}</span>
                </div>
                <div class="topbar-hint">
                    {
                        self.status.as_ref().map(|status| {
                            // errors are actionable text and get the danger colour,
                            // information stays in the accent colour
                            let style = if status.is_error() {
                                "color:var(--danger);border-color:var(--danger);margin-right:10px"
                            } else {
                                "color:var(--accent);border-color:var(--accent-line);margin-right:10px"
                            };
                            html!{<span class="brand-tag" style={style}
                                title={status.text.clone()}>{status.text.clone()}</span>}
                        }).unwrap_or_default()
                    }
                    <kbd>{"ctrl"}</kbd>{" + wheel to zoom · "}
                    <kbd>{"click a name"}</kbd>{" adds it · "}
                    <kbd>{"↗"}</kbd>{" opens its declaration · "}
                    <kbd>{"z"}</kbd>{" "}<kbd>{"f"}</kbd>{" · left / right click sets a cursor"}
                    {" · "}<kbd>{"del"}</kbd>{" removes the selected row"}
                </div>
            </header>
        }
    }
}
