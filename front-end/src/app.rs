use std::rc::Rc;

#[cfg(feature = "backend")]
use std::cell::RefCell;

use wave_insight_lib::data_struct::{
    Module,
    Signal};

#[cfg(feature = "backend")]
use wave_insight_lib::data_struct::ModuleValue;

use yew::prelude::*;
#[cfg(feature = "server")]
use web_sys::console;//TODO:for debug

use crate::code_reader::CodeReader;
use crate::lsp::LspBridge;
use crate::nav::{
    DesignState,
    Goto,
    PathPicker,
    SharedDesign,
    SharedWaveform,
    SignalInspector,
    SourceClick};
use crate::wave_show::WaveShow;
use crate::module_struct::ModuleStruct;

use crate::top_bar::TopBar;

#[cfg(feature = "backend")]
use web_sys::{MessageEvent, WebSocket};
#[cfg(feature = "backend")]
use wasm_bindgen::{prelude::Closure, JsCast};

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
    /// A signal was picked in the sidebar; the wave view adds it.
    SignalAdd((String,Rc<Signal>)),
    /// A dump was opened: its hierarchy and the handle values are read from.
    #[cfg(feature = "wasm")]
    GetWaveform((Module, SharedWaveform)),
    /// The tauri/server transports hand over a module tree instead.
    #[cfg(feature = "backend")]
    GetModule(Module),
    /// One source file was read; it goes to the design database.
    GetVerilog((String, String)),
    /// A source tab was closed; the file leaves the source pane.
    CloseSource(usize),
    /// A file could not be opened, or the design could not be parsed.
    #[cfg(feature = "wasm")]
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
    #[cfg(feature = "tauri")]
    GetValueFromTauri(String),
    #[cfg(feature = "tauri")]
    AddValueFromTauri(Option<(String, (Vec<i32>, Vec<(u8, u8)>))>),
    #[cfg(feature = "server")]
    WsSend(String),
    #[cfg(feature = "server")]
    GetWebsocket(String),

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
    module: Rc<Module>,
    waveform_module: Option<Rc<Module>>,
    design_module: Option<Rc<Module>>,
    /// The opened dump.
    waveform: Option<SharedWaveform>,
    end_clock: i32,
    /// Parsed sources plus the elaborated design.
    design: SharedDesign,
    /// The language server client the source editor talks to.
    lsp: LspBridge,
    verilog_source: Vec<(String,String)>,
    signal_add: (String,Rc<Signal>),
    /// Hierarchical path of the signal selected in the wave view.
    selected: Option<String>,
    picker: Option<Picker>,
    /// Last jump request handed to the source pane.
    goto: Option<Goto>,
    goto_token: u32,
    /// One line of feedback for the top bar.
    status: Option<String>,
    #[cfg(feature = "backend")]
    signal_value: Rc<RefCell<ModuleValue>>,//do not re-render when change, so RefCell is ok
    #[cfg(feature = "backend")]
    signal_value_raw: ModuleValue,//TODO:not a good implement
    #[cfg(feature = "server")]
    websocket: Option<WebSocket>,
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

        #[cfg(feature = "server")]
        let websocket = create_websocket(ctx);
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
            module: Rc::new(Module::new()),
            waveform_module: None,
            design_module: None,
            waveform: None,
            end_clock: 0,
            design,
            lsp,
            verilog_source: vec![],
            signal_add: ("".to_string(),Rc::new(Signal::new())),
            selected: None,
            picker: None,
            goto: None,
            goto_token: 0,
            status: None,
            #[cfg(feature = "backend")]
            signal_value: Rc::new(RefCell::new(ModuleValue::new())),
            #[cfg(feature = "backend")]
            signal_value_raw: ModuleValue::new(),
            #[cfg(feature = "server")]
            websocket: Some(websocket),
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
                    if p.strip_suffix(".vcd").is_some() {
                        let args = to_value(&GetFileListArgs { path: p }).unwrap();
                        let link = ctx.link().callback(Msg::GetModule);
                        spawn_local(async move {
                            let ret: Module = from_value(invoke("choose_vcd_absolute", args).await).unwrap();
                            //console::log_1(&format!("{:?}", ret).into());
                            link.emit(ret);
                        });
                    }else if p.strip_suffix(".v").is_some() {
                        let args = to_value(&GetFileListArgs { path: p.clone() }).unwrap();
                        let link = ctx.link().callback(Msg::GetModule);
                        let link_verilog = ctx.link().callback(Msg::GetVerilog);
                        spawn_local(async move {
                            let ret: (String, Module) = from_value(invoke("choose_verilog_absolute", args).await).unwrap();
                            //console::log_1(&format!("{:?}", ret).into());
                            link.emit(ret.1);
                            link_verilog.emit((p.clone(), ret.0));
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
            Msg::SignalAdd((name, signal)) => {
                #[cfg(feature = "server")]
                ctx.link().callback(Msg::WsSend).emit(format!("s:{}",signal.value_key));
                #[cfg(feature = "tauri")]
                ctx.link().callback(Msg::GetValueFromTauri).emit(signal.value_key.clone());
                let display = if signal.value_key.contains('.') {
                    signal.value_key.clone()
                }else {
                    name
                };
                // a row from the design tree knows where it is declared
                let declare = self.declare_target(&signal.value_key);
                self.signal_add = (display, signal);
                self.selected = Some(self.signal_add.1.value_key.clone());
                if let Some((file, line, col)) = declare {
                    self.goto_to(file, line, col);
                }
                true
            }
            #[cfg(feature = "wasm")]
            Msg::GetWaveform((module, waveform)) => {
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
                self.end_clock = module.end_clock;
                self.waveform_module = Some(Rc::new(module));
                self.waveform = Some(waveform);
                self.refresh_module();
                self.sync_lsp_top();
                self.status = Some("waveform loaded".to_string());
                true
            }
            #[cfg(feature = "backend")]
            Msg::GetModule(m) => {
                self.module = Rc::new(m);
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
                self.refresh_module();
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
            #[cfg(feature = "wasm")]
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
            #[cfg(feature = "tauri")]
            Msg::GetValueFromTauri(key) => {
                let args = to_value(&TauriArgs { key }).unwrap();
                let link = ctx.link().callback(Msg::AddValueFromTauri);
                spawn_local(async move {
                    let ret = from_value(invoke("get_value", args).await).unwrap();
                    //console::log_1(&format!("{:?}", ret).into());
                    link.emit(ret);
                });
                false
            }
            #[cfg(feature = "tauri")]
            Msg::AddValueFromTauri(x) => {
                match x {
                    Some((key, value)) => {
                        self.signal_value.borrow_mut().value.insert(key, value);
                    },
                    None => {},
                };
                false
            }
            #[cfg(feature = "server")]
            Msg::WsSend(e) => {
                match &mut self.websocket {
                    Some(ws) => {
                       if ws.send_with_str(&e).is_ok() {};
                    },
                    None => {},
                }
                true
            }
            #[cfg(feature = "server")]
            Msg::GetWebsocket(m) => {
                if let Some(module_string) = m.strip_prefix("module:") {
                    let module: Module = serde_json::from_str(module_string).unwrap();//TODO:do not unwrap
                    self.module = Rc::new(module);
                }else if let Some(signal_string) = m.strip_prefix("sig:") {
                    if let Some((key, value)) = signal_string.split_once(29u8 as char) {
                        let value_parse: (Vec<i32>, Vec<(u8, u8)>) = serde_json::from_str(value).unwrap();//TODO:do not unwrap
                        self.signal_value_raw.value.insert(key.to_string(), value_parse);
                        self.signal_value = Rc::new(RefCell::new(self.signal_value_raw.clone()));
                        //TODO:value update here will cause an adition signal add on wave show
                    }
                }
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let design_version = self.design.borrow().version();

        // the tauri/server transports still feed a pre-expanded value tree
        let legacy_values = {
            #[cfg(feature = "backend")]
            {
                Some(Rc::clone(&self.signal_value))
            }
            #[cfg(not(feature = "backend"))]
            {
                None
            }
        };

        html! {
            <div class="app" style={format!("--sidebar-w:{:.0}px", self.sidebar_width)}>
                <TopBar onnavigationiconclick={link.callback(|_| Msg::NavIconClick)}
                    status={self.status.clone()} />
                <div class="app-body">
                    {if self.drawer_state {
                        html!{
                        <aside class="sidebar">
                            {self.file_button(ctx)}
                            <ModuleStruct module={Rc::clone(&self.module)} signaladd={link.callback(Msg::SignalAdd)}/>
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
                            waveform={self.waveform.clone()}
                            signal_value={legacy_values}
                            end_clock={self.end_clock}
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
    /// fallback so the module list is usable before a VCD is loaded.
    fn refresh_module(&mut self) {
        self.design_module = self.design.borrow().module_tree().map(Rc::new);
        self.module = match (&self.waveform_module, &self.design_module) {
            (Some(waveform), _) => Rc::clone(waveform),
            (None, Some(design)) => Rc::clone(design),
            (None, None) => Rc::new(Module::new()),
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
        }else if let Some(info) = self
            .waveform
            .as_ref()
            .and_then(|waveform| waveform.signal_by_path(path))
        {
            signal.size = info.width().max(1) as usize;
        }
        signal
    }

    /// Add one hierarchical path to the wave view.
    fn add_path(&mut self, path: &str) {
        let signal = self.signal_for_path(path);
        self.signal_add = (path.to_string(), Rc::new(signal));
        self.selected = Some(path.to_string());
        self.status = Some(format!("added {path} to the waveform"));
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

    #[cfg(feature = "wasm")]
    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <FileLoad
                ongetwaveform={ctx.link().callback(Msg::GetWaveform)}
                ongetverilog={ctx.link().callback(Msg::GetVerilog)}
                onerror={ctx.link().callback(Msg::Error)} />
        }
    }
    #[cfg(feature = "tauri")]
    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <FileLoad
                ongetmodule={ctx.link().callback(Msg::GetModule)}
                ongetverilog={ctx.link().callback(Msg::GetVerilog)} />
        }
    }
    #[cfg(feature = "server")]
    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <button onclick={ctx.link().callback(|_| Msg::WsSend("file".to_string()))}>{"ws"}</button>
        }
    }
    
}

#[cfg(feature = "server")]
fn create_websocket(ctx: &Context<App>) -> WebSocket {
    let callback: Callback<String> = ctx.link().callback(Msg::GetWebsocket);

    let window = web_sys::window().expect("should have a window in this context");
    let addr = window.location().hostname().unwrap();

    let ws = WebSocket::new(&format!("ws://{}:2993",addr)).unwrap();//TODO:when not connect

    let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
        if let Ok(txt) = e.data().dyn_into::<js_sys::JsString>() {
            console::log_1(&format!("message event, received Text: {:?}", txt).into());
            callback.emit(txt.as_string().unwrap())
        } else {
            console::log_1(&format!("message event, received Unknown: {:?}", e.data()).into());
        }
    }) as Box<dyn FnMut(MessageEvent)>);
    ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
    onmessage_callback.forget();
    ws
}

#[cfg(feature = "tauri")]
#[derive(Serialize, Deserialize)]
struct TauriArgs {
    key: String,
}
