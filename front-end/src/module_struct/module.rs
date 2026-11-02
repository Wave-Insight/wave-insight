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

use wasm_bindgen::JsCast;
use web_sys::Event;
use yew::prelude::*;

use crate::module_struct::seam;
use crate::module_struct::virtual_list::{self, ROW_H};
use crate::module_struct::SignalStruct;
use crate::nav::SidebarTree;
use crate::state::{bulk, Focus};

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

/// Which kind of sidebar row a context menu was opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    /// A module/instance row of the tree.
    Module,
    /// A signal row of the selected scope.
    Signal,
}

/// A context-menu gesture on a sidebar row (right click, or the context-menu
/// key / Shift+F10 on the keyboard).
#[derive(Clone, Debug, PartialEq)]
pub struct RowContext {
    pub kind: RowKind,
    /// The row's full path.
    pub path: String,
    /// Signals "add all in scope" would add (module rows; 0 for signals).
    pub scope_signals: usize,
    /// The visible module rows at the time of the gesture (for "select all
    /// visible"); empty for signal rows.
    pub visible: Vec<String>,
    /// Viewport coordinates the menu opens at.
    pub x: i32,
    pub y: i32,
}

/// What a module-tree click means for the bulk selection.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeSelect {
    /// Plain click: the selection becomes this row (and it becomes the anchor).
    Reset(String),
    /// Ctrl/Cmd click: toggle this row in the selection.
    Toggle(String),
    /// Shift click: the visible range from the anchor to this row.
    Range(Vec<String>),
}

pub enum Msg {
    /// A scope row was clicked; `ctrl`/`shift` are the modifier state.
    Select {
        path: String,
        ctrl: bool,
        shift: bool,
    },
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
    /// A scope row was clicked: the store owns the single selection.
    #[prop_or_default]
    pub oninstance: Callback<String>,
    /// The module-tree multi-selection (bulk add): selected scope paths.
    #[prop_or_default]
    pub selection: Vec<String>,
    /// The Shift+click anchor (the last plain/ctrl click).
    #[prop_or_default]
    pub anchor: Option<String>,
    /// A row was clicked with modifiers: the store owns the multi-selection.
    #[prop_or_default]
    pub onselection: Callback<TreeSelect>,
    /// "Add all signals in this scope": the scope path.
    #[prop_or_default]
    pub onaddscope: Callback<String>,
    /// "Add selected (N)": the app adds the selection's signals in one batch.
    #[prop_or_default]
    pub onaddselected: Callback<()>,
    /// The selection bar's "Clear" button.
    #[prop_or_default]
    pub onclear: Callback<()>,
    /// A row asked for its context menu (right click / keyboard).
    #[prop_or_default]
    pub onrowcontext: Callback<RowContext>,
}

pub struct ModuleStruct {
    /// The tree the selection belongs to, so a new tree resets it.
    tree: Option<SidebarTree>,
    /// The last module selection seen in the props: a focus change re-renders
    /// the sidebar even when the tree did not move.
    instance: Option<String>,
    /// The multi-selection last seen in the props (bulk-add highlight).
    selection: Vec<String>,
    /// The Shift anchor last seen in the props.
    anchor: Option<String>,
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
            selection: Vec::new(),
            anchor: None,
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

