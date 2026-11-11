//! The file picker: chunked reads with progress and a cancel action, the size
//! guard, and the two open paths (browser picker / drop, tauri file dialog).
//!
//! Opening a big dump used to be one read_as_bytes plus one synchronous parse,
//! which froze the tab for seconds and could exceed the browser's wasm memory.
//! The browser path now reads ~8 MB slices (progress::CHUNK_BYTES) through
//! File::slice + Blob::array_buffer, reports every slice in the top bar, yields
//! to the event loop so the line paints, and lets Esc / the cancel button abort
//! before the parse. The parse itself stays synchronous (the waveform backend
//! is), so it announces itself first and starts one frame later.
//!
//! The tauri dialog still returns whole bytes from the backend command: it gets
//! the size guard, the "parsing ..." line and the load report, but not the
//! per-chunk progress (there is nothing to slice through invoke).

use crate::state::types::DumpHandle;
#[cfg(not(feature = "tauri"))]
use crate::wave_show::memory as wave_memory;
use wasm_bindgen_futures::JsFuture;
use web_sys::console;
use yew::prelude::*;

use super::progress;

#[cfg(not(feature = "tauri"))]
use std::cell::Cell;
#[cfg(not(feature = "tauri"))]
use std::collections::VecDeque;
#[cfg(not(feature = "tauri"))]
use std::rc::Rc;

#[cfg(not(feature = "tauri"))]
use wasm_bindgen::closure::Closure;
#[cfg(not(feature = "tauri"))]
use wasm_bindgen::JsCast;

#[cfg(not(feature = "tauri"))]
use wasm_bindgen_futures::spawn_local;

#[cfg(feature = "tauri")]
use crate::file_load::FileList;

#[cfg(not(feature = "tauri"))]
use web_sys::{DragEvent, Event, File, HtmlInputElement, KeyboardEvent};

use wave_insight_lib::waveform::Waveform;

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
/// not handle is reported instead of being parsed as Verilog (a .txt dump used
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

/// What one open attempt cost, reported upwards so the app can extend the
/// success line (the app owns the status line, the loader owns the timings).
#[derive(Clone, Debug, PartialEq)]
pub struct LoadReport {
    /// The file name the user picked, without the directory.
    pub file_name: String,
    /// Bytes that were read and handed to the parser.
    pub bytes: u64,
    /// Cheap content fingerprint of the parsed bytes (see [`fingerprint`]).
    ///
    /// The dump identity is the file name plus the byte size, and a re-simulated
    /// dump can have exactly the size of the one that is already open. The app
    /// compares this fingerprint with the open dump's: equal means the same
    /// content (focus the tab, as before), different means the file was
    /// re-written and its tab has to reload the new waveform in place.
    pub fingerprint: u64,
    /// Milliseconds the synchronous parse took.
    pub parse_ms: f64,
    /// Size of the wasm linear memory after the parse, in MB; None where it
    /// cannot be read (the host target the unit tests run on).
    pub memory_mb: Option<f64>,
}

impl LoadReport {
    /// The success line, keeping the stable "waveform loaded" prefix.
    pub fn status(&self) -> String {
        progress::loaded_status(self.parse_ms, self.bytes, self.memory_mb)
    }

    /// Just the numbers, for a status line that already says what opened.
    pub fn suffix(&self) -> String {
        progress::stats_suffix(self.parse_ms, self.bytes, self.memory_mb)
    }
}

/// Parse a whole dump and time it.
///
/// Shared by the browser reader and the tauri paths: all three end up in the
/// same synchronous Waveform::open_bytes, so the timing/reporting must not be
/// duplicated per transport. An Err is the message to show the user (what is
/// wrong, then what to do about it).
pub fn parse_dump(file_name: &str, bytes: Vec<u8>) -> Result<(DumpHandle, LoadReport), String> {
    let size = bytes.len() as u64;
    // the fingerprint rides along with the bytes the parse already owns, so it
    // costs one linear pass over a buffer that was just read (see [`fingerprint`])
    let content = fingerprint(&bytes);
    let started = js_sys::Date::now();
    let opened = Waveform::open_bytes(bytes);
    let parse_ms = js_sys::Date::now() - started;
    let report = LoadReport {
        file_name: file_name.to_string(),
        bytes: size,
        fingerprint: content,
        parse_ms,
        memory_mb: wasm_memory_mb(),
    };
    match opened {
        Ok(waveform) => {
            console::log_1(
                &format!(
                    "waveform {file_name}: opened in {parse_ms:.1} ms ({:?}, {} signals)",
                    waveform.file_format(),
                    waveform.signal_count()
                )
                .into(),
            );
            Ok((DumpHandle::new(waveform), report))
        }
        Err(error) => {
            // the library message says *what* is wrong ("unknown waveform
            // format", "corrupt ..."); add what the user can do about it
            let error = error.message();
            console::log_1(&format!("{file_name}: {error}").into());
            Err(format!(
                "{file_name}: could not be opened ({error}) — check that it is an intact VCD/FST/GHW dump"
            ))
        }
    }
}

