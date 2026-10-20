use std::rc::Rc;

use wave_insight_lib::data_struct::Signal;
use wave_insight_lib::waveform::TimeRange;

use yew::prelude::*;

use crate::code_reader::CodeReader;
use crate::lsp::LspBridge;
use crate::nav::{
    DesignState,
    Goto,
    PathPicker,
    SharedDesign,
    SharedWaveform,
    SidebarTree,
    SignalInspector,
    SourceClick};
use crate::wave_show::WaveShow;
use crate::module_struct::ModuleStruct;

use crate::top_bar::TopBar;

use crate::file_load::FileLoad;

#[cfg(feature = "tauri")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "tauri")]
use serde_wasm_bindgen::{to_value, from_value};
#[cfg(feature = "tauri")]
use wasm_bindgen::prelude::*;
#[cfg(feature = "tauri")]
use wasm_bindgen_futures::spawn_local;
#[cfg(feature = "tauri")]
use crate::file_load::{invoke, listen};

pub enum Msg {
    #[cfg(feature = "tauri")]
    FileDrop(Vec<String>),

    NavIconClick,
    /// A signal row was clicked in the sidebar; its hierarchical path.
    SidebarAdd(String),
    /// A dump was opened: the handle values are decoded from, plus its time
    /// range so the ruler and the fit-to-trace zoom can be drawn.
    GetWaveform(SharedWaveform),
    /// One source file was read; it goes to the design database.
    GetVerilog((String, String)),
    /// A source tab was closed; the file leaves the source pane.
    CloseSource(usize),
    /// A file could not be opened, or the design could not be parsed.
    Error(String),
    /// A plain click on an identifier in the source editor.
    SourceClick(SourceClick),
    /// The wave view selected (or cleared) a signal.
    WaveSelect(Option<String>),
    /// The wave view asked where a signal is declared.
    WaveJump(String),
    /// Bring one source position into view.
    Goto((String, u32, u32)),
    /// Several signals cover the clicked position; the user chose one.
    PickPath(usize),
    CancelPick,

    ///the sidebar splitter was grabbed
    SplitterDown(MouseEvent),
    ///the mouse moved while dragging the splitter
    SplitterMove(MouseEvent),
    SplitterUp,
}

///width of the sidebar when nothing else was chosen
const SIDEBAR_DEFAULT: f64 = 280.0;
const SIDEBAR_MIN: f64 = 180.0;

fn sidebar_clamp(width: f64) -> f64 {
    let max = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .map(|w| (w * 0.45).max(SIDEBAR_MIN + 40.0))
        .unwrap_or(640.0);
    width.max(SIDEBAR_MIN).min(max)
}

/// Candidates waiting for the user to pick one.
struct Picker {
    title: String,
    paths: Vec<String>,
}

pub struct App {
    drawer_state: bool,
    /// The sidebar tree: the dump hierarchy once a waveform is open, the
    /// elaborated design otherwise.
    tree: Option<SidebarTree>,
    /// The opened dump.
    waveform: Option<SharedWaveform>,
    /// Start and end of the trace; the ruler and the fit zoom use it.
    time_range: Option<TimeRange>,
    /// Parsed sources plus the elaborated design.
    design: SharedDesign,
    /// The language server client the source editor talks to.
    lsp: LspBridge,
    verilog_source: Vec<(String,String)>,
    signal_add: (String,Rc<Signal>),
    /// Bumped for every signal the user adds, so the wave view can tell "the
    /// same signal was added again" from "nothing happened" (and does not
    /// resurrect a row the user deleted on the next re-render).
    signal_add_seq: u32,
    /// Hierarchical path of the signal selected in the wave view.
    selected: Option<String>,
    picker: Option<Picker>,
    /// Last jump request handed to the source pane.
    goto: Option<Goto>,
    goto_token: u32,
    /// One line of feedback for the top bar.
    status: Option<String>,
    sidebar_width: f64,
    ///(mouse x, sidebar width) while the splitter is dragged
    splitter_start: Option<(f64, f64)>,
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    #[allow(unused_variables)] // `ctx` is only used by the tauri/server transports
    fn create(ctx: &Context<Self>) -> Self {

        #[cfg(feature = "tauri")]
        let file_drop_callback = ctx.link().callback(Msg::FileDrop);

        #[cfg(feature = "tauri")]
        spawn_local(async move {
            #[derive(Debug, Serialize, Deserialize)]
            #[allow(non_snake_case)] // tauri's event envelope
            struct TauriEvent {
                event: String,
                windowLabel: String,
                payload: Vec<String>,
                id: f64,
            }
            let closure = Closure::<dyn FnMut(JsValue)>::new(move |x: JsValue| {
                let tauri_event: TauriEvent = serde_wasm_bindgen::from_value(x).unwrap();//TODO:do not unwrap
                //console::log_1(&format!("callback: {:?}", tauri_event).into());
                file_drop_callback.emit(tauri_event.payload);
            });
            let ret = listen("tauri://file-drop", &closure).await;
            closure.forget();
        });

        // The language server client runs the same code the desktop transport
        // pumps, but against its own parse of the same sources; navigation
        // falls back to `design` whenever it cannot answer.
        let design = SharedDesign::new(DesignState::new());
        let lsp = LspBridge::new(
            design.clone(),
            ctx.link()
                .callback(|goto: Goto| Msg::Goto((goto.file, goto.line, goto.col))),
        );
        Self {
            drawer_state: true,
            tree: None,
            waveform: None,
            time_range: None,
            design,
            lsp,
            verilog_source: vec![],
            signal_add: ("".to_string(),Rc::new(Signal::new())),
            signal_add_seq: 0,
            selected: None,
            picker: None,
            goto: None,
            goto_token: 0,
            status: None,
            sidebar_width: SIDEBAR_DEFAULT,
            splitter_start: None,
        }
    }

