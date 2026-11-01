//! The sidebar: the module tree and the signal list of the selected scope.
//!
//! Both lists are virtualized for big dumps: a 105 MB dump declares tens of
//! thousands of signals, and drawing one DOM row per signal froze the sidebar.
//! Only the rows inside the scroll viewport (plus a small overscan) are drawn,
//! with a spacer above and below so the scrollbar keeps the real length; the
//! math lives in [`virtual_list`]. Lists up to
//! [`virtual_list::VIRTUALIZE_ABOVE`] rows are drawn whole, exactly as before,
//! so the small dumps the QA matrix drives keep the same DOM.
//!
//! The tree is flattened here (the old recursive component kept its own
//! expansion flag per row) because a window needs one flat list to slice. The
//! expansion behaviour is unchanged: clicking a scope row selects it and opens
//! its children, and a row only expands, never collapses (as before).
//!
//! Because rows can be scrolled out of the DOM, the sidebar publishes the
//! logical lists through [`seam`] (`window.__waveInsightSidebar`) so the QA
//! matrix can read the whole sidebar and still prove the DOM stays bounded.

use std::cell::Cell;
use std::collections::HashSet;

use web_sys::Event;
use yew::prelude::*;

use crate::module_struct::seam;
use crate::module_struct::virtual_list::{self, ROW_H};
use crate::module_struct::SignalStruct;
use crate::nav::SidebarTree;
use crate::state::Focus;

/// One row of the flattened module tree.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FlatRow {
    /// Full dotted path; also the expansion key.
    path: String,
    /// Scope name inside its parent, as the row shows it.
    name: String,
    /// Nesting depth, for the indent.
    depth: usize,
    /// Whether the row draws a caret at all.
    has_children: bool,
    /// Whether its children are shown.
    expanded: bool,
}

pub enum Msg {
    /// A scope row was clicked: select it (and open it, as before).
    Select(String),
    /// A signal row was clicked: its hierarchical path.
    SignalAdd(String),
    /// The modules panel scrolled or was measured: (scroll top, viewport).
    ModuleScroll(f64, f64),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleStructProps {
    /// The tree to show: the opened dump's scopes or the elaborated design's
    /// instances. None until one of them exists.
    #[prop_or_default]
    pub tree: Option<SidebarTree>,
    /// A signal was added: its display name and its hierarchical path.
    #[prop_or_default]
    pub signaladd: Callback<(String, String)>,
    /// The store's focus slice: `instance` is the selected scope (R2 step 3b;
    /// the expanded set stays component-local).
    #[prop_or_default]
    pub focus: Focus,
    /// A scope row was clicked: the store owns the selection.
    #[prop_or_default]
    pub oninstance: Callback<String>,
}

pub struct ModuleStruct {
    /// The tree the selection belongs to, so a new tree resets it.
    tree: Option<SidebarTree>,
    /// The last module selection seen in the props: a focus change re-renders
    /// the sidebar even when the tree did not move.
    instance: Option<String>,
    /// Scope paths whose children are open (the old per-row show_sub flag).
    expanded: HashSet<String>,
    /// The flattened rows of the open tree. Rebuilt when the tree or the
    /// expansion changes, never per render: a 20k-scope tree is walked once per
    /// click, not once per scroll frame.
    rows: Vec<FlatRow>,
    /// Scopes in the whole tree (seam only; the DOM shows the open part).
    scope_count: usize,
    module_scroll: f64,
    module_viewport: f64,
    /// The window the last render drew, published to the seam from rendered().
    module_window: Cell<virtual_list::Window>,
    /// The logical module list changed: rewrite the seam array after the render.
    modules_dirty: bool,
    /// The modules panel, the tree's scroll container.
    module_ref: NodeRef,
}

impl Component for ModuleStruct {
    type Message = Msg;
    type Properties = ModuleStructProps;