/// A cheap content fingerprint of a dump's bytes.
///
/// The dump identity is the file name plus the byte size; that is enough to
/// dedupe a file that is opened twice, but not to tell the open file from a
/// re-simulated one that happens to have the same size. The store compares this
/// hash to decide whether a re-open is the same content (focus the tab) or new
/// bytes (reload the tab in place, see `Action::DumpAdded`).
///
/// FNV-1a over 8-byte words: one linear pass at memory bandwidth, noise next to
/// the parse it rides along with, and stable across runs (no random state), so
/// the same file always fingerprints the same. This is a change detector, not a
/// cryptographic digest: a collision would only mean a re-written dump is not
/// refreshed, nothing worse.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET_BASIS;
    let (words, remainder) = bytes.as_chunks::<8>();
    for word in words {
        hash = (hash ^ u64::from_le_bytes(*word)).wrapping_mul(PRIME);
    }
    for byte in remainder {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(PRIME);
    }
    // the size is part of the identity already; folding it in costs nothing and
    // keeps the hash well defined for the empty payload
    hash ^ (bytes.len() as u64).wrapping_mul(PRIME)
}

/// Size of the wasm linear memory in MiB, as the browser reports it.
///
/// Only meaningful on wasm32; the host target (unit tests) has no such memory,
/// so the helpers there return None instead of pretending.
#[cfg(target_arch = "wasm32")]
pub fn wasm_memory_mb() -> Option<f64> {
    let memory = wasm_bindgen::memory();
    let buffer = js_sys::Reflect::get(&memory, &wasm_bindgen::JsValue::from_str("buffer")).ok()?;
    let byte_length =
        js_sys::Reflect::get(&buffer, &wasm_bindgen::JsValue::from_str("byteLength")).ok()?;
    let byte_length = byte_length.as_f64()?;
    Some(byte_length / (1024.0 * 1024.0))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn wasm_memory_mb() -> Option<f64> {
    None
}

/// One macrotask turn: lets the browser paint the status line before the next
/// chunk is read (or before the synchronous parse starts).
///
/// Awaiting a bare promise only yields a microtask, and microtasks never reach
/// the paint step, so the "reading ..." line would not be visible until the
/// whole file was already there.
pub(crate) async fn next_tick() {
    if let Some(window) = web_sys::window() {
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
        });
        let _ = JsFuture::from(promise).await;
    }
}

pub enum Msg {
    /// Files were picked or dropped; they are queued and read one at a time.
    #[cfg(not(feature = "tauri"))]
    Files(Vec<File>),
    #[cfg(not(feature = "tauri"))]
    DragOver(DragEvent),
    /// Files were dropped on the drop zone; the browser has no file handle for
    /// a drop event, so they go through the same reader as the picker.
    #[cfg(not(feature = "tauri"))]
    FileDrop(DragEvent),
    /// One slice of the current file was read.
    #[cfg(not(feature = "tauri"))]
    Progress(String, u64, u64),
    /// All bytes are in memory; (type, name, bytes, $var estimate, generation).
    #[cfg(not(feature = "tauri"))]
    Read(FileType, String, Vec<u8>, Option<u64>, u32),
    /// The parse starts here, one frame after the "parsing ..." line.
    #[cfg(not(feature = "tauri"))]
    Parse(String, Vec<u8>, u32),
    /// The user aborted the read: nothing is opened.
    #[cfg(not(feature = "tauri"))]
    Cancel,
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
    /// Progress and one-line notes (info tone, never an error): the app shows
    /// them in the top bar. Called once per read chunk.
    #[prop_or_default]
    pub onstatus: Callback<String>,
    /// What the last successful parse cost; the app appends the numbers to the
    /// success line.
    #[prop_or_default]
    pub onloadreport: Callback<LoadReport>,
}

