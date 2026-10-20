use wave_insight_lib::data_struct::Module;
use wave_insight_lib::waveform::Waveform;
use yew::prelude::*;
use web_sys::{DragEvent, Event, HtmlInputElement};
use gloo_file::File;

#[cfg(feature = "backend")]
use crate::file_load::FileList;
#[cfg(feature = "wasm")]
use crate::nav::SharedWaveform;

use gloo_file::callbacks::FileReader;
use web_sys::console;

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "tauri"])]
    async fn invoke(cmd: &str, args: JsValue) -> JsValue;
}

pub enum FileType {
    ///a waveform dump: vcd, fst or ghw, all handled by the waveform backend
    IsWaveform,
    IsVerilog,
}

///waveforms are told apart by extension; everything else is source
pub fn file_type_of(name: &str) -> FileType {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".vcd") || lower.ends_with(".fst") || lower.ends_with(".ghw") {
        FileType::IsWaveform
    }else {
        FileType::IsVerilog
    }
}

pub enum Msg {
    ///the bytes are read raw because fst is binary
    #[cfg(feature = "wasm")]
    Loaded(FileType,String,Vec<u8>),
    #[cfg(feature = "wasm")]
    Files(Vec<File>),
    #[cfg(feature = "wasm")]
    DragOver(DragEvent),
    /// Files were dropped on the drop zone; the browser has no file handle for
    /// a drop event, so they go through the same reader as the picker.
    #[cfg(feature = "wasm")]
    FileDrop(DragEvent),
    #[cfg(feature = "backend")]
    Get,
    #[cfg(feature = "backend")]
    ExitList,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct FileLoadProps {
    /// The file picker of the tauri/server transports; the browser reads the
    /// dropped files itself.
    #[cfg(feature = "backend")]
    pub ongetmodule: Callback<Module>,
    /// A dump was opened: its hierarchy (legacy module tree) and the handle the
    /// wave view decodes values from.
    #[cfg(feature = "wasm")]
    pub ongetwaveform: Callback<(Module, SharedWaveform)>,
    /// A source file was read; it goes to the design database, not to a parser.
    pub ongetverilog: Callback<(String, String)>,
    /// A file could not be used; the app shows it in the top bar.
    #[cfg(feature = "wasm")]
    pub onerror: Callback<String>,
}

pub struct FileLoad {
    #[cfg(feature = "wasm")]
    task: Vec<FileReader>,
    #[cfg(feature = "backend")]
    filelist_show: bool,
}

impl Component for FileLoad {
    type Message = Msg;
    type Properties = FileLoadProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            #[cfg(feature = "wasm")]
            task: vec![],
            #[cfg(feature = "backend")]
            filelist_show: false,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            #[cfg(feature = "wasm")]
            Msg::Loaded(file_type, file_name, bytes) => {
                match file_type {
                    FileType::IsWaveform => {
                        console::log_1(&format!("opening waveform {file_name}").into());
                        self.open_waveform(ctx, file_name, bytes);
                    },
                    FileType::IsVerilog => {
                        // the design database parses it; no separate scan here
                        match String::from_utf8(bytes) {
                            Ok(text) => ctx.props().ongetverilog.emit((file_name, text)),
                            Err(_) => ctx.props().onerror.emit(format!("{file_name}: not utf-8")),
                        }
                    },
                }
                true
            }
            #[cfg(feature = "wasm")]
            Msg::Files(result) => {
                console::log_1(&format!("input {} file",result.len()).into());
                for file in result.into_iter() {
                    let link = ctx.link().clone();
                    let file_name = file.name();
                    console::log_1(&format!("file name:{file_name}").into());
                    let file_type = file_type_of(&file_name);
                    let task = gloo_file::callbacks::read_as_bytes(&file, move |res| {
                        link.send_message(Msg::Loaded(
                            file_type,
                            file_name.clone(),
                            res.unwrap_or_default(),
                        ))
                    });
                    self.task.push(task);
                }
                true
            }
            #[cfg(feature = "wasm")]
            Msg::DragOver(e) => {
                // without this the browser refuses the drop
                e.prevent_default();
                true
            }
            #[cfg(feature = "wasm")]
            Msg::FileDrop(e) => {
                e.prevent_default();
                let files = e
                    .data_transfer()
                    .and_then(|data| data.files())
                    .map(files_from)
                    .unwrap_or_default();
                ctx.link().send_message(Msg::Files(files));
                true
            }
            #[cfg(feature = "backend")]
            Msg::Get => {
                self.filelist_show = true;
                true
            }
            #[cfg(feature = "backend")]
            Msg::ExitList => {
                self.filelist_show = false;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        let link = ctx.link();
        #[cfg(feature = "wasm")]
        let onchangefunc = link.callback(move |e: Event| {
                        let input: HtmlInputElement = e.target_unchecked_into();
                        Msg::Files(input.files().map(files_from).unwrap_or_default())
                    });
        #[cfg(feature = "wasm")]
        html! {
            <div class="file-strip">
                <label class="file-drop"
                    ondragover={link.callback(Msg::DragOver)}
                    ondrop={link.callback(Msg::FileDrop)}>
                    <input class="file-native" type="file" multiple=true onchange={onchangefunc}/>
                    <span class="file-drop-icon">{"＋"}</span>
                    <span class="file-drop-text">{"Load or drop .vcd / .fst / .ghw / .v files"}</span>
                </label>
            </div>
        }
        #[cfg(feature = "backend")]
        html! {
            <div>
                if self.filelist_show {
                    <FileList
                        onexit={link.callback(|_| Msg::ExitList)}
                        ongetmodule={ctx.props().ongetmodule.clone()}
                        ongetverilog={ctx.props().ongetverilog.clone()}
                    />
                }
                <div class="file-strip">
                    <button type="button" class="btn" onclick={link.callback(|_| Msg::Get)}>{"Open file…"}</button>
                </div>
            </div>
        }
    }
}

#[cfg(feature = "wasm")]
impl FileLoad {
    /// Open a dump: the header is parsed here, the values are decoded lazily per
    /// signal by the wave view.
    fn open_waveform(&mut self, ctx: &Context<Self>, file_name: String, bytes: Vec<u8>) {
        let error = match Waveform::open_bytes(bytes) {
            Ok(waveform) => match waveform.legacy_module() {
                Ok(module) => {
                    // `end_clock` lives on the root of the legay tree; the wave
                    // view scales its time axis with it
                    ctx.props()
                        .ongetwaveform
                        .emit((module, SharedWaveform::new(waveform)));
                    return;
                }
                Err(error) => error.message(),
            },
            Err(error) => error.message(),
        };
        console::log_1(&format!("{file_name}: {error}").into());
        ctx.props().onerror.emit(format!("{file_name}: {error}"));
    }
}

/// Turn a browser [`FileList`] into the files `gloo_file` can read.
#[cfg(feature = "wasm")]
fn files_from(list: web_sys::FileList) -> Vec<File> {
    match js_sys::try_iter(&list) {
        Ok(Some(files)) => files
            .filter_map(|file| file.ok())
            .map(web_sys::File::from)
            .map(File::from)
            .collect(),
        _ => Vec::new(),
    }
}
