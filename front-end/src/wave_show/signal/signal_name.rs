use yew::prelude::*;

pub enum Msg {
    ContextMenu(MouseEvent),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalNameProps {
    pub name: String,
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
    /// The value at cursor 1, shown in the value column between the name column
    /// and the waveform (Verdi/GTKWave convention; it used to float on the
    /// canvas next to the cursor line).
    #[prop_or_default]
    pub value: Option<String>,
    /// The value at cursor 2. `Some` only while the second cursor exists, so the
    /// second column only takes space when it has something to show.
    #[prop_or_default]
    pub value2: Option<String>,
}

/// Inline style of one value column.
///
/// `styles.css` is owned by the UI pass, so the column is styled inline here: a
/// fixed, right aligned monospace slot, tinted with the colour of the cursor it
/// belongs to and separated from the name by the same rule the table uses.
/// Moving this into a `.wave-value` rule is a pure cosmetics follow-up.
fn value_style(cursor: usize) -> String {
    let color = if cursor == 1 {
        "var(--cursor-a)"
    } else {
        "var(--cursor-b)"
    };
    format!(
        "flex:0 0 54px;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;\
         text-align:right;font-family:var(--font-mono);font-size:10.5px;color:{color};\
         border-left:1px solid var(--line-soft);padding-left:6px;"
    )
}

pub struct SignalName {
}

impl Component for SignalName {
    type Message = Msg;
    type Properties = SignalNameProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::ContextMenu(event) => {
                event.prevent_default();
                props.menu.emit(());
                true
            },
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let class = if ctx.props().choose {"wave-name-cell chosen"} else {"wave-name-cell"};
        let style = format!("top:{:.1}px", ctx.props().top);
        let value = ctx.props().value.clone().unwrap_or_default();
        let value2 = ctx.props().value2.clone();
        html! {
            <div class={class} style={style}>
                <p class="wave-name-text" title={ctx.props().name.clone()}
                    oncontextmenu={link.callback(Msg::ContextMenu)}
                    onclick={ctx.props().onclick.reform(|_: MouseEvent| ())}>
                    {&ctx.props().name}
                </p>
                <span class="wave-value" style={value_style(1)} title={value.clone()}>{value}</span>
                {
                    if let Some(value2) = value2 {
                        html!{<span class="wave-value" style={value_style(2)} title={value2.clone()}>{value2}</span>}
                    } else {
                        html!{}
                    }
                }
                <span class="wave-more" title="Go to declaration"
                    onclick={ctx.props().onjump.reform(|_: MouseEvent| ())}>{"↗"}</span>
                <span class="wave-more" title="Signal settings"
                    onclick={ctx.props().menu.reform(|_: MouseEvent| ())}>{"⋯"}</span>
            </div>
        }
    }
}
