use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlInputElement;
use yew::prelude::*;

use wave_insight_lib::data_struct::ShowType;

use super::settings::Settings;

///signal colours, tuned to stay readable on the dark wave canvas
const PALETTE: [(u8,u8,u8);7] = [
    (255,107,138),//rose
    (255,178,92), //amber
    (255,224,102),//yellow
    (94,230,168), //mint
    (76,194,255), //cyan
    (157,140,255),//violet
    (255,138,209),//pink
];

fn btn_class(on: bool) -> &'static str {
    if on {"btn active"} else {"btn"}
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CtrlProps {
    pub name: String,
    /// Hierarchical path of the row, for "copy path" (the name is the short one).
    #[prop_or_default]
    pub path: String,
    pub setting: Settings,
    pub load: Vec<String>,
    pub drive: Vec<String>,
    #[prop_or_default]
    pub onset: Callback<(bool,Settings)>,//<close ctrl menu,new Settings>
    /// The load/drive buttons: `(load?, index)`. The app resolves the span and
    /// lists the several matches in the inspector.
    #[prop_or_default]
    pub onlocate: Callback<(bool, usize)>,
    pub delete: Callback<()>,
}

/// Copy text with the async clipboard API.
///
/// Called from a click, which the browser counts as the user gesture the API
/// requires. `false` when the API is missing or refused (an iframe without the
/// permission, an old browser), so the button can say so instead of lying.
fn copy_to_clipboard(text: &str) -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let read = |target: &JsValue, key: &str| js_sys::Reflect::get(target, &JsValue::from_str(key));
    let Ok(navigator) = read(&window, "navigator") else {
        return false;
    };
    let Ok(clipboard) = read(&navigator, "clipboard") else {
        return false;
    };
    let Ok(write) = read(&clipboard, "writeText") else {
        return false;
    };
    let Ok(write) = write.dyn_into::<js_sys::Function>() else {
        return false;
    };
    let args = js_sys::Array::new();
    args.push(&JsValue::from_str(text));
    write.apply(&clipboard, &args).is_ok()
}

pub enum Msg {
    ExitMenu(MouseEvent),
    ChooseShowType(ShowType),
    ChooseColor((u8,u8,u8)),
    ActiveFixed,
    SetFixed(u32),
    ActiveAnalog,
    FlipLoadShow,
    FlipDriveShow,
    AddAllLoad,
    AddAllDrive,
    DeleteSig,
    /// Put the row's full hierarchical path on the clipboard.
    CopyPath,
}

pub struct Ctrl {
    show_load: bool,
    show_drive: bool,
    /// `Some(true)` after a successful copy, `Some(false)` when the browser
    /// refused; `None` before the button was pressed.
    copied: Option<bool>,
}

