//! Choose one signal path when a source position maps to several of them.

use wasm_bindgen::JsCast;
use yew::prelude::*;

use super::{focus_first_in_modal, modal_keydown, ModalKey};

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct PathPickerProps {
    pub title: String,
    /// Candidate hierarchical paths, e.g. `top.u_cpu.pc` and `top.u_cpu16.pc`.
    pub paths: Vec<String>,
    #[prop_or_default]
    pub onchoose: Callback<usize>,
    #[prop_or_default]
    pub oncancel: Callback<()>,
}

pub enum Msg {
    Choose(usize),
    Cancel,
    /// A key pressed inside the dialog: Escape cancels, Tab stays inside.
    KeyDown(KeyboardEvent),
}

pub struct PathPicker {
    /// The element that had focus when the picker opened, so cancelling returns
    /// the keyboard there.
    opener: Option<web_sys::Element>,
}

impl Component for PathPicker {
    type Message = Msg;
    type Properties = PathPickerProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            opener: web_sys::window()
                .and_then(|window| window.document())
                .and_then(|document| document.active_element()),
        }
    }

    fn rendered(&mut self, _ctx: &Context<Self>, first_render: bool) {
        if first_render {
            focus_first_in_modal();
        }
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let Some(opener) = self.opener.take() {
            if let Ok(element) = opener.dyn_into::<web_sys::HtmlElement>() {
                let _ = element.focus();
            }
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Choose(index) => {
                ctx.props().onchoose.emit(index);
                false
            }
            Msg::Cancel => {
                ctx.props().oncancel.emit(());
                false
            }
            Msg::KeyDown(event) => match modal_keydown(&event) {
                ModalKey::Close => {
                    ctx.props().oncancel.emit(());
                    false
                }
                ModalKey::Trapped => true,
                ModalKey::Ignore => false,
            },
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let props = ctx.props();
        let link = ctx.link();

        html! {
            <div>
                <div class="overlay" onclick={link.callback(|_| Msg::Cancel)}></div>
                <div class="modal" role="dialog" aria-modal="true"
                    aria-label={props.title.clone()}
                    onkeydown={link.callback(Msg::KeyDown)}>
                    <h4 class="modal-title">{props.title.clone()}</h4>
                    <div class="modal-section">
                        <p class="modal-label">{format!("{} signals match", props.paths.len())}</p>
                    </div>
                    <div style="max-height:260px;overflow-y:auto;padding:0 8px"
                        role="listbox" aria-label="Signals that match">
                        {
                            for props.paths.iter().enumerate().map(|(index, path)| {
                                html! {
                                    <div class="sig-row" title={path.clone()} role="option"
                                        aria-selected="false" tabindex="0"
                                        onclick={link.callback(move |_| Msg::Choose(index))}
                                        onkeydown={link.callback(move |event: KeyboardEvent| {
                                            if event.key() == "Enter" || event.key() == " " {
                                                event.prevent_default();
                                                Msg::Choose(index)
                                            } else {
                                                Msg::KeyDown(event)
                                            }
                                        })}>
                                        <span class="sig-name">{path.clone()}</span>
                                        <span class="sig-add">{"+"}</span>
                                    </div>
                                }
                            })
                        }
                    </div>
                    <div class="modal-section">
                        <div class="modal-row">
                            <button type="button" class="btn"
                                onclick={link.callback(|_| Msg::Cancel)}>{"Cancel"}</button>
                        </div>
                    </div>
                </div>
            </div>
        }
    }
}
