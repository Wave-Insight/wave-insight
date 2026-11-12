use yew::prelude::*;

pub enum Msg {
    ContextMenu(MouseEvent),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalNameProps {
    /// The visible label: the leaf signal name by default, the full instance
    /// path when the label option is on.
    pub name: String,
    /// The full label (hierarchical path plus the bus range). It always goes
    /// into the tooltip, so the path is one hover away even while only the leaf
    /// name is shown. Empty for callers that have nothing more to show.
    #[prop_or_default]
    pub full: String,
    pub choose: bool,
    /// Distance of this row from the top of the row list, in css pixels.
    ///
    /// The list is virtualized: only the rows around the viewport exist in the
    /// dom, and they are placed absolutely at `top`.
    #[prop_or_default]
    pub top: f64,
    #[prop_or_default]
    pub menu: Callback<()>,
    #[prop_or_default]
    pub onclick: Callback<()>,
    /// Open the declaration of this signal in the source pane.
    #[prop_or_default]
    pub onjump: Callback<()>,
    /// The value at cursor 1, shown in the value column that ends the label
    /// cell (Verdi/GTKWave convention; it used to float on the canvas next to
    /// the cursor line). `None` while cursor 1 is not set: the column is not
    /// rendered at all then, so no dead slot is reserved before the first click.
    #[prop_or_default]
    pub value: Option<String>,
    /// The value at cursor 2. `Some` only while the second cursor exists, so the
    /// second column only takes space when it has something to show.
    #[prop_or_default]
    pub value2: Option<String>,
}

pub struct SignalName {}

impl Component for SignalName {
    type Message = Msg;
    type Properties = SignalNameProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {}
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::ContextMenu(event) => {
                event.prevent_default();
                props.menu.emit(());
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let class = if ctx.props().choose {
            "wave-name-cell chosen"
        } else {
            "wave-name-cell"
        };
        let style = format!("top:{:.1}px", ctx.props().top);
        let value = ctx.props().value.clone();
        let value2 = ctx.props().value2.clone();
        // the path stays discoverable even when only the leaf name is displayed
        let title = if ctx.props().full.is_empty() {
            ctx.props().name.clone()
        } else {
            ctx.props().full.clone()
        };
        html! {
            <div class={class} style={style}
                role="option" aria-selected={if ctx.props().choose {"true"} else {"false"}} tabindex="0">
                <p class="wave-name-text" title={title}
                    oncontextmenu={link.callback(Msg::ContextMenu)}
                    onclick={ctx.props().onclick.reform(|_: MouseEvent| ())}>
                    {&ctx.props().name}
                </p>
                // The hover-only actions sit between the name and the values:
                // after the values they reserved an invisible strip of blank
                // space in front of the waveform (the Wave 18 user report).
                <span class="wave-more" title="Go to declaration"
                    onclick={ctx.props().onjump.reform(|_: MouseEvent| ())}>{"↗"}</span>
                <span class="wave-more" title="Signal settings"
                    onclick={ctx.props().menu.reform(|_: MouseEvent| ())}>{"⋯"}</span>
                {
                    // The cursor values are the last thing in the cell, flush to
                    // the waveform; their width is the pane's `--wave-value-ch`
                    // (one shared width, so the columns line up per row and the
                    // cell never reserves one for a cursor that does not exist).
                    if let Some(value) = value {
                        html!{<span class="wave-value cursor-a" title={value.clone()}>{value}</span>}
                    } else {
                        html!{}
                    }
                }
                {
                    if let Some(value2) = value2 {
                        html!{<span class="wave-value cursor-b" title={value2.clone()}>{value2}</span>}
                    } else {
                        html!{}
                    }
                }
            </div>
        }
    }
}
