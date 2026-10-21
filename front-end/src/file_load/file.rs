#[cfg(not(feature = "tauri"))]
use wave_insight_lib::waveform::Waveform;
use yew::prelude::*;
#[cfg(not(feature = "tauri"))]
use web_sys::{DragEvent, Event, HtmlInputElement};
#[cfg(not(feature = "tauri"))]
use gloo_file::File;
#[cfg(feature = "tauri")]
use crate::file_load::FileList;
use crate::state::types::DumpHandle;

#[cfg(not(feature = "tauri"))]
use gloo_file::callbacks::FileReader;
#[cfg(not(feature = "tauri"))]
use web_sys::console;

#[cfg(not(feature = "tauri"))]
#[derive(Clone, Copy)]
pub enum FileType {
    ///a waveform dump: vcd, fst or ghw, all handled by the waveform backend
    IsWaveform,
    ///a SystemVerilog/Verilog source, parsed by the design database
    IsVerilog,
    ///anything else: refused with a message instead of being fed to the parser
    Unknown,
}

/// Waveforms and sources are told apart by extension; an extension this app does
/// not handle is reported instead of being parsed as Verilog (a `.txt` dump used
/// to end up in sv-parser with a confusing "parsed" status).
#[cfg(not(feature = "tauri"))]
pub fn file_type_of(name: &str) -> FileType {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".vcd") || lower.ends_with(".fst") || lower.ends_with(".ghw") {
        FileType::IsWaveform
    } else if lower.ends_with(".sv")
        || lower.ends_with(".v")
        || lower.ends_with(".svh")
        || lower.ends_with(".vh")
    {
        FileType::IsVerilog
    } else {
        FileType::Unknown
    }
}