    #[allow(unused_variables)] // `ctx` is only used by the tauri/server transports
    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            #[cfg(feature = "tauri")]
            Msg::FileDrop(path) => {
                #[derive(Serialize, Deserialize)]
                struct GetFileListArgs {
                    path: String,
                }
                path.into_iter().for_each(|p| {
                    let lower = p.to_ascii_lowercase();
                    if lower.ends_with(".vcd") || lower.ends_with(".fst") || lower.ends_with(".ghw") {
                        let args = to_value(&GetFileListArgs { path: p }).unwrap();
                        let onwaveform = ctx.link().callback(Msg::GetWaveform);
                        let onerror = ctx.link().callback(Msg::Error);
                        spawn_local(async move {
                            //the command returns the file bytes; the webview
                            //parses them exactly like a browser upload
                            let bytes: Vec<u8> = from_value(invoke("choose_vcd_absolute", args).await)
                                .unwrap_or_default();
                            if bytes.is_empty() {
                                onerror.emit("the dump could not be read".to_string());
                                return;
                            }
                            match wave_insight_lib::waveform::Waveform::open_bytes(bytes) {
                                Ok(waveform) => onwaveform.emit(SharedWaveform::new(waveform)),
                                Err(error) => onerror.emit(error.message()),
                            }
                        });
                    } else if lower.ends_with(".v") || lower.ends_with(".sv") {
                        let args = to_value(&GetFileListArgs { path: p.clone() }).unwrap();
                        let onverilog = ctx.link().callback(Msg::GetVerilog);
                        let onerror = ctx.link().callback(Msg::Error);
                        spawn_local(async move {
                            let text: String =
                                from_value(invoke("choose_verilog_absolute", args).await).unwrap_or_default();
                            if text.is_empty() {
                                onerror.emit(format!("{p}: the source could not be read"));
                                return;
                            }
                            onverilog.emit((p, text));
                        });
                    };
                });
                true
            }

