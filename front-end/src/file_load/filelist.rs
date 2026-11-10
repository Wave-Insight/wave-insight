use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::{from_value, to_value};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::state::types::DumpHandle;

use super::file::{next_tick, parse_dump};
use super::progress;

#[wasm_bindgen]
extern "C" {
    // tauri 2 renamed the JS module: __TAURI__.tauri is now __TAURI__.core.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "core"])]
    pub async fn invoke(cmd: &str, args: JsValue) -> JsValue;
    // tauri 2 removed the tauri://file-drop event; the webview owns the
    // drag-drop API now. The listener itself is installed in app.rs.
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "webview"], js_name = getCurrentWebview)]
    pub fn current_webview() -> JsValue;
}

///one row of the directory listing, produced by the `get_file_list` command
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct FileEntry {
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
    } else if lower.ends_with(".sv")
        || lower.ends_with(".v")
        || lower.ends_with(".svh")
        || lower.ends_with(".vh")
    {
        Kind::Source
    } else {
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
    /// A dump was opened; the webview parsed the bytes the command returned.
    pub ongetwaveform: Callback<DumpHandle>,
    pub ongetverilog: Callback<(String, String)>,
    /// The file could not be read or parsed.
    pub onerror: Callback<String>,
    /// Progress and notes (info tone): the top bar shows them.
    #[prop_or_default]
    pub onstatus: Callback<String>,
    /// What the last successful parse cost; the app appends it to the success
    /// line.
    #[prop_or_default]
    pub onloadreport: Callback<crate::file_load::LoadReport>,
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
        let args = to_value(&GetFileListArgs {
            name: self.path.clone(),
        })
        .unwrap_or(JsValue::NULL);
        let link = ctx.link().clone();
        spawn_local(async move {
            let ret = invoke("get_file_list", args).await;
            match from_value::<Vec<FileEntry>>(ret) {
                Ok(list) => link.send_message(Msg::SetList(list)),
                Err(_) => {
                    link.send_message(Msg::Failed("the folder could not be read".to_string()))
                }
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
                // the native backend resolves the dialog's navigation parts against
                // its own base directory (the directory the listing came from), so
                // the parts are passed as-is: a joined string would be relative to
                // the process working directory instead and would not open
                let native_parts = choose.clone();
                let args = match to_value(&GetFileListArgs { name: choose }) {
                    Ok(args) => args,
                    Err(_) => {
                        self.error = Some(format!("{name}: invalid path"));
                        return true;
                    }
                };
                match kind_of(&name) {
                    Kind::Waveform => {
                        let ongetwaveform = ctx.props().ongetwaveform.clone();
                        let onreport = ctx.props().onloadreport.clone();
                        let onstatus = ctx.props().onstatus.clone();
                        let onerror = ctx.props().onerror.clone();
                        let exit = ctx.props().onexit.clone();
                        let file = name.clone();
                        spawn_local(async move {
                            // Wave 15/17: try the native backend first (mmap,
                            // windowed IPC). Large dumps stay native and read-only.
                            // A dump the backend proved small (the >64 MiB
                            // threshold) keeps the wasm reader; a native ERROR does
                            // not fall back, because copying a possibly huge file
                            // into wasm to retry is the multi-GB peak we avoid.
                            match crate::remote::try_open_parts(&native_parts).await {
                                Ok(Some(session)) => {
                                    if crate::remote::deliver_open(session.clone()) {
                                        exit.emit(());
                                        return;
                                    }
                                    // no listener (the app is not ready): close it
                                    // again instead of leaking the mapping
                                    let id = session.id();
                                    spawn_local(async move {
                                        let _ = crate::remote::close(id).await;
                                    });
                                    onerror.emit(format!(
                                        "{file}: the native dump was opened but the app could not display it; it was closed again"
                                    ));
                                    return;
                                }
                                // proven small -> fall through to the wasm reader
                                Ok(None) => {}
                                Err(error) => match crate::remote::native_fallback(false) {
                                    crate::remote::NativeFallback::Wasm => {}
                                    crate::remote::NativeFallback::Report => {
                                        onerror.emit(crate::remote::native_error_status(
                                            &file, &error,
                                        ));
                                        return;
                                    }
                                },
                            }
                            // The command returns the whole file as a *raw* IPC
                            // body (tauri::ipc::Response): invoke resolves to an
                            // ArrayBuffer and dump_bytes copies it into a Vec<u8>
                            // once. There is no slice to report progress for (the
                            // backend hands back one buffer), so the dialog gets
                            // the size guard and the parse line but no percentage.
                            let raw = invoke("choose_vcd", args).await;
                            let bytes = match dump_bytes(&raw) {
                                Some(bytes) => bytes,
                                None => {
                                    onerror.emit(format!("{file}: the dump could not be read"));
                                    return;
                                }
                            };
                            if bytes.is_empty() {
                                onerror.emit(format!("{file}: the dump could not be read"));
                                return;
                            }
                            let size = bytes.len() as u64;
                            if let Some(message) = progress::guard_message(&file, size) {
                                match progress::size_verdict(size) {
                                    // the bytes are already here; refusing to
                                    // parse them is still better than freezing
                                    progress::SizeVerdict::TooLarge => {
                                        onerror.emit(message);
                                        return;
                                    }
                                    progress::SizeVerdict::Large => onstatus.emit(message),
                                    progress::SizeVerdict::Ok => {}
                                }
                            }
                            onstatus.emit(progress::parsing_status(&file, size, None));
                            next_tick().await;
                            match parse_dump(&file, bytes) {
                                Ok((handle, report)) => {
                                    onreport.emit(report);
                                    ongetwaveform.emit(handle);
                                    exit.emit(());
                                }
                                Err(message) => onerror.emit(message),
                            }
                        });
                    }
                    Kind::Source => {
                        let link_verilog = ctx.props().ongetverilog.clone();
                        let onerror = ctx.props().onerror.clone();
                        let exit = ctx.props().onexit.clone();
                        let file = name.clone();
                        spawn_local(async move {
                            match from_value::<String>(invoke("choose_verilog", args).await) {
                                Ok(text) => {
                                    link_verilog.emit((file, text));
                                    exit.emit(());
                                }
                                Err(_) => {
                                    onerror.emit(format!("{file}: the source could not be read"))
                                }
                            }
                        });
                    }
                    Kind::Other => {
                        self.error = Some(format!("{name}: not a waveform or source file"));
                        return true;
                    }
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
        } else {
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

/// Turn the raw result of a dump command into bytes.
///
/// choose_vcd / choose_vcd_absolute answer with tauri::ipc::Response, i.e. an
/// InvokeResponseBody::Raw: the webview's invoke resolves to an ArrayBuffer, so
/// this is the single copy into wasm memory (Uint8Array::to_vec allocates the
/// Vec<u8> and copies once). That replaces the old path, where the command
/// returned Vec<u8> and Tauri's default IPC serialized it to a JSON array of
/// numbers (a 100 MB dump became hundreds of MB of JSON plus a 100M-element JS
/// array in the webview).
///
/// A Uint8Array view is accepted too (defensive: another transport could hand
/// one over), and the legacy JSON number-array shape is kept as a fallback so a
/// stale dist still opens dumps - but that fallback is the slow path this
/// change exists to remove.
pub fn dump_bytes(value: &JsValue) -> Option<Vec<u8>> {
    if value.is_instance_of::<js_sys::ArrayBuffer>() {
        return Some(js_sys::Uint8Array::new(value).to_vec());
    }
    if value.is_instance_of::<js_sys::Uint8Array>() {
        let view: &js_sys::Uint8Array = value.unchecked_ref();
        return Some(view.to_vec());
    }
    if js_sys::Array::is_array(value) {
        return from_value::<Vec<u8>>(value.clone()).ok();
    }
    None
}
