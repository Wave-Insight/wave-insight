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
//!
//! Wave 19: the rows select like the module tree's rows (plain click resets,
//! Ctrl/Cmd-click toggles, Shift-click ranges over the *filtered* order) and
//! adding is explicit: the row's "+", a double click, Enter, or the panel's
//! "Add selected (N)" button, which hands the whole selection to the app as ONE
//! batch ([`SignalStructProps::onaddmany`]). The selection itself is pure and
//! unit tested in [`super::select`].

use std::cell::Cell;

use web_sys::{Event, HtmlInputElement};
use yew::prelude::*;

use super::search::{compile_regex, matches_plain, MatchOptions};
use super::select::{SignalSelect, SignalSelection};
use super::SignalComponent;
use crate::module_struct::seam;
use crate::module_struct::virtual_list::{self, ROW_H};
use crate::nav::{SidebarTree, SignalRow};
use crate::state::bulk::BULK_ADD_CAP;

pub enum Msg {
    /// A row click: plain / Ctrl / Shift, resolved against the filtered order.
    Click(SignalSelect),
    /// The row's "+" or a double click: add this one signal immediately.
    Add(String),
    /// Enter on a focused row: add the list's selection when it has one, the
    /// focused row otherwise.
    Enter(String),
    /// The header / bulk bar "Add selected": the whole selection, in one batch.
    AddSelected,
    /// The header "Select all": every signal of the filtered list.
    SelectAll,
    /// The bulk bar's "Clear".
    ClearSelection,
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
    /// One signal was added explicitly (the row's "+", a double click or Enter):
    /// its hierarchical path.
    #[prop_or_default]
    pub signaladd: Callback<String>,
    /// "Add selected (N)": the selected signal paths, added by the app as ONE
    /// rows-added batch.
    #[prop_or_default]
    pub onaddmany: Callback<Vec<String>>,
    /// A signal row asked for its context menu.
    #[prop_or_default]
    pub onrowcontext: Callback<crate::module_struct::RowContext>,
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
    /// Wave 19: the explicit multi-selection ("Add selected"). Per list: a new
    /// tree/scope resets it, and the filter prunes paths it no longer shows.
    selection: SignalSelection,
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
            selection: SignalSelection::default(),
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
            // the new scope lists different signals: carrying a selection over
            // would silently add rows the user cannot see any more
            self.selection.clear();
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
            Msg::Click(select) => match select {
                SignalSelect::Reset(path) => self.selection.reset(&path),
                SignalSelect::Toggle(path) => self.selection.toggle(&path),
                SignalSelect::Range(path) => {
                    let signals = self.signals(ctx);
                    let visible = self.visible_paths(signals);
                    self.selection.range(&visible, &path)
                }
            },
            Msg::Add(path) => {
                // the explicit single add: one row, exactly the old click path
                props.signaladd.emit(path);
                true
            }
            Msg::Enter(path) => {
                if self.selection.is_empty() {
                    props.signaladd.emit(path);
                } else {
                    props.onaddmany.emit(self.selection.paths().to_vec());
                }
                true
            }
            Msg::AddSelected => {
                if self.selection.is_empty() {
                    return false;
                }
                props.onaddmany.emit(self.selection.paths().to_vec());
                true
            }
            Msg::SelectAll => {
                let signals = self.signals(ctx);
                let visible = self.visible_paths(signals);
                self.selection.select_all(&visible)
            }
            Msg::ClearSelection => self.selection.clear(),
            Msg::SetFilter(s) => {
                if self.filter == s {
                    return false;
                }
                self.filter = s;
                self.dirty = true;
                self.prune_selection(ctx);
                true
            }
            Msg::ToggleCase => {
                self.options.case_sensitive = !self.options.case_sensitive;
                self.dirty = true;
                self.prune_selection(ctx);
                true
            }
            Msg::ToggleWholeWord => {
                self.options.whole_word = !self.options.whole_word;
                self.dirty = true;
                self.prune_selection(ctx);
                true
            }
            Msg::ToggleRegex => {
                self.options.regex = !self.options.regex;
                self.dirty = true;
                self.prune_selection(ctx);
                true
            }
            Msg::Clear => {
                if self.filter.is_empty() {
                    return false;
                }
                self.filter.clear();
                self.dirty = true;
                self.prune_selection(ctx);
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
        let selected: std::collections::HashSet<&str> =
            self.selection.paths().iter().map(String::as_str).collect();
        let selected_count = self.selection.len();
        let over_cap = selected_count > BULK_ADD_CAP;
        let add_selected_title = if over_cap {
            format!(
                "Too many signals ({selected_count}) — the limit is {}; select fewer signal rows",
                BULK_ADD_CAP
            )
        } else {
            format!("Add {selected_count} signals from the selected signal rows")
        };
        let select_all_title = format!("Select all {count} visible signals for bulk add");
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
            // Wave 19: the panel head moved in here from ModuleStruct: the
            // select-all / add-selected controls need this list's own selection
            // state (the Modules head keeps the module-tree selection).
            <section class="panel panel-grow">
                <div class="panel-head">
                    {"Signals"}
                    <span class="count">{signals.len().to_string()}</span>
                    <button type="button" class="tree-select-all sig-select-all"
                        aria-label="Select all visible signals"
                        title={select_all_title}
                        disabled={count == 0}
                        onclick={link.callback(|_: MouseEvent| Msg::SelectAll)}>
                        {"Select all"}
                    </button>
                    <button type="button"
                        class={if over_cap {"tree-add-selected sig-add-selected over-cap"} else {"tree-add-selected sig-add-selected"}}
                        aria-label="Add selected signals to the wave"
                        aria-disabled={if over_cap {"true"} else {"false"}}
                        title={add_selected_title.clone()}
                        disabled={selected_count == 0}
                        onclick={link.callback(|_: MouseEvent| Msg::AddSelected)}>
                        {format!("Add selected ({selected_count})")}
                    </button>
                </div>
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
                                selected={selected.contains(signal.path.as_str())}
                                onselect={link.callback(Msg::Click)}
                                onadd={link.callback(Msg::Add)}
                                onenter={link.callback(Msg::Enter)}
                                onrowcontext={ctx.props().onrowcontext.clone()}/>
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
                    // the sticky bulk bar is a footer like the module tree's: it
                    // must not push the rows (no bar above the list)
                    {if selected_count > 0 {
                        html!{
                        <div class="bulk-bar sig-bulk-bar" role="status" aria-label="Signal selection">
                            <span class="bulk-count">{format!("{selected_count} signals selected")}</span>
                            <button type="button" class="bulk-add sig-bulk-add"
                                aria-label="Add selected signals to the wave"
                                aria-disabled={if over_cap {"true"} else {"false"}}
                                title={add_selected_title.clone()}
                                onclick={link.callback(|_: MouseEvent| Msg::AddSelected)}>
                                {"Add selected"}
                            </button>
                            <button type="button" class="bulk-clear sig-bulk-clear"
                                aria-label="Clear signal selection"
                                title="Clear the selected signals"
                                onclick={link.callback(|_: MouseEvent| Msg::ClearSelection)}>
                                {"Clear"}
                            </button>
                            <span class="bulk-hint" title="Click a signal row to select it, Ctrl/Cmd-click to toggle, Shift-click a range">
                                {"Ctrl/Cmd-click to toggle · Shift-click a range"}
                            </span>
                        </div>
                        }
                    } else {
                        html!{}
                    }}
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
            </section>
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

    /// The paths of the filtered list, in display order: what Shift ranges and
    /// "Select all" work on. The list is virtualized, but the logical order is
    /// the order the user sees, exactly like the module tree ranges over its
    /// whole open tree instead of the drawn window.
    fn visible_paths<'a>(&self, signals: &'a [SignalRow]) -> Vec<&'a str> {
        match self.visible_indices(signals) {
            Ok(None) => signals.iter().map(|signal| signal.path.as_str()).collect(),
            Ok(Some(indices)) => indices
                .iter()
                .map(|&index| signals[index].path.as_str())
                .collect(),
            Err(()) => Vec::new(),
        }
    }

    /// The filter (or its toggles) changed: a selection must never keep a path
    /// the list no longer shows (it would be added invisibly).
    fn prune_selection(&mut self, ctx: &Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let visible = self.visible_paths(self.signals(ctx));
        self.selection.prune(&visible);
    }
}