/// A file waiting for its turn. Only one is read at a time, so at most one set
/// of bytes is ever in wasm memory.
#[cfg(not(feature = "tauri"))]
struct PendingFile {
    file: File,
    name: String,
    size: u64,
    file_type: FileType,
}

/// The file being read right now; it drives the progress row in the sidebar.
#[cfg(not(feature = "tauri"))]
struct Reading {
    name: String,
    total: u64,
    done: u64,
}

pub struct FileLoad {
    #[cfg(not(feature = "tauri"))]
    queue: VecDeque<PendingFile>,
    #[cfg(not(feature = "tauri"))]
    reading: Option<Reading>,
    /// Set by the cancel button / Esc; the chunk loop checks it before every
    /// slice, so a cancelled load never reaches the parser.
    #[cfg(not(feature = "tauri"))]
    cancel: Option<Rc<Cell<bool>>>,
    /// Bumped by a cancel only: a message from an abandoned read carries the
    /// epoch it was started with and is dropped. Starting the *next* file must
    /// not bump it, or a parse that is still one frame away would be lost.
    #[cfg(not(feature = "tauri"))]
    epoch: u32,
    /// The window keydown listener that turns Esc into a cancel while reading.
    #[cfg(not(feature = "tauri"))]
    esc: Option<Closure<dyn FnMut(KeyboardEvent)>>,
    #[cfg(feature = "tauri")]
    filelist_show: bool,
}

impl Component for FileLoad {
    type Message = Msg;
    type Properties = FileLoadProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            #[cfg(not(feature = "tauri"))]
            queue: VecDeque::new(),
            #[cfg(not(feature = "tauri"))]
            reading: None,
            #[cfg(not(feature = "tauri"))]
            cancel: None,
            #[cfg(not(feature = "tauri"))]
            epoch: 0,
            #[cfg(not(feature = "tauri"))]
            esc: None,
            #[cfg(feature = "tauri")]
            filelist_show: false,
        }
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        #[cfg(not(feature = "tauri"))]
        self.detach_esc();
    }

    #[allow(unused_variables)]
    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            #[cfg(not(feature = "tauri"))]
            Msg::Files(files) => {
                console::log_1(&format!("input {} file", files.len()).into());
                self.enqueue(ctx, files);
                self.reading.is_some() || !self.queue.is_empty()
            }
            #[cfg(not(feature = "tauri"))]
            Msg::Progress(name, done, total) => {
                let Some(reading) = self.reading.as_mut() else {
                    return false;
                };
                if reading.name != name {
                    return false;
                }
                reading.done = done;
                reading.total = total;
                // the top bar carries the same line as the sidebar row
                ctx.props()
                    .onstatus
                    .emit(progress::reading_status(&name, done, total));
                true
            }
            #[cfg(not(feature = "tauri"))]
            Msg::Read(file_type, file_name, bytes, signals, epoch) => {
                if epoch != self.epoch {
                    // a cancelled read finished after the fact: drop it
                    return false;
                }
                self.reading = None;
                self.cancel = None;
                self.detach_esc();
                match file_type {
                    FileType::IsWaveform => {
                        let size = bytes.len() as u64;
                        ctx.props()
                            .onstatus
                            .emit(progress::parsing_status(&file_name, size, signals));
                        // the parse is synchronous (and may block for seconds):
                        // give the status line a frame to paint before it runs
                        let link = ctx.link().clone();
                        let name = file_name.clone();
                        let epoch = self.epoch;
                        spawn_local(async move {
                            next_tick().await;
                            next_tick().await;
                            link.send_message(Msg::Parse(name, bytes, epoch));
                        });
                    }
                    FileType::IsVerilog => {
                        // the design database parses it; no separate scan here
                        match String::from_utf8(bytes) {
                            Ok(text) => ctx.props().ongetverilog.emit((file_name, text)),
                            Err(_) => ctx.props().onerror.emit(format!(
                                "{file_name}: not valid UTF-8 text — save the source as UTF-8 and load it again"
                            )),
                        }
                        self.start_next(ctx);
                    }
                    FileType::Unknown => {
                        ctx.props().onerror.emit(format!(
                            "{file_name}: unsupported file type — load a waveform (.vcd/.fst/.ghw) or a source (.v/.sv/.vh/.svh)"
                        ));
                        self.start_next(ctx);
                    }
                }
                true
            }
            #[cfg(not(feature = "tauri"))]
            Msg::Parse(file_name, bytes, epoch) => {
                if epoch != self.epoch {
                    return false;
                }
                match parse_dump(&file_name, bytes) {
                    Ok((handle, report)) => {
                        // the report goes first so the app can hang the numbers
                        // on the success line before the handle arrives
                        ctx.props().onloadreport.emit(report);
                        ctx.props().ongetwaveform.emit(handle);
                    }
                    Err(message) => ctx.props().onerror.emit(message),
                }
                self.start_next(ctx);
                true
            }
            #[cfg(not(feature = "tauri"))]
            Msg::Cancel => {
                // everything queued or in flight belongs to the old epoch now
                self.epoch = self.epoch.wrapping_add(1);
                if let Some(cancel) = self.cancel.take() {
                    cancel.set(true);
                }
                self.queue.clear();
                self.detach_esc();
                match self.reading.take() {
                    Some(reading) => {
                        ctx.props()
                            .onstatus
                            .emit(progress::cancelled_status(&reading.name));
                        true
                    }
                    None => false,
                }
            }
            #[cfg(not(feature = "tauri"))]
            Msg::LoadFailed(file_name, error) => {
                self.reading = None;
                self.cancel = None;
                self.detach_esc();
                ctx.props().onerror.emit(format!(
                    "{file_name}: could not be read ({error}) — check that the file still exists and is readable"
                ));
                self.start_next(ctx);
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
                        onstatus={ctx.props().onstatus.clone()}
                        onloadreport={ctx.props().onloadreport.clone()}
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
                {self.progress_row(ctx)}
            </div>
        };
        picker
    }
}