            Msg::NavIconClick => {
                self.drawer_state = !self.drawer_state;
                true
            }
            Msg::SplitterDown(e) => {
                e.prevent_default();
                if e.detail() == 2 {
                    //double click restores the default width
                    self.sidebar_width = SIDEBAR_DEFAULT;
                    self.splitter_start = None;
                }else {
                    self.splitter_start = Some((e.x() as f64, self.sidebar_width));
                }
                true
            }
            Msg::SplitterMove(e) => {
                if let Some((start_x, start_width)) = self.splitter_start {
                    self.sidebar_width = sidebar_clamp(start_width + (e.x() as f64 - start_x));
                }
                true
            }
            Msg::SplitterUp => {
                self.splitter_start = None;
                true
            }
            Msg::SidebarAdd(path) => {
                self.add_path(&path);
                true
            }
            Msg::GetWaveform(waveform) => {
                // the dump knows which module it was made from; that is the best
                // guess for the design top
                let top_hint = waveform
                    .as_waveform()
                    .hierarchy()
                    .root()
                    .children()
                    .first()
                    .map(|scope| scope.name().to_string());
                self.design.borrow_mut().set_top_hint(top_hint);
                // cheap: for VCD this scans the body text, it does not parse it
                self.time_range = waveform.as_waveform().time_range_hint().ok().flatten();
                self.waveform = Some(waveform);
                self.refresh_tree();
                self.sync_lsp_top();
                self.status = Some("waveform loaded".to_string());
                true
            }
            Msg::GetVerilog((file, text)) => {
                self.design.borrow_mut().add_source(&file, &text);
                match self.verilog_source.iter_mut().find(|(name,_)| name == &file) {
                    Some(entry) => entry.1 = text.clone(),
                    None => self.verilog_source.push((file.clone(), text.clone())),
                }
                // every loaded source is an open LSP document, not only the
                // visible tab, so a definition can point into any of them
                self.lsp.open(&file, &text);
                self.refresh_tree();
                self.sync_lsp_top();
                self.status = Some(format!("parsed {file}"));
                true
            }
            Msg::CloseSource(index) => {
                if let Some((file, _text)) = self
                    .verilog_source
                    .get(index)
                    .map(|(file, text)| (file.clone(), text.clone()))
                {
                    self.verilog_source.remove(index);
                    // the buffer goes away; the source stays in the design and
                    // in the language server's database, so declaration targets
                    // in the closed file keep resolving
                    self.lsp.close(&file);
                    self.status = Some(format!("closed {file}"));
                }
                true
            }
            Msg::Error(message) => {
                self.status = Some(message);
                true
            }
            Msg::SourceClick(click) => {
                let candidates = self.candidates_at(&click);
                match candidates.len() {
                    0 => {
                        self.status = Some(format!(
                            "`{}` is not a signal of the elaborated design", click.word));
                    }
                    1 => {
                        self.add_path(&candidates[0]);
                    }
                    _ => {
                        self.status = Some(format!(
                            "{} signals match `{}`", candidates.len(), click.word));
                        self.picker = Some(Picker {
                            title: format!("{} signals match `{}`", candidates.len(), click.word),
                            paths: candidates,
                        });
                    }
                }
                true
            }
            Msg::WaveSelect(path) => {
                self.selected = path;
                true
            }
            Msg::WaveJump(path) => {
                match self.declare_target(&path) {
                    Some((file, line, col)) => {
                        self.goto_to(file, line, col);
                    }
                    None => {
                        self.status = Some(format!(
                            "no declaration for {path} — load the .v/.sv sources it came from"));
                    }
                }
                true
            }
            Msg::Goto((file, line, col)) => {
                self.goto_to(file, line, col);
                true
            }
            Msg::PickPath(index) => {
                if let Some(picker) = self.picker.take() {
                    if let Some(path) = picker.paths.get(index).cloned() {
                        self.add_path(&path);
                    }
                }
                true
            }
            Msg::CancelPick => {
                self.picker = None;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let design_version = self.design.borrow().version();

        html! {
            <div class="app" style={format!("--sidebar-w:{:.0}px", self.sidebar_width)}>
                <TopBar onnavigationiconclick={link.callback(|_| Msg::NavIconClick)}
                    status={self.status.clone()} />
                <div class="app-body">
                    {if self.drawer_state {
                        html!{
                        <aside class="sidebar">
                            {self.file_button(ctx)}
                            <ModuleStruct tree={self.tree.clone()}
                                signaladd={link.callback(|(_name, path): (String, String)| Msg::SidebarAdd(path))}/>
                        </aside>
                        }
                    }else {
                        html!{}
                    }}
                    {if self.drawer_state {
                        html!{
                            <div class={if self.splitter_start.is_some() {"splitter dragging"} else {"splitter"}}
                                title="Drag to resize, double click to reset"
                                onmousedown={link.callback(Msg::SplitterDown)} />
                        }
                    }else {
                        html!{}
                    }}
                    <main class="main-pane">
                        <div class="code-pane">
                            <CodeReader
                                file={self.verilog_source.clone()}
                                goto={self.goto.clone()}
                                lsp={self.lsp.clone()}
                                onclose={link.callback(Msg::CloseSource)}
                                onwordclick={link.callback(Msg::SourceClick)} />
                            <SignalInspector
                                design={self.design.clone()}
                                version={design_version}
                                path={self.selected.clone()}
                                ongoto={link.callback(Msg::Goto)} />
                        </div>
                        <WaveShow
                            signaladd={(self.signal_add.0.clone(),Rc::clone(&self.signal_add.1))}
                            added_seq={self.signal_add_seq}
                            waveform={self.waveform.clone()}
                            time_range={self.time_range}
                            onselect={link.callback(Msg::WaveSelect)}
                            onjump={link.callback(Msg::WaveJump)} />
                    </main>
                </div>
                {if let Some(picker) = &self.picker {
                    html!{
                        <PathPicker title={picker.title.clone()}
                            paths={picker.paths.clone()}
                            onchoose={link.callback(Msg::PickPath)}
                            oncancel={link.callback(|_| Msg::CancelPick)} />
                    }
                }else {
                    html!{}
                }}
                {if self.splitter_start.is_some() {
                    html!{
                        <div class="splitter-shield"
                            onmousemove={link.callback(Msg::SplitterMove)}
                            onmouseup={link.callback(|_| Msg::SplitterUp)}
                            //releasing the button outside the window never fires mouseup
                            onmouseout={link.callback(|_| Msg::SplitterUp)} />
                    }
                }else {
                    html!{}
                }}
            </div>
        }
    }
}

