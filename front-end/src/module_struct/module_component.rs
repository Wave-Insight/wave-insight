use yew::prelude::*;

use crate::nav::SidebarTree;

pub enum Msg {
    Click,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct ModuleComponentProps {
    /// The whole tree; children are looked up by path when the row expands.
    pub tree: SidebarTree,
    /// The scope this row renders.
    pub path: String,
    /// Nesting depth, for the indent.
    pub depth: usize,
    /// Currently selected scope path.
    #[prop_or_default]
    pub selected: String,
    #[prop_or_default]
    pub onselect: Callback<String>,
}

pub struct ModuleComponent {
    show_sub: bool,
}

impl Component for ModuleComponent {
    type Message = Msg;
    type Properties = ModuleComponentProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self { show_sub: false }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Click => {
                //clicking a module row selects it and expands it
                self.show_sub = true;
                ctx.props().onselect.emit(ctx.props().path.clone());
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let props = ctx.props();
        let Some(module) = props.tree.find(&props.path) else {
            return html! {};
        };
        let has_children = !module.children.is_empty();
        let caret = if !has_children {
            ""
        } else if self.show_sub {
            "▾"
        } else {
            "▸"
        };
        let space: String = (0..props.depth * 2).map(|_| " ").collect();
        let chosen = if props.selected == module.path {
            "tree-row chosen"
        } else {
            "tree-row"
        };
        let tree = props.tree.clone();
        let selected = props.selected.clone();
        let onselect = props.onselect.clone();
        html! {
            <div>
                <div class={chosen} onclick={ctx.link().callback(|_| Msg::Click)}>
                    <span class={if self.show_sub {"tree-caret open"} else {"tree-caret"}}>{caret}</span>
                    <span class="tree-name">{space + &module.name}</span>
                </div>
                if self.show_sub {
                    { for module.children.iter().map(|child| {
                        let child_path = child.path.clone();
                        html! {
                            <ModuleComponent
                                tree={tree.clone()}
                                path={child_path}
                                depth={props.depth + 1}
                                selected={selected.clone()}
                                onselect={onselect.clone()} />
                        }
                    }) }
                }
            </div>
        }
    }
}