impl Component for Ctrl {
    type Message = Msg;
    type Properties = CtrlProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            show_load: false,
            show_drive: false,
            copied: None,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::CopyPath => {
                self.copied = Some(copy_to_clipboard(&props.path));
                true
            }
            Msg::ExitMenu(_e) => {
                props.onset.emit((true,props.setting.clone()));
                true
            }
            Msg::ChooseShowType(show_type) => {
                let mut set = props.setting.clone();
                set.show_type = show_type;
                props.onset.emit((true,set));
                true
            }
            Msg::ChooseColor((r,g,b)) => {
                let mut set = props.setting.clone();
                set.color = (r,g,b);
                props.onset.emit((true,set));
                true
            }
            Msg::ActiveFixed => {
                let mut set = props.setting.clone();
                set.fixed_active = !set.fixed_active;
                props.onset.emit((true,set));
                true
            }
            Msg::SetFixed(exp) => {
                let mut set = props.setting.clone();
                set.fixed_point = exp;
                props.onset.emit((false,set));
                true
            }
            Msg::ActiveAnalog => {
                let mut set = props.setting.clone();
                set.analog_active = !set.analog_active;
                props.onset.emit((true,set));
                true
            }
            Msg::FlipLoadShow => {
                self.show_load = !self.show_load;
                true
            }
            Msg::FlipDriveShow => {
                self.show_drive = !self.show_drive;
                true
            }
            Msg::AddAllDrive => {
                // Verdi's "drive" button: show the first driver, and let the app
                // list the others in the Drivers / loads panel
                props.onlocate.emit((false, 0));
                true
            }
            Msg::AddAllLoad => {
                // Verdi's "load" button: same for the first load
                props.onlocate.emit((true, 0));
                true
            }
            Msg::DeleteSig => {
                props.delete.emit(());
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let props = ctx.props();

        html! {
            <div>
                <div class="overlay" onclick={link.callback(Msg::ExitMenu)}></div>
                <div class="modal">
                    <h4 class="modal-title">{props.name.clone()}</h4>

                    <div class="modal-section">
                        <p class="modal-label">{"Format"}</p>
                        <div class="modal-row">
                            <button type="button" class={btn_class(props.setting.show_type==ShowType::Hex)} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Hex))}>{"Hex"}</button>
                            <button type="button" class={btn_class(props.setting.show_type==ShowType::UInt)} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::UInt))}>{"Dec"}</button>
                            <button type="button" class={btn_class(props.setting.show_type==ShowType::SInt)} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::SInt))}>{"Signed"}</button>
                            <button type="button" class={btn_class(props.setting.show_type==ShowType::Oct)} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Oct))}>{"Oct"}</button>
                            <button type="button" class={btn_class(props.setting.show_type==ShowType::Bin)} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Bin))}>{"Bin"}</button>
                            <button type="button" class={btn_class(props.setting.show_type==ShowType::Ascii)} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Ascii))}>{"ASCII"}</button>
                        </div>
                    </div>

                    <div class="modal-section">
                        <p class="modal-label">{"Fixed point"}</p>
                        <div class="modal-row">
                            <button type="button" class={btn_class(props.setting.fixed_active)} onclick={link.callback(|_| Msg::ActiveFixed)}>{"active"}</button>
                            <input class="modal-input" type="number" onchange={link.callback(|e: Event| Msg::SetFixed(e.target_unchecked_into::<HtmlInputElement>().value_as_number() as u32))} />
                        </div>
                    </div>

                    <div class="modal-section">
                        <p class="modal-label">{"Analog"}</p>
                        <div class="modal-row">
                            <button type="button" class={btn_class(props.setting.analog_active)} onclick={link.callback(|_| Msg::ActiveAnalog)}>{"active"}</button>
                            // the trace is a step plot (the value holds until the
                            // next change), so there is no interpolation to set
                        </div>
                    </div>

                    <div class="modal-section">
                        <p class="modal-label">{"Load / drive"}</p>
                        <div class="modal-row">
                            <button type="button" class="btn"
                                onclick={link.callback(|_| Msg::AddAllLoad)}
                                onmouseover={link.callback(|_| Msg::FlipLoadShow)}
                                onmouseout={link.callback(|_| Msg::FlipLoadShow)}>{"load"}</button>
                            { if self.show_load {html!{
                                <span class="modal-pop">
                                    {for props.load.iter().map(|l| {
                                        html!{<p>{l}</p>}
                                    })}
                                </span>
                            }}else {
                                html!{}
                            }}
                            <button type="button" class="btn"
                                onclick={link.callback(|_| Msg::AddAllDrive)}
                                onmouseover={link.callback(|_| Msg::FlipDriveShow)}
                                onmouseout={link.callback(|_| Msg::FlipDriveShow)}>{"drive"}</button>
                            { if self.show_drive {html!{
                                <span class="modal-pop">
                                    {for props.drive.iter().map(|l| {
                                        html!{<p>{l}</p>}
                                    })}
                                </span>
                            }}else {
                                html!{}
                            }}
                        </div>
                    </div>

                    <div class="modal-section">
                        <p class="modal-label">{"Colour"}</p>
                        <div class="modal-row">
                            {
                                for PALETTE.iter().map(|&c| {
                                    let class = if props.setting.color==c {"swatch active"} else {"swatch"};
                                    html!{
                                        <button type="button" class={class}
                                            style={format!("--swatch:#{:02X}{:02X}{:02X}",c.0,c.1,c.2)}
                                            onclick={link.callback(move |_| Msg::ChooseColor(c))}>
                                        </button>
                                    }
                                })
                            }
                        </div>
                    </div>

                    <div class="modal-section">
                        <p class="modal-label">{"Signal"}</p>
                        <div class="modal-row">
                            <button type="button" class="btn"
                                title={props.path.clone()}
                                onclick={link.callback(|_| Msg::CopyPath)}>
                                {
                                    match self.copied {
                                        Some(true) => "Copied ✓",
                                        Some(false) => "Copy failed",
                                        None => "Copy path",
                                    }
                                }
                            </button>
                            <code class="modal-path">{props.path.clone()}</code>
                        </div>
                    </div>

                    <div class="modal-section">
                        <div class="modal-row">
                            <button type="button" class="btn danger"
                                onclick={link.callback(|_| Msg::DeleteSig)}>{"Remove signal"}</button>
                        </div>
                    </div>
                </div>
            </div>
        }
    }
}
