//! The signal list of the selected scope: filtering plus a virtualized list.
//!
//! A 105 MB dump declares tens of thousands of signals in one scope; drawing a
//! DOM row per signal was the sidebar's freeze. Only the rows inside the scroll
//! window (plus an overscan) are drawn, with spacers above and below so the
//! scrollbar keeps the real length; the math is in [`super::super::virtual_list`].
//! Lists up to [`virtual_list::VIRTUALIZE_ABOVE`] rows are drawn whole, exactly
//! as before, so the QA matrix's small fixtures keep the same DOM.
//!
//! The component takes the tree and the selected path instead of a cloned
//! `Vec<SignalRow>`: cloning a 20k-signal scope on every parent render was part
//! of the cost, and the virtualized list only needs the slice of the window.
//! The logical (filtered) list is published through [`super::super::seam`] so
//! the QA matrix can still read it while the DOM stays bounded.

use std::cell::Cell;

use web_sys::{Event, HtmlInputElement};
use yew::prelude::*;

use super::search::{compile_regex, matches_plain, MatchOptions};
use super::SignalComponent;
use crate::module_struct::seam;
use crate::module_struct::virtual_list::{self, ROW_H};
use crate::nav::{SidebarTree, SignalRow};

pub enum Msg {
    GetClick(String),
    SetFilter(String),
    /// `Aa`: match case exactly.
    ToggleCase,
    /// `ab`: only whole words.
    ToggleWholeWord,
    /// `.*`: treat the query as a regular expression.
    ToggleRegex,
    /// The `x` button: empty the query, keeping the toggles.
    Clear,
    /// The list scrolled or was measured: (scroll top, viewport height).
    Scroll(f64, f64),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalStructProps {
    /// The sidebar tree; the list is the signals declared in `selected`.
    pub tree: SidebarTree,
    /// Full path of the scope whose signals are listed.
    pub selected: String,
    /// A signal was clicked: its hierarchical path.
    #[prop_or_default]
    pub signaladd: Callback<String>,
}

pub struct SignalStruct {
    filter: String,
    options: MatchOptions,
    scroll_top: f64,
    viewport_h: f64,
    /// The window the last render drew, published to the seam from rendered().
    window: Cell<virtual_list::Window>,
    /// The logical list changed (scope, filter, toggles): rewrite the seam
    /// array after the render. Scrolling does not, which is what keeps the seam
    /// from rebuilding 20k names per frame.
    dirty: bool,
    /// The tree/scope the cached scroll state belongs to.
    last_tree: Option<SidebarTree>,
    last_selected: String,
    /// The scrolling list, the signal list's viewport.
    scroll_ref: NodeRef,
}

impl Component for SignalStruct {
    type Message = Msg;
    type Properties = SignalStructProps;

    fn create(ctx: &Context<Self>) -> Self {
        Self {
            filter: "".to_string(),
            // the pre-toggle behaviour: case-insensitive substring
            options: MatchOptions::default(),
            scroll_top: 0.0,
            viewport_h: 0.0,
            window: Cell::new(virtual_list::Window::EMPTY),
            dirty: true,
            last_tree: Some(ctx.props().tree.clone()),
            last_selected: ctx.props().selected.clone(),
            scroll_ref: NodeRef::default(),
        }
    }