impl App {
    /// Rebuild the sidebar tree: the dump hierarchy wins, the design is the
    /// fallback so the module list is usable before a dump is loaded.
    fn refresh_tree(&mut self) {
        self.tree = match &self.waveform {
            Some(waveform) => Some(SidebarTree::from_hierarchy(
                waveform.as_waveform().hierarchy(),
            )),
            None => self.design.borrow().sidebar_tree(),
        };
    }

    /// Tell the language server which module to elaborate, so its hover,
    /// definition and outline answers describe the same hierarchy the sidebar
    /// and the inspector show.
    fn sync_lsp_top(&self) {
        let top = {
            let state = self.design.borrow();
            state.top().map(str::to_owned)
        };
        self.lsp.set_top(top.as_deref());
    }

    /// Where `path` is declared, when the design knows it.
    fn declare_target(&self, path: &str) -> Option<(String, u32, u32)> {
        self.design
            .borrow()
            .declaration(path)
            .map(|span| (span.file.display().to_string(), span.start_line, span.start_col))
    }

    /// Ask the source pane to show one position.
    fn goto_to(&mut self, file: String, line: u32, col: u32) {
        if line == 0 {
            return;
        }
        if !self.verilog_source.iter().any(|(name, _)| name == &file) {
            self.status = Some(format!("{file} is not open in the source pane"));
            return;
        }
        self.goto_token = self.goto_token.wrapping_add(1);
        self.status = Some(format!("{file}:{line}:{col}"));
        self.goto = Some(Goto { file, line, col, token: self.goto_token });
    }

    /// Build the legacy signal row for a hierarchical path.
    fn signal_for_path(&self, path: &str) -> Signal {
        let mut signal = Signal::new();
        signal.value_key = path.to_string();
        signal.size = 1;
        if let Some(flat) = self.design.borrow().flat_signal(path) {
            signal.size = flat.width.unwrap_or(1).max(1) as usize;
            signal.location_define = wave_insight_lib::data_struct::CodeLocation {
                file_name: flat.declaration.file.display().to_string(),
                line: flat.declaration.start_line,
            };
            signal.drive = flat.drivers.iter().map(|driver| driver.span.to_string()).collect();
            signal.load = flat.loads.iter().map(|load| load.span.to_string()).collect();
        }
        // The dump is what gets drawn, so its width wins when the two disagree
        // (a truncated vector would otherwise be formatted at the declared
        // width, which the legacy formatters cannot do).
        if let Some(info) = self
            .waveform
            .as_ref()
            .and_then(|waveform| waveform.signal_by_path(path))
        {
            signal.size = info.width().max(1) as usize;
        }
        signal
    }

    /// Add one hierarchical path to the wave view and, when the design knows
    /// it and its file is open, show where it is declared.
    fn add_path(&mut self, path: &str) {
        let signal = self.signal_for_path(path);
        self.signal_add = (path.to_string(), Rc::new(signal));
        self.signal_add_seq = self.signal_add_seq.wrapping_add(1);
        self.selected = Some(path.to_string());
        self.status = Some(format!("added {path} to the waveform"));
        // only jump when the source pane can show the file: a closed tab would
        // otherwise turn a successful add into an "is not open" message
        if let Some((file, line, col)) = self.declare_target(path) {
            if self.verilog_source.iter().any(|(name, _)| name == &file) {
                self.goto_to(file, line, col);
            }
        }
    }

    /// Every signal path a click at `click` may refer to.
    ///
    /// The design database answers exactly (a position can belong to one
    /// module body instantiated several times, so the answer can be a list).
    /// Without a matching design, the dump hierarchy is searched by local name,
    /// which keeps navigation useful when only the VCD is loaded.
    fn candidates_at(&self, click: &SourceClick) -> Vec<String> {
        let mut found = {
            let design = self.design.borrow();
            let exact = design.signals_at(&click.file, click.line, click.col);
            if exact.is_empty() {
                design.signals_matching(&click.word)
            }else {
                exact
            }
        };
        if found.is_empty() {
            if let Some(waveform) = &self.waveform {
                found = waveform
                    .as_waveform()
                    .signals()
                    .iter()
                    .filter(|info| info.name() == click.word)
                    .map(|info| info.path().to_string())
                    .collect();
            }
        }
        found
    }

    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <FileLoad
                ongetwaveform={ctx.link().callback(Msg::GetWaveform)}
                ongetverilog={ctx.link().callback(Msg::GetVerilog)}
                onerror={ctx.link().callback(Msg::Error)} />
        }
    }
    
}
