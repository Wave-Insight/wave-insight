//! The driver/load panel: for the selected signal it lists the declaration and
//! every reference that drives or reads it. Every row jumps to its span.

use yew::html::Scope;
use yew::prelude::*;

use super::{SharedDesign, SourceRow};

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalInspectorProps {
    pub design: SharedDesign,
    /// Design database version; changes when a source file was parsed, even when
    /// `design` itself is the same handle.
    pub version: u64,
    /// Hierarchical path of the signal selected in the wave view.
    #[prop_or_default]
    pub path: Option<String>,
    /// `(file, line, col)` of a row the user clicked.
    #[prop_or_default]
    pub ongoto: Callback<(String, u32, u32)>,
}

pub enum Msg {
    Goto((String, u32, u32)),
}

pub struct SignalInspector;

impl Component for SignalInspector {
    type Message = Msg;
    type Properties = SignalInspectorProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Goto(target) => {
                ctx.props().ongoto.emit(target);
                false
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let props = ctx.props();
        let link = ctx.link();

        // take an owned copy of everything; the database borrow must not be held
        // while the html is built
        let (details, has_sources, top, diagnostics, candidates) = {
            let design = props.design.borrow();
            let candidates = props
                .path
                .as_deref()
                .map(|path| {
                    design
                        .resolve_candidates(path)
                        .iter()
                        .take(4)
                        .map(|signal| signal.path.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (
                props.path.as_deref().and_then(|path| design.signal_details(path)),
                !design.is_empty(),
                design.top().map(str::to_string),
                design.diagnostic_count(),
                candidates,
            )
        };

        html! {
            <section class="panel" style="flex:0 0 auto;max-height:190px">
                <div class="panel-head">
                    {"Drivers / loads"}
                    // a signal path is case sensitive, so keep the panel head's
                    // `text-transform: uppercase` off it
                    <span class="count" style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap;text-transform:none"
                        title={top.clone().map(|top| format!("top: {top}")).unwrap_or_default()}>
                        {props.path.clone().unwrap_or_else(|| "—".to_string())}
                    </span>
                </div>
                <div class="panel-body">
                    {self.body(link, props, details, has_sources, top, diagnostics, candidates)}
                </div>
            </section>
        }
    }
}

impl SignalInspector {
    #[allow(clippy::too_many_arguments)]
    fn body(
        &self,
        link: &Scope<Self>,
        props: &SignalInspectorProps,
        details: Option<super::SignalDetails>,
        has_sources: bool,
        top: Option<String>,
        diagnostics: usize,
        candidates: Vec<String>,
    ) -> Html {
        let Some(path) = &props.path else {
            return html! {
                <div class="empty-hint" style="padding:14px 12px;text-align:left">
                    {"Select a signal in the waveform to see where it is declared, "}
                    {"what drives it and where it is read."}
                </div>
            };
        };
        if !has_sources {
            return html! {
                <div class="empty-hint" style="padding:14px 12px;text-align:left">
                    {"No design loaded. Drop the "}<kbd>{".sv"}</kbd>
                    {" / "}<kbd>{".v"}</kbd>{" sources to get declarations, drivers and loads."}
                </div>
            };
        }
        let Some(details) = details else {
            if candidates.len() > 1 {
                return html! {
                    <div class="empty-hint" style="padding:14px 12px;text-align:left">
                        <code>{path.clone()}</code>
                        {format!(" matches {} signals and is ambiguous:", candidates.len())}
                        <span style="display:block;color:var(--text-1)">{candidates.join(", ")}</span>
                        {" Pick the instance in the hierarchy to see its drivers and loads."}
                    </div>
                };
            }
            return html! {
                <div class="empty-hint" style="padding:14px 12px;text-align:left">
                    <code>{path.clone()}</code>
                    {" is not part of the elaborated design"}
                    {top.map(|top| html!{{format!(" (top: {top})")}}).unwrap_or_default()}
                    {" — load the sources the dump was made from."}
                </div>
            };
        };

        let mut width = details.width.map(|w| format!("{w} bit")).unwrap_or_else(|| "?".to_string());
        if let Some(direction) = &details.direction {
            width = format!("{direction} · {width}");
        }
        width = format!("{} · {width}", details.kind);

        html! {
            <>
                <div class="sig-row" style="cursor:default">
                    <span class="sig-name" title={details.path.clone()}>{details.path.clone()}</span>
                    <span class="sig-size">{width}</span>
                </div>
                {self.section(link, "Declaration", details.declaration.as_slice())}
                {self.section(link, &format!("Drivers ({})", details.drivers.len()), &details.drivers)}
                {self.section(link, &format!("Loads ({})", details.loads.len()), &details.loads)}
                {
                    if diagnostics > 0 {
                        html!{<div class="sig-row" style="cursor:default;color:var(--text-2)">
                            <span class="sig-name">{format!("{diagnostics} design diagnostic(s)")}</span>
                        </div>}
                    }else {html!{}}
                }
            </>
        }
    }

    fn section(&self, link: &Scope<Self>, title: &str, rows: &[SourceRow]) -> Html {
        html! {
            <>
                <p class="modal-label" style="padding:6px 12px 2px">{title.to_string()}</p>
                {
                    if rows.is_empty() {
                        html!{
                            <div class="sig-row" style="cursor:default;color:var(--text-2)">
                                <span class="sig-name">{"none"}</span>
                            </div>
                        }
                    }else {
                        html!{ for rows.iter().map(|row| self.row(link, row)) }
                    }
                }
            </>
        }
    }

    fn row(&self, link: &Scope<Self>, row: &SourceRow) -> Html {
        let (file, line, col) = row.target();
        html! {
            <div class="sig-row" title={format!("{} — {}", row.label, row.detail)}
                onclick={link.callback(move |_| Msg::Goto((file.clone(), line, col)))}>
                <span class="sig-name">{row.label.clone()}</span>
                <span class="sig-size">{row.detail.clone()}</span>
                <span class="sig-add">{"↗"}</span>
            </div>
        }
    }
}