    fn changed(&mut self, ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
        // A pointer compare on the tree handle: the design/dump tree is an Rc
        // and only replaced when it really changed.
        if self.last_tree.as_ref() != Some(&ctx.props().tree)
            || self.last_selected != ctx.props().selected
        {
            self.last_tree = Some(ctx.props().tree.clone());
            self.last_selected = ctx.props().selected.clone();
            self.dirty = true;
            return true;
        }
        false
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        let window = self.window.get();
        if self.dirty {
            let signals = self.signals(ctx);
            // the seam carries the filtered list the unvirtualized DOM showed
            let names: Vec<String> = match self.visible_indices(signals) {
                Ok(None) => signals.iter().map(SignalRow::display_name).collect(),
                Ok(Some(indices)) => indices
                    .iter()
                    .map(|&index| signals[index].display_name())
                    .collect(),
                Err(()) => Vec::new(),
            };
            seam::publish_signals(
                names.iter().map(String::as_str),
                signals.len(),
                window.len(),
                window.windowed,
            );
            self.dirty = false;
        } else {
            seam::publish_signal_window(window.len(), window.windowed);
        }
        // The first paint and a scope change change the measured viewport; the
        // browser also clamps scrollTop when the list gets shorter, and the
        // window has to follow what the DOM actually shows.
        if let Some(element) = self.scroll_ref.cast::<web_sys::Element>() {
            let height = element.client_height() as f64;
            let top = element.scroll_top() as f64;
            if (height - self.viewport_h).abs() > 0.5 || (top - self.scroll_top).abs() > 0.5 {
                ctx.link().send_message(Msg::Scroll(top, height));
            }
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::GetClick(path) => {
                props.signaladd.emit(path);
                true
            }
            Msg::SetFilter(s) => {
                if self.filter == s {
                    return false;
                }
                self.filter = s;
                self.dirty = true;
                true
            }
            Msg::ToggleCase => {
                self.options.case_sensitive = !self.options.case_sensitive;
                self.dirty = true;
                true
            }
            Msg::ToggleWholeWord => {
                self.options.whole_word = !self.options.whole_word;
                self.dirty = true;
                true
            }
            Msg::ToggleRegex => {
                self.options.regex = !self.options.regex;
                self.dirty = true;
                true
            }
            Msg::Clear => {
                if self.filter.is_empty() {
                    return false;
                }
                self.filter.clear();
                self.dirty = true;
                true
            }
            Msg::Scroll(top, height) => {
                if (height - self.viewport_h).abs() <= 0.5 && (top - self.scroll_top).abs() <= 0.5 {
                    return false;
                }
                self.scroll_top = top;
                self.viewport_h = height;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let signals = self.signals(ctx);
        let query = self.filter.trim();
        let (indices, invalid) = match self.visible_indices(signals) {
            Ok(indices) => (indices, false),
            Err(()) => (Some(Vec::new()), true),
        };
        let count = indices.as_ref().map(Vec::len).unwrap_or(signals.len());
        let window = virtual_list::window(count, self.viewport_h, self.scroll_top, ROW_H);
        self.window.set(window);
        let rows: Vec<&SignalRow> = match &indices {
            None => signals[window.first..window.last].iter().collect(),
            Some(matching) => matching[window.first..window.last]
                .iter()
                .map(|&index| &signals[index])
                .collect(),
        };
        let onscroll = {
            let scroll_ref = self.scroll_ref.clone();
            link.callback(move |_: Event| {
                let (top, height) = scroll_ref
                    .cast::<web_sys::Element>()
                    .map(|element| (element.scroll_top() as f64, element.client_height() as f64))
                    .unwrap_or((0.0, 0.0));
                Msg::Scroll(top, height)
            })
        };
        let flag = |on: bool| if on { "btn active" } else { "btn" };
        html! {
            <div class="sig-panel">
                <div class="sig-scroll" ref={self.scroll_ref.clone()} onscroll={onscroll}>
                {
                    if window.top > 0.0 {
                        html! {
                            <div class="sig-spacer"
                                style={format!("height:{:.0}px", window.top)}></div>
                        }
                    } else {
                        html! {}
                    }
                }
                {
                    for rows.into_iter().map(|signal| { html!{
                        <SignalComponent
                            name={signal.display_name()}
                            width={signal.width}
                            path={signal.path.clone()}
                            onclick={link.callback(Msg::GetClick)}/>
                    }})
                }
                {
                    if window.bottom > 0.0 {
                        html! {
                            <div class="sig-spacer"
                                style={format!("height:{:.0}px", window.bottom)}></div>
                        }
                    } else {
                        html! {}
                    }
                }
                if count == 0 && !query.is_empty() {
                    <div class="panel-note">{if invalid { "invalid regular expression" } else { "no signal matches" }}</div>
                }
                </div>
                <div class="filter-wrap">
                    <input type="text" placeholder="filter signals…" value={self.filter.clone()}
                        oninput={link.callback(|e: InputEvent| Msg::SetFilter(e.target_unchecked_into::<HtmlInputElement>().value()))} />
                    // inline layout: styles.css is outside this change's scope,
                    // so the row cannot rely on a new rule yet
                    <div class="search-flags" style="display:flex;gap:6px;align-items:center;margin-top:6px">
                        <button type="button" class={flag(self.options.case_sensitive)}
                            title="Match case"
                            onclick={link.callback(|_| Msg::ToggleCase)}>{"Aa"}</button>
                        <button type="button" class={flag(self.options.whole_word)}
                            title="Whole word only"
                            onclick={link.callback(|_| Msg::ToggleWholeWord)}>{"ab"}</button>
                        <button type="button" class={flag(self.options.regex)}
                            title="Regular expression"
                            onclick={link.callback(|_| Msg::ToggleRegex)}>{".*"}</button>
                        <button type="button" class="btn" title="Clear the search"
                            onclick={link.callback(|_| Msg::Clear)}>{"×"}</button>
                    </div>
                </div>
            </div>
        }
    }
}

impl SignalStruct {
    /// The signals of the selected scope, read from the tree the props carry
    /// (no clone of the list).
    fn signals<'a>(&self, ctx: &'a Context<Self>) -> &'a [SignalRow] {
        ctx.props()
            .tree
            .find(&ctx.props().selected)
            .map(|scope| scope.signals.as_slice())
            .unwrap_or(&[])
    }

    /// The indices of the signals matching the search bar, or None when the
    /// query matches everything (the unfiltered fast path: the window is then a
    /// contiguous slice).
    ///
    /// Err is an invalid regular expression: the list shows nothing and the
    /// search bar says why (the pre-virtualization behaviour).
    fn visible_indices(&self, signals: &[SignalRow]) -> Result<Option<Vec<usize>>, ()> {
        let query = self.filter.trim();
        if query.is_empty() {
            return Ok(None);
        }
        if self.options.regex {
            let Some(regex) = compile_regex(query, self.options) else {
                return Err(());
            };
            Ok(Some(
                (0..signals.len())
                    .filter(|&index| regex.test(&signals[index].name))
                    .collect(),
            ))
        } else {
            Ok(Some(
                (0..signals.len())
                    .filter(|&index| matches_plain(&signals[index].name, query, self.options))
                    .collect(),
            ))
        }
    }
}
