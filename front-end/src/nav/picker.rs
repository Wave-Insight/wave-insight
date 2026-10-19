//! Choose one signal path when a source position maps to several of them.

use yew::prelude::*;

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
}

pub struct PathPicker;

impl Component for PathPicker {
    type Message = Msg;
    type Properties = PathPickerProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self
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
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let props = ctx.props();
        let link = ctx.link();

        html! {
            <div>
                <div class="overlay" onclick={link.callback(|_| Msg::Cancel)}></div>
                <div class="modal">
                    <h4 class="modal-title">{props.title.clone()}</h4>
                    <div class="modal-section">
                        <p class="modal-label">{format!("{} signals match", props.paths.len())}</p>
                    </div>
                    <div style="max-height:260px;overflow-y:auto;padding:0 8px">
                        {
                            for props.paths.iter().enumerate().map(|(index, path)| {
                                html! {
                                    <div class="sig-row" title={path.clone()}
                                        onclick={link.callback(move |_| Msg::Choose(index))}>
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
