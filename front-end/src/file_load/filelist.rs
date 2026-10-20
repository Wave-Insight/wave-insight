use wave_insight_lib::data_struct::Module;
use yew::prelude::*;
use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::{to_value, from_value};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "tauri"])]
    pub async fn invoke(cmd: &str, args: JsValue) -> JsValue;
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"])]
    pub async fn listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>);
}

///one row of the directory listing, produced by the `get_file_list` command
#[derive(Clone, Debug, PartialEq, Deserialize)]
struct FileEntry {
    name: String,
    is_dir: bool,
}

///how a file is going to be opened, decided from its extension
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Waveform,
    Source,
    Other,
}

fn kind_of(name: &str) -> Kind {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".vcd") || lower.ends_with(".fst") || lower.ends_with(".ghw") {
        Kind::Waveform
    }else if lower.ends_with(".sv") || lower.ends_with(".v") || lower.ends_with(".svh") || lower.ends_with(".vh") {
        Kind::Source
    }else {
        Kind::Other
    }
}

pub enum Msg {
    Exit,
    Up,
    ///enter a directory
    Into(String),
    ///open a file
    Open(String),
    SetList(Vec<FileEntry>),
    Failed(String),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct FileListProps {
    pub onexit: Callback<()>,
    pub ongetmodule: Callback<Module>,
    pub ongetverilog: Callback<(String, String)>,
}

pub struct FileList {
    list: Vec<FileEntry>,
    ///path components of the directory being shown, relative to the process
    ///working directory the backend starts in
    path: Vec<String>,
    loading: bool,
    error: Option<String>,
}

impl FileList {
    fn reload(&self, ctx: &Context<Self>) {
        let args = to_value(&GetFileListArgs { name: self.path.clone() }).unwrap_or(JsValue::NULL);
        let link = ctx.link().clone();
        spawn_local(async move {
            let ret = invoke("get_file_list", args).await;
            match from_value::<Vec<FileEntry>>(ret) {
                Ok(list) => link.send_message(Msg::SetList(list)),
                Err(_) => link.send_message(Msg::Failed(
                    "the folder could not be read".to_string()
                )),
            }
        });
    }
}

impl Component for FileList {
    type Message = Msg;
    type Properties = FileListProps;

    fn create(ctx: &Context<Self>) -> Self {
        let this = Self {
            list: vec![],
            path: vec![],
            loading: true,
            error: None,
        };
        this.reload(ctx);
        this
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Exit => {
                ctx.props().onexit.emit(());
                false
            }
            Msg::Up => {
                if self.path.pop().is_some() {
                    self.loading = true;
                    self.error = None;
                    self.reload(ctx);
                }
                true
            }
            Msg::Into(name) => {
                self.path.push(name);
                self.loading = true;
                self.error = None;
                self.reload(ctx);
                true
            }
            Msg::Open(name) => {
                let mut choose = self.path.clone();
                choose.push(name.clone());
                let args = match to_value(&GetFileListArgs { name: choose }) {
                    Ok(args) => args,
                    Err(_) => {
                        self.error = Some(format!("{name}: invalid path"));
                        return true;
                    },
                };
                match kind_of(&name) {
                    Kind::Waveform => {
                        let link = ctx.props().ongetmodule.clone();
                        let exit = ctx.props().onexit.clone();
                        spawn_local(async move {
                            if let Ok(module) = from_value::<Module>(invoke("choose_vcd", args).await) {
                                link.emit(module);
                                exit.emit(());
                            }
                        });
                    },
                    Kind::Source => {
                        let link_module = ctx.props().ongetmodule.clone();
                        let link_verilog = ctx.props().ongetverilog.clone();
                        let exit = ctx.props().onexit.clone();
                        let file = name.clone();
                        spawn_local(async move {
                            match from_value::<(String, Module)>(invoke("choose_verilog", args).await) {
                                Ok((text, module)) => {
                                    link_module.emit(module);
                                    link_verilog.emit((file, text));
                                    exit.emit(());
                                },
                                Err(_) => (),
                            }
                        });
                    },
                    Kind::Other => {
                        self.error = Some(format!("{name}: not a waveform or source file"));
                        return true;
                    },
                }
                true
            }
            Msg::SetList(list) => {
                self.loading = false;
                self.list = list;
                true
            }
            Msg::Failed(message) => {
                self.loading = false;
                self.error = Some(message);
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let here = if self.path.is_empty() {
            ".".to_string()
        }else {
            self.path.join("/")
        };

        html! {
            <div>
                <div class="overlay" onclick={link.callback(|_| Msg::Exit)}></div>
                <div class="modal file-modal">
                    <h4 class="modal-title">{"Open file"}</h4>

                    <div class="file-modal-bar">
                        <button class="btn" type="button" title="Go up one folder"
                            disabled={self.path.is_empty()}
                            onclick={link.callback(|_| Msg::Up)}>{"↑"}</button>
                        <span class="file-modal-path" title={here.clone()}>{here}</span>
                        <button class="btn" type="button" onclick={link.callback(|_| Msg::Exit)}>{"✕"}</button>
                    </div>

                    <div class="file-browser">
                        {
                            if self.loading {
                                html!{ <div class="file-browser-empty">{"Reading the folder…"}</div> }
                            }else if let Some(error) = &self.error {
                                html!{ <div class="file-browser-empty error">{error.clone()}</div> }
                            }else if self.list.is_empty() {
                                html!{ <div class="file-browser-empty">{"Nothing to open here."}</div> }
                            }else {
                                html!{}
                            }
                        }
                        {
                            for self.list.iter().map(|entry| {
                                let name = entry.name.clone();
                                let is_dir = entry.is_dir;
                                let kind = kind_of(&name);
                                let class = if is_dir {"file-row dir"} else {"file-row"};
                                let icon = if is_dir {
                                    "▸"
                                }else {
                                    match kind {
                                        Kind::Waveform => "∿",
                                        Kind::Source => "≡",
                                        Kind::Other => "·",
                                    }
                                };
                                let tag = if is_dir {
                                    ""
                                }else {
                                    match kind {
                                        Kind::Waveform => "waveform",
                                        Kind::Source => "source",
                                        Kind::Other => "unsupported",
                                    }
                                };
                                let name_click = name.clone();
                                let onclick = if is_dir {
                                    let n = name_click.clone();
                                    link.callback(move |_| Msg::Into(n.clone()))
                                }else {
                                    let n = name_click.clone();
                                    link.callback(move |_| Msg::Open(n.clone()))
                                };
                                html!{
                                    <div class={class} onclick={onclick} title={name.clone()}>
                                        <span class="file-row-icon">{icon}</span>
                                        <span class="file-row-name">{name.clone()}</span>
                                        <span class="file-row-tag">{tag}</span>
                                    </div>
                                }
                            })
                        }
                    </div>

                    <div class="file-modal-foot">
                        {"waveforms: .vcd .fst .ghw   ·   sources: .v .sv"}
                    </div>
                </div>
            </div>
        }
    }
}

#[derive(Serialize, Deserialize)]
struct GetFileListArgs {
    name: Vec<String>,
}
