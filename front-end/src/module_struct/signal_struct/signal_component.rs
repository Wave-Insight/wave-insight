//! One row of the signal list.
//!
//! Wave 19: a row click *selects* — plain click resets the selection to the row,
//! Ctrl/Cmd-click toggles it, Shift-click ranges from the anchor — exactly like
//! the module tree's rows. Adding is explicit: the row's "+" affordance, a
//! double click, Enter on the focused row, or the list's "Add selected (N)"
//! button for the whole selection. The context menu (right click, the
//! context-menu key, Shift+F10) is unchanged.

use wasm_bindgen::JsCast;
use yew::prelude::*;

use super::select::SignalSelect;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalComponentProps {
    /// Name as shown in the list, with the index range for buses.
    pub name: String,
    /// Bit width, shown on the right.
    pub width: usize,
    /// Hierarchical path; the row key the wave view gets.
    pub path: String,
    /// The row is part of the list's multi-selection.
    #[prop_or_default]
    pub selected: bool,
    /// The row was clicked: plain / Ctrl / Shift selection intent.
    #[prop_or_default]
    pub onselect: Callback<SignalSelect>,
    /// The explicit add gestures of this row: the "+" button and a double click.
    #[prop_or_default]
    pub onadd: Callback<String>,
    /// Enter on the focused row: the list adds its selection when it has one and
    /// this row otherwise (the signal list resolves which).
    #[prop_or_default]
    pub onenter: Callback<String>,
    /// The row asked for its context menu (right click / context-menu key).
    #[prop_or_default]
    pub onrowcontext: Callback<crate::module_struct::RowContext>,
}

pub struct SignalComponent {}

impl Component for SignalComponent {
    type Message = ();
    type Properties = SignalComponentProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {}
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        // every gesture goes straight to the parent's callbacks; this component
        // only exists so a row re-renders when its own props change
        false
    }

    fn changed(&mut self, _ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let props = ctx.props();
        let selected = props.selected;

        // a row click carries the selection intent; the list owns the state
        let select_path = props.path.clone();
        let onselect = props.onselect.clone();
        let onclick = Callback::from(move |event: MouseEvent| {
            onselect.emit(SignalSelect::from_click(
                &select_path,
                event.ctrl_key() || event.meta_key(),
                event.shift_key(),
            ));
        });

        // double click adds this row immediately (the selection may also have
        // moved to it: the two clicks of the double click selected it first)
        let dblclick_path = props.path.clone();
        let onadd = props.onadd.clone();
        let ondblclick = Callback::from(move |_: MouseEvent| onadd.emit(dblclick_path.clone()));

        // the "+" is a real button: a labelled, stable add target that does not
        // reselect the row (it stops the click before the row's handler)
        let add_path = props.path.clone();
        let add_callback = props.onadd.clone();
        let add_label = format!("Add {} to the wave", props.name);
        let add_click = Callback::from(move |event: MouseEvent| {
            event.stop_propagation();
            add_callback.emit(add_path.clone());
        });
        let add_dblclick = Callback::from(|event: MouseEvent| event.stop_propagation());

        // right click / context-menu key / Shift+F10: the shared row menu
        let context_path = props.path.clone();
        let context_callback = props.onrowcontext.clone();
        let context_menu = Callback::from(move |event: MouseEvent| {
            event.prevent_default();
            event.stop_propagation();
            context_callback.emit(crate::module_struct::RowContext {
                kind: crate::module_struct::RowKind::Signal,
                path: context_path.clone(),
                scope_signals: 0,
                visible: Vec::new(),
                x: event.client_x(),
                y: event.client_y(),
            });
        });
        let enter_path = props.path.clone();
        let onenter = props.onenter.clone();
        let keyboard_path = props.path.clone();
        let keyboard_callback = props.onrowcontext.clone();
        let keyboard_menu = Callback::from(move |event: KeyboardEvent| {
            if event.key() == "Enter" {
                // explicit add from the keyboard: the list adds its selection
                // when it has one, otherwise just this row
                event.prevent_default();
                onenter.emit(enter_path.clone());
                return;
            }
            let opens = event.key() == "ContextMenu" || (event.shift_key() && event.key() == "F10");
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
            keyboard_callback.emit(crate::module_struct::RowContext {
                kind: crate::module_struct::RowKind::Signal,
                path: keyboard_path.clone(),
                scope_signals: 0,
                visible: Vec::new(),
                x: (left + 24.0) as i32,
                y: (top + height) as i32,
            });
        });

        let class = if selected {
            "sig-row selected"
        } else {
            "sig-row"
        };
        html! {
            <div class={class}
                onclick={onclick}
                ondblclick={ondblclick}
                tabindex="0"
                aria-selected={if selected {"true"} else {"false"}}
                oncontextmenu={context_menu}
                onkeydown={keyboard_menu}
                title={props.path.clone()}>
                <span class="sig-name">{&props.name}</span>
                <span class="sig-size">{props.width.to_string()}</span>
                <button type="button" class="sig-add"
                    tabindex="-1"
                    aria-label={add_label.clone()}
                    title={add_label}
                    onclick={add_click}
                    ondblclick={add_dblclick}>{"+"}</button>
            </div>
        }
    }
}