    fn changed(&mut self, ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
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
        // the multi-selection highlight lives in the props (store state)
        if self.selection != ctx.props().selection || self.anchor != ctx.props().anchor {
            self.selection = ctx.props().selection.clone();
            self.anchor = ctx.props().anchor.clone();
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
            Msg::Select { path, ctrl, shift } => {
                if ctrl {
                    // Ctrl/Cmd+click toggles the row in the bulk selection; the
                    // single (inspector) selection does not move
                    ctx.props().onselection.emit(TreeSelect::Toggle(path));
                    return true;
                }
                if shift {
                    // Shift+click selects the visible range from the anchor
                    let visible: Vec<String> =
                        self.rows.iter().map(|row| row.path.clone()).collect();
                    let paths = bulk::range_between(&visible, ctx.props().anchor.as_deref(), &path);
                    ctx.props().onselection.emit(TreeSelect::Range(paths));
                    return true;
                }
                // the store owns the selection; the component keeps only the
                // expansion set (a presentation detail)
                ctx.props().oninstance.emit(path.clone());
                ctx.props()
                    .onselection
                    .emit(TreeSelect::Reset(path.clone()));
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
        let selection: std::collections::HashSet<&str> =
            ctx.props().selection.iter().map(String::as_str).collect();
        // the button counts what would be added, deduped across the scopes
        let selected_count = bulk::signals_under_many(tree, &ctx.props().selection).len();
        let link = link.clone();
        let onaddselected = {
            let callback = ctx.props().onaddselected.clone();
            Callback::from(move |_: MouseEvent| callback.emit(()))
        };
        let onclear = {
            let callback = ctx.props().onclear.clone();
            Callback::from(move |_: MouseEvent| callback.emit(()))
        };
        let oncontext = ctx.props().onrowcontext.clone();
        let visible_paths = self.visible_paths();
        let select_all = {
            let callback = ctx.props().onselection.clone();
            let paths = visible_paths.clone();
            Callback::from(move |_: MouseEvent| {
                callback.emit(TreeSelect::Range(paths.clone()));
            })
        };
        let over_cap = selected_count > bulk::BULK_ADD_CAP;
        let add_selected_title = if over_cap {
            format!(
                "Too many signals ({selected_count}) — the limit is {}; select fewer scopes",
                bulk::BULK_ADD_CAP
            )
        } else {
            format!("Add {selected_count} signals from the selected scopes")
        };
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
                    <div class="panel-head">
                        {"Modules"}
                        <button type="button" class="tree-select-all"
                            aria-label="Select all visible modules"
                            title="Select all visible modules for bulk add"
                            onclick={select_all}>
                            {"Select all"}
                        </button>
                        <button type="button"
                            class={if over_cap {"tree-add-selected over-cap"} else {"tree-add-selected"}}
                            aria-label="Add selected signals"
                            aria-disabled={if over_cap {"true"} else {"false"}}
                            title={add_selected_title.clone()}
                            disabled={ctx.props().selection.is_empty()}
                            onclick={onaddselected.clone()}>
                            {format!("Add selected ({selected_count})")}
                        </button>
                    </div>
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
                                let is_selected = selection.contains(row.path.as_str());
                                let chosen = match (row.path == selected, is_selected) {
                                    (true, true) => "tree-row chosen selected",
                                    (true, false) => "tree-row chosen",
                                    (false, true) => "tree-row selected",
                                    (false, false) => "tree-row",
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
                                let add_path = row.path.clone();
                                let add_label = format!("Add all signals in {}", row.path);
                                let add_callback = ctx.props().onaddscope.clone();
                                let click_link = link.clone();
                                let context_path = row.path.clone();
                                let context_signals = bulk::signals_under(tree, &row.path).len();
                                let context_visible = visible_paths.clone();
                                let context_callback = oncontext.clone();
                                let context_menu = Callback::from(move |event: MouseEvent| {
                                    event.prevent_default();
                                    event.stop_propagation();
                                    context_callback.emit(RowContext {
                                        kind: RowKind::Module,
                                        path: context_path.clone(),
                                        scope_signals: context_signals,
                                        visible: context_visible.clone(),
                                        x: event.client_x(),
                                        y: event.client_y(),
                                    });
                                });
                                let keyboard_path = row.path.clone();
                                let keyboard_signals = bulk::signals_under(tree, &row.path).len();
                                let keyboard_visible = visible_paths.clone();
                                let keyboard_callback = oncontext.clone();
                                let keyboard_menu = Callback::from(move |event: KeyboardEvent| {
                                    let opens = event.key() == "ContextMenu"
                                        || (event.shift_key() && event.key() == "F10");
                                    if !opens {
                                        return;
                                    }
                                    event.prevent_default();
                                    let (left, top, height) = event
                                        .current_target()
                                        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                                        .map(|element| {
                                            let rect = element.get_bounding_client_rect();
                                            (rect.left(), rect.top(), rect.height())
                                        })
                                        .unwrap_or((0.0, 0.0, 0.0));
                                    keyboard_callback.emit(RowContext {
                                        kind: RowKind::Module,
                                        path: keyboard_path.clone(),
                                        scope_signals: keyboard_signals,
                                        visible: keyboard_visible.clone(),
                                        x: (left + 24.0) as i32,
                                        y: (top + height) as i32,
                                    });
                                });
                                html! {
                                    <div class={chosen} title={title}
                                        tabindex="0"
                                        aria-selected={if is_selected {"true"} else {"false"}}
                                        oncontextmenu={context_menu}
                                        onkeydown={keyboard_menu}
                                        onclick={click_link.callback(move |event: MouseEvent| Msg::Select {
                                            path: path.clone(),
                                            ctrl: event.ctrl_key() || event.meta_key(),
                                            shift: event.shift_key(),
                                        })}>
                                        <span class={caret_class}>{caret}</span>
                                        <span class="tree-name">{space + &row.name}</span>
                                        <button type="button" class="tree-add"
                                            aria-label={add_label.clone()}
                                            title={add_label}
                                            onclick={Callback::from(move |event: MouseEvent| {
                                                event.stop_propagation();
                                                add_callback.emit(add_path.clone());
                                            })}></button>
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
                    // the sticky bulk bar is a footer on purpose: it must not
                    // push the tree rows (a bar above the list moved every row
                    // when the first selection appeared, which broke the
                    // click-the-row-you-see behaviour)
                    {if !ctx.props().selection.is_empty() {
                        html!{
                        <div class="bulk-bar" role="status" aria-label="Bulk selection">
                            <span class="bulk-count">{format!("{selected_count} signals selected")}</span>
                            <button type="button" class="bulk-add"
                                aria-label="Add selected signals"
                                aria-disabled={if over_cap {"true"} else {"false"}}
                                title={add_selected_title.clone()}
                                onclick={onaddselected.clone()}>
                                {"Add selected"}
                            </button>
                            <button type="button" class="bulk-clear"
                                aria-label="Clear bulk selection"
                                title="Clear the bulk selection"
                                onclick={onclear.clone()}>
                                {"Clear"}
                            </button>
                            <span class="bulk-hint" title="Click a module row to select it, Ctrl/Cmd-click to toggle, Shift-click a range">
                                {"Ctrl/Cmd-click to toggle · Shift-click a range"}
                            </span>
                        </div>
                        }
                    } else {
                        html!{}
                    }}
                </section>
                <section class="panel panel-grow">
                    <div class="panel-head">
                        {"Signals"}
                        <span class="count">{signal_count.to_string()}</span>
                    </div>
                    <SignalStruct
                        tree={tree.clone()}
                        selected={selected}
                        signaladd={link.callback(Msg::SignalAdd)}
                        onrowcontext={ctx.props().onrowcontext.clone()} />
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

    /// The visible module rows, in display order (for "select all visible").
    fn visible_paths(&self) -> Vec<String> {
        self.rows.iter().map(|row| row.path.clone()).collect()
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