#[cfg(not(feature = "tauri"))]
impl FileLoad {
    /// Queue picked/dropped files: refuse what cannot be loaded at all, warn
    /// about what is large, then start the first read.
    fn enqueue(&mut self, ctx: &Context<Self>, files: Vec<File>) {
        // the pre-flight estimate is cumulative over the files of this drop: two
        // 400 MB dumps each look fine on an empty tab and are fatal together
        let mut projected = wave_memory::wasm_bytes();
        for file in files {
            let name = file.name();
            console::log_1(&format!("file name:{name}").into());
            let size = file_blob(&file).size() as u64;
            let file_type = file_type_of(&name);
            if matches!(file_type, FileType::Unknown) {
                // read nothing: the extension alone already says it is useless
                ctx.props().onerror.emit(format!(
                    "{name}: unsupported file type — load a waveform (.vcd/.fst/.ghw) or a source (.v/.sv/.vh/.svh)"
                ));
                continue;
            }
            if let Some(message) = progress::guard_message(&name, size) {
                if progress::size_verdict(size) == progress::SizeVerdict::TooLarge {
                    // refuse before a byte is read: the tab would not survive it
                    ctx.props().onerror.emit(message);
                    continue;
                }
                ctx.props().onstatus.emit(message);
            }
            // Wave 12: the static guard above is about the file size; this one is
            // about this tab's *leftover* wasm memory. The estimate is measured
            // (base + ~3.2x the file size for a VCD), so a near-limit dump is
            // refused with the numbers instead of aborting the allocator halfway
            // through the read.
            match wave_memory::preflight(&name, size, projected) {
                wave_memory::Preflight::Refuse(message) => {
                    ctx.props().onerror.emit(message);
                    continue;
                }
                wave_memory::Preflight::Tight(message) => {
                    ctx.props().onstatus.emit(message);
                    projected = projected.saturating_add(wave_memory::estimated_need(
                        size,
                        wave_memory::format_of(&name),
                    ));
                }
                wave_memory::Preflight::Ok => {
                    projected = projected.saturating_add(wave_memory::estimated_need(
                        size,
                        wave_memory::format_of(&name),
                    ));
                }
            }
            self.queue.push_back(PendingFile {
                file,
                name,
                size,
                file_type,
            });
        }
        self.start_next(ctx);
    }