    fn create(ctx: &Context<Self>) -> Self {
        let mut this = Self {
            tree: None,
            instance: None,
            expanded: HashSet::new(),
            rows: Vec::new(),
            scope_count: 0,
            module_scroll: 0.0,
            module_viewport: 0.0,
            module_window: Cell::new(virtual_list::Window::EMPTY),
            modules_dirty: true,
            module_ref: NodeRef::default(),
        };
        this.sync_tree(ctx);
        this
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        // Props only carry the tree (and a callback); a parent re-render with
        // the same tree must not re-render 20k rows.
        if self.tree.as_ref() != ctx.props().tree.as_ref() {
            self.sync_tree(ctx);
            self.instance = ctx.props().focus.instance.clone();
            return true;
        }
        // the store owns the module selection: a click there must re-render the
        // signal list even though the tree is the same
        if self.instance != ctx.props().focus.instance {
            self.instance = ctx.props().focus.instance.clone();
            return true;
        }
        false
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        // the sidebar is gone (drawer closed): do not leave a stale list behind
        seam::clear();
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        let window = self.module_window.get();
        if self.modules_dirty {
            // the seam carries the whole open tree, not just the window
            seam::publish_modules(
                self.rows.iter().map(|row| row.name.as_str()),
                self.scope_count,
                window.len(),
                window.windowed,
            );
            self.modules_dirty = false;
        } else {
            seam::publish_module_window(window.len(), window.windowed);
        }
        if self.tree.is_none() {
            // no tree means no signal list either; the child is not mounted
            seam::publish_signals(std::iter::empty(), 0, 0, false);
        }
        // The first paint and every expansion change the scroll container's
        // height; the window is computed from the measured value.
        if let Some(element) = self.module_ref.cast::<web_sys::Element>() {
            let height = element.client_height() as f64;
            let top = element.scroll_top() as f64;
            if (height - self.module_viewport).abs() > 0.5 || (top - self.module_scroll).abs() > 0.5
            {
                ctx.link().send_message(Msg::ModuleScroll(top, height));
            }
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Select(path) => {
                // the store owns the selection; the component keeps only the
                // expansion set (a presentation detail)
                ctx.props().oninstance.emit(path.clone());
                // clicking a scope opens it (the old row did the same)
                let opens = self
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.find(&path))
                    .map(|scope| !scope.children.is_empty())
                    .unwrap_or(false);
                if opens && self.expanded.insert(path) {
                    self.rebuild();
                }
                true
            }
            Msg::SignalAdd(path) => {
                let selected = self.selected(ctx);
                let display = self
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.find(&selected))
                    .and_then(|scope| scope.signals.iter().find(|signal| signal.path == path))
                    .map(|signal| signal.display_name())
                    .unwrap_or_else(|| path.rsplit('.').next().unwrap_or(&path).to_string());
                ctx.props().signaladd.emit((display, path));
                true
            }
            Msg::ModuleScroll(top, height) => {
                if (height - self.module_viewport).abs() <= 0.5
                    && (top - self.module_scroll).abs() <= 0.5
                {
                    return false;
                }
                self.module_scroll = top;
                self.module_viewport = height;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let Some(tree) = ctx.props().tree.as_ref() else {
            return html! {
                <div class="side-stack">
                    <section class="panel">
                        <div class="panel-head">{"Modules"}</div>
                        <div class="panel-body">
                            <div class="panel-note">{"Load sources or a dump to see modules."}</div>
                        </div>
                    </section>
                    <section class="panel panel-grow">
                        <div class="panel-head">{"Signals"}</div>
                        <div class="panel-body">
                            <div class="panel-note">{"No signals yet."}</div>
                        </div>
                    </section>
                </div>
            };
        };
        let selected = self.selected(ctx);
        let signal_count = tree
            .find(&selected)
            .map(|scope| scope.signals.len())
            .unwrap_or(0);
        let window = virtual_list::window(
            self.rows.len(),
            self.module_viewport,
            self.module_scroll,
            ROW_H,
        );
        self.module_window.set(window);
        let onselect = link.callback(Msg::Select);
        let onscroll = {
            let module_ref = self.module_ref.clone();
            link.callback(move |_: Event| {
                let (top, height) = module_ref
                    .cast::<web_sys::Element>()
                    .map(|element| (element.scroll_top() as f64, element.client_height() as f64))
                    .unwrap_or((0.0, 0.0));
                Msg::ModuleScroll(top, height)
            })
        };
        let rows = &self.rows[window.first..window.last];
        html! {
            <div class="side-stack">
                <section class="panel">
                    <div class="panel-head">{"Modules"}</div>
                    <div class="panel-body" ref={self.module_ref.clone()} onscroll={onscroll}>
                        {
                            if window.top > 0.0 {
                                html! {
                                    <div class="tree-spacer"
                                        style={format!("height:{:.0}px", window.top)}></div>
                                }
                            } else {
                                html! {}
                            }
                        }
                        {
                            for rows.iter().map(|row| {
                                let chosen = if row.path == selected {
                                    "tree-row chosen"
                                } else {
                                    "tree-row"
                                };
                                let caret = if !row.has_children {
                                    ""
                                } else if row.expanded {
                                    "▾"
                                } else {
                                    "▸"
                                };
                                let caret_class = if row.expanded {
                                    "tree-caret open"
                                } else {
                                    "tree-caret"
                                };
                                let space: String = (0..row.depth * 2).map(|_| " ").collect();
                                let path = row.path.clone();
                                let title = row.path.clone();
                                html! {
                                    <div class={chosen} title={title}
                                        onclick={onselect.clone().reform(move |_: MouseEvent| path.clone())}>
                                        <span class={caret_class}>{caret}</span>
                                        <span class="tree-name">{space + &row.name}</span>
                                    </div>
                                }
                            })
                        }
                        {
                            if window.bottom > 0.0 {
                                html! {
                                    <div class="tree-spacer"
                                        style={format!("height:{:.0}px", window.bottom)}></div>
                                }
                            } else {
                                html! {}
                            }
                        }
                    </div>
                </section>
                <section class="panel panel-grow">
                    <div class="panel-head">
                        {"Signals"}
                        <span class="count">{signal_count.to_string()}</span>
                    </div>
                    <SignalStruct
                        tree={tree.clone()}
                        selected={selected}
                        signaladd={link.callback(Msg::SignalAdd)} />
                </section>
            </div>
        }
    }
}

impl ModuleStruct {
    /// The scope whose signals are listed: the store's module selection, or the
    /// tree's initial path when nothing is selected (a fresh tree) or the
    /// selected scope left the tree.
    fn selected(&self, ctx: &Context<Self>) -> String {
        let focus = ctx.props().focus.instance.clone().unwrap_or_default();
        // an empty selection means "the tree's first scope"; note that
        // `find("")` can resolve to the synthetic root of a dump hierarchy,
        // whose signal list is empty
        let known = !focus.is_empty()
            && self
                .tree
                .as_ref()
                .is_some_and(|tree| tree.find(&focus).is_some());
        if known {
            focus
        } else {
            self.tree
                .as_ref()
                .map(|tree| tree.initial_path())
                .unwrap_or_default()
        }
    }

    /// Adopt the tree from the props: a new tree resets every expansion, exactly
    /// like the old component being rebuilt did (the selection lives in the
    /// store and is cleared by the reducer when the tree changes).
    fn sync_tree(&mut self, ctx: &Context<Self>) {
        self.tree = ctx.props().tree.clone();
        self.expanded.clear();
        self.module_scroll = 0.0;
        self.module_viewport = 0.0;
        self.rebuild();
    }

    /// Flatten the open tree into the rows a render slices.
    fn rebuild(&mut self) {
        self.rows = match &self.tree {
            Some(tree) => tree
                .visible_rows(&self.expanded)
                .into_iter()
                .map(|(path, name, depth, has_children, expanded)| FlatRow {
                    path,
                    name,
                    depth,
                    has_children,
                    expanded,
                })
                .collect(),
            None => Vec::new(),
        };
        self.scope_count = self
            .tree
            .as_ref()
            .map(|tree| tree.root().count_scopes())
            .unwrap_or(0);
        self.modules_dirty = true;
    }
}
