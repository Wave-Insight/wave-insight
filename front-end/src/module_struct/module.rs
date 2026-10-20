use yew::prelude::*;

use crate::module_struct::{ModuleComponent, SignalStruct};
use crate::nav::SidebarTree;

pub enum Msg {
    /// A scope row was clicked.
    Select(String),
    /// A signal row was clicked: its hierarchical path.
    SignalAdd(String),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleStructProps {
    /// The tree to show: the opened dump's scopes or the elaborated design's
    /// instances. `None` until one of them exists.
    #[prop_or_default]
    pub tree: Option<SidebarTree>,
    /// A signal was added: its display name and its hierarchical path.
    #[prop_or_default]
    pub signaladd: Callback<(String, String)>,
}

pub struct ModuleStruct {
    /// Full path of the scope whose signals are listed.
    selected: String,
    /// The tree the selection belongs to, so a new tree resets it.
    tree: Option<SidebarTree>,
}

impl Component for ModuleStruct {
    type Message = Msg;
    type Properties = ModuleStructProps;

    fn create(ctx: &Context<Self>) -> Self {
        let tree = ctx.props().tree.clone();
        let selected = tree
            .as_ref()
            .map(|tree| tree.initial_path())
            .unwrap_or_default();
        Self { selected, tree }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        if self.tree != ctx.props().tree {
            self.tree = ctx.props().tree.clone();
            self.selected = self
                .tree
                .as_ref()
                .map(|tree| tree.initial_path())
                .unwrap_or_default();
        }
        true
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Select(path) => {
                self.selected = path;
                true
            }
            Msg::SignalAdd(path) => {
                let display = ctx
                    .props()
                    .tree
                    .as_ref()
                    .and_then(|tree| tree.find(&self.selected))
                    .and_then(|scope| scope.signals.iter().find(|signal| signal.path == path))
                    .map(|signal| signal.display_name())
                    .unwrap_or_else(|| path.rsplit('.').next().unwrap_or(&path).to_string());
                ctx.props().signaladd.emit((display, path));
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
        let root = tree.root();
        let selected = self.selected.clone();
        let signals = tree
            .find(&self.selected)
            .map(|scope| scope.signals.clone())
            .unwrap_or_default();
        let count = signals.len();
        let onselect = link.callback(Msg::Select);
        let signaladd = link.callback(Msg::SignalAdd);
        html! {
            <div class="side-stack">
                <section class="panel">
                    <div class="panel-head">{"Modules"}</div>
                    <div class="panel-body">
                        {
                            for root.children.iter().map(|child| {
                                let path = child.path.clone();
                                html! {
                                    <ModuleComponent
                                        tree={tree.clone()}
                                        path={path}
                                        depth={0}
                                        selected={selected.clone()}
                                        onselect={onselect.clone()} />
                                }
                            })
                        }
                    </div>
                </section>
                <section class="panel panel-grow">
                    <div class="panel-head">
                        {"Signals"}
                        <span class="count">{count.to_string()}</span>
                    </div>
                    <SignalStruct {signals} {signaladd} />
                </section>
            </div>
        }
    }
}