    /// Start reading the next queued file, if no read is running.
    fn start_next(&mut self, ctx: &Context<Self>) {
        if self.reading.is_some() {
            return;
        }
        let Some(pending) = self.queue.pop_front() else {
            self.detach_esc();
            return;
        };
        let epoch = self.epoch;
        let PendingFile {
            file,
            name,
            size,
            file_type,
        } = pending;
        let cancel = Rc::new(Cell::new(false));
        self.cancel = Some(cancel.clone());
        self.reading = Some(Reading {
            name: name.clone(),
            total: size,
            done: 0,
        });
        self.attach_esc(ctx);
        ctx.props()
            .onstatus
            .emit(progress::reading_status(&name, 0, size));

        // the header scan is only meaningful for a text VCD; FST/GHW are binary
        let vcd = name.to_ascii_lowercase().ends_with(".vcd");
        let link = ctx.link().clone();
        spawn_local(async move {
            // the file (and the Blob view of it) lives inside the future: the
            // reader must own everything it borrows across an await
            let blob = file_blob(&file);
            // Wave 12: reserve the file once and fallibly. Growing a Vec by
            // reallocation needs up to twice the file size at the copy, and an
            // allocation failure in wasm aborts the tab; try_reserve_exact gives
            // the exact final capacity and turns "no memory" into a message.
            let mut bytes: Vec<u8> = match reserve_buffer(size) {
                Ok(bytes) => bytes,
                Err(error) => {
                    link.send_message(Msg::LoadFailed(name.clone(), error));
                    return;
                }
            };
            let mut estimate = if vcd {
                Some(progress::SignalEstimate::new())
            } else {
                None
            };
            let mut offset = 0u64;
            while offset < size {
                if cancel.get() {
                    // the cancel handler already cleared the UI and the queue;
                    // dropping the bytes here is all that is left to do
                    return;
                }
                let end = (offset + progress::CHUNK_BYTES).min(size);
                let chunk = match read_slice(blob, offset, end).await {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        link.send_message(Msg::LoadFailed(name.clone(), error));
                        return;
                    }
                };
                offset = end;
                bytes.extend_from_slice(&chunk);
                if let Some(estimate) = estimate.as_mut() {
                    estimate.feed(&chunk);
                }
                link.send_message(Msg::Progress(name.clone(), offset, size));
                // paint the progress line before the next slice is read
                next_tick().await;
            }
            let signals = estimate.and_then(|estimate| match estimate.count() {
                0 => None,
                count => Some(count),
            });
            link.send_message(Msg::Read(file_type, name, bytes, signals, epoch));
        });
    }

    /// Register the Esc listener while a read runs: the button is in the
    /// sidebar, Esc must work wherever the focus is.
    fn attach_esc(&mut self, ctx: &Context<Self>) {
        if self.esc.is_some() {
            return;
        }
        let Some(window) = web_sys::window() else {
            return;
        };
        let link = ctx.link().clone();
        let listener = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
            if event.key() == "Escape" {
                link.send_message(Msg::Cancel);
            }
        });
        if window
            .add_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref())
            .is_ok()
        {
            self.esc = Some(listener);
        }
    }

    fn detach_esc(&mut self) {
        if let (Some(window), Some(listener)) = (web_sys::window(), self.esc.take()) {
            let _ = window
                .remove_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref());
        }
    }

    /// The progress row under the drop zone while a dump is being read; the
    /// cancel button next to the status is the mouse version of Esc.
    fn progress_row(&self, ctx: &Context<Self>) -> Html {
        let Some(reading) = &self.reading else {
            return html! {};
        };
        let text = progress::reading_status(&reading.name, reading.done, reading.total);
        let percent = progress::percent(reading.done, reading.total);
        html! {
            <div class="file-progress">
                <div class="file-progress-head">
                    <span class="file-progress-text" title={text.clone()}>{text}</span>
                    <button type="button" class="btn file-cancel"
                        title="Stop reading this dump (Esc)"
                        onclick={ctx.link().callback(|_| Msg::Cancel)}>{"Cancel"}</button>
                </div>
                <div class="file-progress-bar">
                    <div class="file-progress-fill" style={format!("width:{percent}%")}></div>
                </div>
            </div>
        }
    }
}