pub enum Msg {
    ///the bytes are read raw because fst is binary
    #[cfg(not(feature = "tauri"))]
    Loaded(FileType,String,Vec<u8>),
    #[cfg(not(feature = "tauri"))]
    Files(Vec<File>),
    #[cfg(not(feature = "tauri"))]
    DragOver(DragEvent),
    /// Files were dropped on the drop zone; the browser has no file handle for
    /// a drop event, so they go through the same reader as the picker.
    #[cfg(not(feature = "tauri"))]
    FileDrop(DragEvent),
    /// Reading a file from the browser failed (moved, unreadable, permission).
    #[cfg(not(feature = "tauri"))]
    LoadFailed(String, String),
    #[cfg(feature = "tauri")]
    Get,
    #[cfg(feature = "tauri")]
    ExitList,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct FileLoadProps {
    /// A dump was opened: the handle the wave view decodes values from.
    pub ongetwaveform: Callback<DumpHandle>,
    /// A source file was read; it goes to the design database, not to a parser.
    pub ongetverilog: Callback<(String, String)>,
    /// A file could not be used; the app shows it in the top bar.
    pub onerror: Callback<String>,
}

pub struct FileLoad {
    #[cfg(not(feature = "tauri"))]
    task: Vec<FileReader>,
    #[cfg(feature = "tauri")]
    filelist_show: bool,
}

impl Component for FileLoad {
    type Message = Msg;
    type Properties = FileLoadProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            #[cfg(not(feature = "tauri"))]
            task: vec![],
            #[cfg(feature = "tauri")]
            filelist_show: false,
        }
    }

    #[allow(unused_variables)]
    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            #[cfg(not(feature = "tauri"))]
            Msg::Loaded(file_type, file_name, bytes) => {
                match file_type {
                    FileType::IsWaveform => {
                        console::log_1(&format!("opening waveform {file_name}").into());
                        open_waveform(ctx, file_name, bytes);
                    },
                    FileType::IsVerilog => {
                        // the design database parses it; no separate scan here
                        match String::from_utf8(bytes) {
                            Ok(text) => ctx.props().ongetverilog.emit((file_name, text)),
                            Err(_) => ctx.props().onerror.emit(format!(
                                "{file_name}: not valid UTF-8 text — save the source as UTF-8 and load it again"
                            )),
                        }
                    },
                    FileType::Unknown => {
                        ctx.props().onerror.emit(format!(
                            "{file_name}: unsupported file type — load a waveform (.vcd/.fst/.ghw) or a source (.v/.sv/.vh/.svh)"
                        ));
                    },
                }
                true
            }
            #[cfg(not(feature = "tauri"))]
            Msg::LoadFailed(file_name, error) => {
                ctx.props().onerror.emit(format!(
                    "{file_name}: could not be read ({error}) — check that the file still exists and is readable"
                ));
                true
            }
            #[cfg(not(feature = "tauri"))]
            Msg::Files(result) => {
                console::log_1(&format!("input {} file",result.len()).into());
                for file in result.into_iter() {
                    let link = ctx.link().clone();
                    let file_name = file.name();
                    console::log_1(&format!("file name:{file_name}").into());
                    let file_type = file_type_of(&file_name);
                    let task = gloo_file::callbacks::read_as_bytes(&file, move |res| match res {
                        Ok(bytes) => link.send_message(Msg::Loaded(file_type, file_name.clone(), bytes)),
                        Err(error) => link.send_message(Msg::LoadFailed(
                            file_name.clone(),
                            error.to_string(),
                        )),
                    });
                    self.task.push(task);
                }
                true
            }
            #[cfg(not(feature = "tauri"))]
            Msg::DragOver(e) => {
                // without this the browser refuses the drop
                e.prevent_default();
                true
            }
            #[cfg(not(feature = "tauri"))]
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
            #[cfg(feature = "tauri")]
            Msg::Get => {
                self.filelist_show = true;
                true
            }
            #[cfg(feature = "tauri")]
            Msg::ExitList => {
                self.filelist_show = false;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        #[cfg(not(feature = "tauri"))]
        let onchangefunc = link.callback(move |e: Event| {
            let input: HtmlInputElement = e.target_unchecked_into();
            Msg::Files(input.files().map(files_from).unwrap_or_default())
        });
        // the desktop build has no file input: it browses the disk through tauri
        #[cfg(feature = "tauri")]
        let picker = html! {
            <div>
                if self.filelist_show {
                    <FileList
                        onexit={link.callback(|_| Msg::ExitList)}
                        ongetwaveform={ctx.props().ongetwaveform.clone()}
                        ongetverilog={ctx.props().ongetverilog.clone()}
                        onerror={ctx.props().onerror.clone()}
                    />
                }
                <div class="file-strip">
                    <button type="button" class="btn" onclick={link.callback(|_| Msg::Get)}>{"Open file…"}</button>
                </div>
            </div>
        };
        #[cfg(not(feature = "tauri"))]
        let picker = html! {
            <div class="file-strip">
                <label class="file-drop"
                    ondragover={link.callback(Msg::DragOver)}
                    ondrop={link.callback(Msg::FileDrop)}>
                    <input class="file-native" type="file" multiple=true onchange={onchangefunc}/>
                    <span class="file-drop-icon">{"＋"}</span>
                    <span class="file-drop-text">{"Load or drop .vcd / .fst / .ghw / .v files"}</span>
                </label>
            </div>
        };
        picker
    }
}

/// Open a dump: only the header is parsed here (plus the cheap time range scan),
/// the values are decoded lazily per signal by the wave view.
#[cfg(not(feature = "tauri"))]
pub fn open_waveform(ctx: &Context<FileLoad>, file_name: String, bytes: Vec<u8>) {
    let started = js_sys::Date::now();
    let opened = Waveform::open_bytes(bytes);
    let open_ms = js_sys::Date::now() - started;
    match opened {
        Ok(waveform) => {
            let started = js_sys::Date::now();
            let range = waveform.time_range_hint().ok().flatten();
            let range_ms = js_sys::Date::now() - started;
            console::log_1(
                &format!(
                    "waveform {file_name}: opened in {open_ms:.1} ms, trace range in {range_ms:.1} ms ({:?}, {} signals)",
                    range,
                    waveform.signal_count()
                )
                .into(),
            );
            ctx.props().ongetwaveform.emit(DumpHandle::new(waveform));
        }
        Err(error) => {
            // the library message says *what* is wrong ("unknown waveform
            // format", "corrupt ..."); add what the user can do about it
            let error = error.message();
            console::log_1(&format!("{file_name}: {error}").into());
            ctx.props().onerror.emit(format!(
                "{file_name}: could not be opened ({error}) — check that it is an intact VCD/FST/GHW dump"
            ));
        }
    }
}

/// Turn a browser [`FileList`] into the files `gloo_file` can read.
#[cfg(not(feature = "tauri"))]
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
