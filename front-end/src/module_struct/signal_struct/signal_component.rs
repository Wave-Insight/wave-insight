use wasm_bindgen::JsCast;
use yew::prelude::*;

pub enum Msg {
    Click,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalComponentProps {
    /// Name as shown in the list, with the index range for buses.
    pub name: String,
    /// Bit width, shown on the right.
    pub width: usize,
    /// Hierarchical path; the row key the wave view gets.
    pub path: String,
    #[prop_or_default]
    pub onclick: Callback<String>,
    /// The row asked for its context menu (right click / context-menu key).
    #[prop_or_default]
    pub onrowcontext: Callback<crate::module_struct::RowContext>,
}

pub struct SignalComponent {}

impl Component for SignalComponent {
    type Message = Msg;
    type Properties = SignalComponentProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {}
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::Click => {
                props.onclick.emit(props.path.clone());
                true
            }
        }
    }

    fn changed(&mut self, _ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let context_path = ctx.props().path.clone();
        let context_callback = ctx.props().onrowcontext.clone();
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
        let keyboard_path = ctx.props().path.clone();
        let keyboard_callback = ctx.props().onrowcontext.clone();
        let keyboard_menu = Callback::from(move |event: KeyboardEvent| {
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
        html! {
            <div class="sig-row" onclick={link.callback(|_| Msg::Click)}
                tabindex="0"
                oncontextmenu={context_menu}
                onkeydown={keyboard_menu}
                title={ctx.props().path.clone()}>
                <span class="sig-name">{&ctx.props().name}</span>
                <span class="sig-size">{ctx.props().width.to_string()}</span>
                <span class="sig-add">{"+"}</span>
            </div>
        }
    }
}