/// Reserve the read buffer for a whole file in one fallible allocation.
///
/// The old reader started at 8 MB and let the vector grow: every growth step
/// copies the data (up to twice the file size transiently) and an allocation
/// failure in wasm does not return an error, it aborts the tab. This reserves
/// the exact capacity up front and turns "not enough memory" into a message the
/// loader can show (the pre-flight check should have caught it first).
#[cfg(not(feature = "tauri"))]
fn reserve_buffer(total: u64) -> Result<Vec<u8>, String> {
    let size = usize::try_from(total).map_err(|_| {
        "the file is too large for this build's address space — open it in the desktop build"
            .to_string()
    })?;
    let mut bytes: Vec<u8> = Vec::new();
    bytes.try_reserve_exact(size).map_err(|_| {
        format!(
            "not enough free wasm memory to hold {} — close a dump you are not looking at and try again",
            progress::format_size(total)
        )
    })?;
    Ok(bytes)
}

/// Read one slice of a browser File as raw bytes.
#[cfg(not(feature = "tauri"))]
async fn read_slice(blob: &web_sys::Blob, start: u64, end: u64) -> Result<Vec<u8>, String> {
    let slice = blob
        .slice_with_f64_and_f64(start as f64, end as f64)
        .map_err(|_| "the browser refused to slice the file".to_string())?;
    let buffer = JsFuture::from(slice.array_buffer())
        .await
        .map_err(|_| "the browser could not read the bytes".to_string())?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// A browser File is a Blob: the reader and the size guard both only need the
/// Blob view of it.
#[cfg(not(feature = "tauri"))]
fn file_blob(file: &File) -> &web_sys::Blob {
    file.as_ref()
}

/// Turn a browser FileList into the files the reader can slice.
#[cfg(not(feature = "tauri"))]
fn files_from(list: web_sys::FileList) -> Vec<File> {
    match js_sys::try_iter(&list) {
        Ok(Some(files)) => files.filter_map(|file| file.ok()).map(File::from).collect(),
        _ => Vec::new(),
    }
}
#[cfg(all(test, not(feature = "tauri")))]
mod tests {
    use super::*;

    #[test]
    fn the_read_buffer_is_reserved_exactly() {
        let bytes = reserve_buffer(1024 * 1024).expect("a small reservation succeeds");
        assert!(bytes.is_empty());
        assert!(bytes.capacity() >= 1024 * 1024);
    }

    #[test]
    fn an_impossible_reservation_is_an_error_not_an_abort() {
        // the old path let the vector grow into the allocator, which aborts a
        // wasm tab; the reserve has to come back as a message instead
        let error = reserve_buffer(u64::MAX).expect_err("this cannot be reserved");
        assert!(
            error.contains("wasm memory") || error.contains("address space"),
            "unhelpful message: {error}"
        );
    }

    #[test]
    fn the_content_fingerprint_separates_same_length_dumps() {
        // the case the fingerprint exists for: a re-simulated dump with the same
        // name and the same byte size but different values
        let a = include_bytes!("../../../qa/fixtures/wave.vcd");
        let mut b = a.to_vec();
        let marker = b"b00000001 $";
        let at = b
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("the fixture has a pc value to change");
        b[at..at + marker.len()].copy_from_slice(b"b10101010 $");
        assert_eq!(a.len(), b.len(), "the pair is a same-size pair");
        assert_ne!(fingerprint(a), fingerprint(&b));
        // the same bytes always fingerprint the same, whatever buffer holds them
        // (a re-open is a dedupe, and the loader hands over a Vec)
        let owned = a.to_vec();
        assert_eq!(fingerprint(a), fingerprint(&owned));
        // the empty payload is well defined and distinct from one zero byte
        assert_eq!(fingerprint(b""), fingerprint(b""));
        assert_ne!(fingerprint(b""), fingerprint(b"\0"));
        // a one-byte change anywhere is enough (no sampling window to miss it)
        for at in [0usize, a.len() / 2, a.len() - 1] {
            let mut changed = a.to_vec();
            changed[at] ^= 0x01;
            assert_ne!(fingerprint(a), fingerprint(&changed), "byte {at}");
        }
    }
}
