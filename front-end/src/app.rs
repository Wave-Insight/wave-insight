use std::cell::Cell;

use wave_insight_lib::data_struct::Signal;

use yew::prelude::*;

use crate::code_reader::CodeReader;
use crate::lsp::LspBridge;
use crate::nav::{
    match_dump_candidates,
    DesignState,
    Goto,
    PathPicker,
    SharedDesign,
    SidebarTree,
    SignalInspector,
    SourceClick};
use crate::state::{self, Action, Cursors, DumpHandle, Layout, RowsSlice, Status, Store, Subscription};
use crate::wave_show::WaveShow;
use crate::module_struct::ModuleStruct;

use crate::top_bar::TopBar;

use crate::file_load::FileLoad;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

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

    /// One typed change for the store; everything the app *does* is an action.
    Action(Action),
    /// The store's layout slice changed (the app subscribes to it).
    Layout(Layout),
    /// `.main-pane` was measured; the editor height is derived from it.
    MainMeasured(f64),
    /// The window was resized: measure again.
    Resize,
    /// A signal row was clicked in the sidebar; its hierarchical path.
    SidebarAdd(String),
    /// A dump was opened: the handle values are decoded from, plus its time
    /// range so the ruler and the fit-to-trace zoom can be drawn.
    GetWaveform(DumpHandle),
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
    /// The settings dialog asked for a driver/load: `(load?, index, path)`.
    Locate((bool, usize, String)),
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
    /// the editor/wave splitter was grabbed
    WaveSplitterDown(MouseEvent),
    /// the mouse moved while dragging that splitter
    WaveSplitterMove(MouseEvent),
    WaveSplitterUp,
}

// The locked layout constants (docs/state-store.md §3.4). The QA gate asserts
// exactly these numbers.
/// Height of the tab strip: the collapsed editor is exactly this tall.
const TAB_STRIP: f64 = 32.0;
/// Expanded editor content, minimum.
const EDITOR_CONTENT_MIN: f64 = 120.0;
/// Height of one wave row (kept in sync with `--row-h`).
const ROW_HEIGHT: f64 = 30.0;
/// Rows the wave pane must keep, whatever the ratio says.
const WAVE_MIN_ROWS: f64 = 4.0;
/// The time ruler above the rows: it is part of the wave pane too.
const TIMEBAR: f64 = 30.0;
/// Height of `.wave-splitter` (kept in sync with `--wave-splitter-h`).
const WAVE_SPLITTER: f64 = 5.0;
/// Upper bound of the inspector dock's height (`max-height` in styles.css).
const INSPECTOR_MAX: f64 = 190.0;

/// The widest the sidebar may get for the current window.
fn sidebar_max() -> f64 {
    web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .map(|w| (w * 0.45).max(state::types::SIDEBAR_MIN + 40.0))
        .unwrap_or(640.0)
}

/// Candidates waiting for the user to pick one.
struct Picker {
    title: String,
    paths: Vec<String>,
}

pub struct App {
    /// The one place the app's state lives (R2 step 1: layout + status).
    store: Store,
    /// The layout slice, mirrored here so `view` needs no borrow.
    layout: Layout,
    /// Kept so the subscription lives exactly as long as the component.
    _layout_sub: Subscription,
    /// The app div: the splitter writes `--sidebar-w` here during a drag
    /// without going through Yew (design doc §4.1).
    root: NodeRef,
    /// Bumped by every `update` that returns `true`; published for the QA gate
    /// as `__waveDebug.render_count` (the *shell* re-render count, R2 step 1).
    render_count: Cell<u32>,
    /// The main pane, measured so the stored editor ratio can become pixels.
    main: NodeRef,
    /// Height of `.main-pane` in css pixels, `0` until the first measurement.
    main_h: f64,
    /// `window.addEventListener("resize")`: unlike `window.onresize` (owned by
    /// the wave pane) this one does not steal another component's listener.
    resize_listener: Option<Closure<dyn FnMut()>>,
    /// The sidebar tree: the dump hierarchy once a waveform is open, the
    /// elaborated design otherwise.
    tree: Option<SidebarTree>,
    /// Parsed sources plus the elaborated design.
    design: SharedDesign,
    /// The language server client the source editor talks to.
    lsp: LspBridge,
    /// Cursor/dump subscriptions for the editor inlay hints; they update the
    /// page-wide hint context without rendering anything.
    _cursors_sub: Subscription,
    _rows_sub: Subscription,
    verilog_source: Vec<(String,String)>,
    /// Hierarchical path of the signal selected in the wave view.
    selected: Option<String>,
    picker: Option<Picker>,
    /// Last jump request handed to the source pane.
    goto: Option<Goto>,
    goto_token: u32,
    ///(mouse x, sidebar width) while the splitter is dragged
    splitter_start: Option<(f64, f64)>,
    /// Width written straight to the DOM while the splitter is being dragged;
    /// committed to the store on mouseup.
    sidebar_preview: Option<f64>,
    ///(mouse y, editor height) while the editor/wave splitter is dragged
    wave_splitter_start: Option<(f64, f64)>,
    /// Height written straight to the DOM while that splitter is dragged;
    /// committed to the store as a ratio on mouseup.
    editor_preview: Option<f64>,
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
        // R2: the store is created here and owns the layout and the status line
        // from now on; the panes are still wired with props (later steps).
        let store = Store::new();
        let layout = store.select(state::select::layout);
        let layout_sub = store.subscribe(
            state::select::layout,
            ctx.link().callback(Msg::Layout),
        );
        // The editor's inlay hints show the value at cursor 1; the context is
        // page state (the provider runs inside a monaco callback), so it is
        // updated from subscriptions and never triggers a Yew render.
        crate::lsp::inlay::init(design.clone(), store.clone());
        let cursors_sub = store.subscribe(
            state::select::cursors,
            Callback::from(|cursors: Cursors| {
                crate::lsp::inlay::set_cursor(cursors.one.map(i64::from))
            }),
        );
        let rows_sub = store.subscribe(
            state::select::rows,
            Callback::from(|_rows: RowsSlice| crate::lsp::inlay::refresh()),
        );
        let mut this = Self {
            store,
            layout,
            _layout_sub: layout_sub,
            root: NodeRef::default(),
            render_count: Cell::new(0),
            main: NodeRef::default(),
            main_h: 0.0,
            resize_listener: None,
            tree: None,
            design,
            lsp,
            _cursors_sub: cursors_sub,
            _rows_sub: rows_sub,
            verilog_source: vec![],
            selected: None,
            picker: None,
            goto: None,
            goto_token: 0,
            splitter_start: None,
            sidebar_preview: None,
            wave_splitter_start: None,
            editor_preview: None,
        };
        this.attach_window_listeners(ctx);
        this
    }

    #[allow(unused_variables)] // `ctx` is only used by the tauri/server transports
    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let changed = self.handle(ctx, msg);
        // R2 render_count: how many times the *shell* re-rendered. Leaf
        // components have their own subscriptions and do not bump this; keep it
        // that way, the splitter assertion is written against it.
        if changed {
            self.render_count.set(self.render_count.get().wrapping_add(1));
        }
        changed
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        self.publish_render_count();
        // R2 step 2: the editor height is a pixel value derived from the measured
        // main pane, so keep the measurement fresh
        let measured = self.measure_main();
        if measured > 1.0 && (measured - self.main_h).abs() > 0.5 {
            ctx.link().send_message(Msg::MainMeasured(measured));
        }
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let (Some(window), Some(listener)) = (web_sys::window(), self.resize_listener.take()) {
            let _ = window.remove_event_listener_with_callback(
                "resize",
                listener.as_ref().unchecked_ref(),
            );
        }
    }


    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let design_version = self.design.borrow().version();
        // while the splitter is dragged the preview is authoritative (it was
        // written straight to the DOM); otherwise the store's layout is
        let sidebar_w = self.sidebar_preview.unwrap_or(self.layout.sidebar_w);
        let empty = self.dump().is_none() && self.verilog_source.is_empty() && self.tree.is_none();
        // R2 step 2/4: with no sources the editor collapses to the tab strip and
        // the wave pane takes the rest (design doc §3.1, constants §3.4)
        let collapsed = self.verilog_source.is_empty();
        let inspector_visible = self.inspector_visible();
        let editor_h = self
            .editor_preview
            .unwrap_or_else(|| self.editor_height(collapsed, inspector_visible));

        html! {
            <div class="app" ref={self.root.clone()} style={format!("--sidebar-w:{sidebar_w:.0}px; --editor-h:{editor_h:.0}px")}>
                <TopBar store={self.store.clone()}
                    onnavigationiconclick={link.callback(|_| Msg::Action(Action::ToggleDrawer))} />
                <div class="app-body">
                    {if self.layout.drawer_open {
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
                    {if self.layout.drawer_open {
                        html!{
                            <div class={if self.splitter_start.is_some() {"splitter dragging"} else {"splitter"}}
                                title="Drag to resize, double click to reset"
                                onmousedown={link.callback(Msg::SplitterDown)} />
                        }
                    }else {
                        html!{}
                    }}
                    <main class="main-pane" ref={self.main.clone()}>
                        {if empty {
                            // R2 polish: with nothing open the panes used to show
                            // an empty tree and an empty wave area; say what to do.
                            html!{
                                <div class="start-hint">
                                    <div class="empty-hint">
                                        <div><strong>{"Nothing loaded yet"}</strong></div>
                                        <div>{"Drop a waveform ( "}
                                            <kbd>{".vcd"}</kbd>{" "}<kbd>{".fst"}</kbd>{" "}<kbd>{".ghw"}</kbd>
                                            {" ) to inspect its signals."}</div>
                                        <div>{"Drop your "}<kbd>{".sv"}</kbd>{" / "}<kbd>{".v"}</kbd>
                                            {" sources to get declarations, drivers and loads."}</div>
                                        <div>{"Use the ＋ button in the sidebar, or drag the files onto it."}</div>
                                    </div>
                                </div>
                            }
                        }else {
                            html!{}
                        }}
                        <div class={if collapsed {"code-pane collapsed"} else {"code-pane"}}>
                            <CodeReader
                                file={self.verilog_source.clone()}
                                goto={self.goto.clone()}
                                lsp={self.lsp.clone()}
                                onclose={link.callback(Msg::CloseSource)}
                                onwordclick={link.callback(Msg::SourceClick)} />
                        </div>
                        {
                            // R2 step 4: the inspector is its own dock between the
                            // editor and the wave pane, outside the collapsing part
                            if inspector_visible {
                                html!{
                                    <div class="inspector">
                                        <SignalInspector
                                            design={self.design.clone()}
                                            version={design_version}
                                            path={self.selected.clone()}
                                            ongoto={link.callback(Msg::Goto)} />
                                    </div>
                                }
                            }else {
                                html!{}
                            }
                        }
                        <div class={if self.wave_splitter_start.is_some() {"wave-splitter dragging"} else {"wave-splitter"}}
                            title="Drag to resize, double click to reset"
                            onmousedown={link.callback(Msg::WaveSplitterDown)} />
                        <WaveShow
                            store={self.store.clone()}
                            onselect={link.callback(Msg::WaveSelect)}
                            onjump={link.callback(Msg::WaveJump)}
                            onlocate={link.callback(Msg::Locate)} />
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
                }else if self.wave_splitter_start.is_some() {
                    html!{
                        <div class="splitter-shield row"
                            onmousemove={link.callback(Msg::WaveSplitterMove)}
                            onmouseup={link.callback(|_| Msg::WaveSplitterUp)}
                            //releasing the button outside the window never fires mouseup
                            onmouseout={link.callback(|_| Msg::WaveSplitterUp)} />
                    }
                }else {
                    html!{}
                }}
            </div>
        }
    }
}

impl App {
    /// Height of `.main-pane` in css pixels (`0` before the first paint).
    fn measure_main(&self) -> f64 {
        self.main
            .cast::<web_sys::Element>()
            .map(|element| element.get_bounding_client_rect().height())
            .unwrap_or(0.0)
    }

    /// Pixel height of the source pane, derived from the measured main pane and
    /// the stored ratio (design doc §3.2, constants §3.4):
    ///
    /// * collapsed: the tab strip only, no editor content;
    /// * expanded: `ratio * main`, never below 120 px of content, and the wave
    ///   pane keeps at least four rows — the row minimum wins over the ratio, so
    ///   the ratio is trimmed here and never the rows.
    fn editor_height(&self, collapsed: bool, inspector_visible: bool) -> f64 {
        if collapsed {
            return TAB_STRIP;
        }
        if self.main_h <= 1.0 {
            // nothing measured yet: a sane expanded height for the first paint
            return 2.0 * (TAB_STRIP + EDITOR_CONTENT_MIN);
        }
        let (min, max) = self.editor_bounds(inspector_visible);
        // The inspector dock sits in the same column, between the editor and the
        // wave pane, so the stored ratio applies to the space the dock leaves.
        // Applied to the whole pane it handed the dock's pixels to the editor and
        // the wave pane dropped to the four-row minimum as soon as the sources
        // (and with them the dock) opened.
        let dock = if inspector_visible { INSPECTOR_MAX } else { 0.0 };
        let share = (self.main_h - dock).max(min);
        (share * f64::from(self.layout.editor_ratio)).clamp(min, max)
    }

    /// The editor heights the splitter may not leave: the expanded content is at
    /// least 120 px, and the wave pane keeps its four rows plus the ruler and the
    /// dock (the row minimum wins, so the ratio is trimmed).
    fn editor_bounds(&self, inspector_visible: bool) -> (f64, f64) {
        let min = TAB_STRIP + EDITOR_CONTENT_MIN;
        let inspector = if inspector_visible { INSPECTOR_MAX } else { 0.0 };
        let wave = WAVE_MIN_ROWS * ROW_HEIGHT + TIMEBAR;
        let max = (self.main_h - wave - WAVE_SPLITTER - inspector).max(min);
        (min, max)
    }

    /// The inspector dock is visible when it has something to say: a loaded
    /// design, or a selected signal (design doc §3.1, `inspector_visible`).
    fn inspector_visible(&self) -> bool {
        !self.design.borrow().is_empty() || self.selected.is_some()
    }

    /// `window.addEventListener("resize")`, kept out of `window.onresize` so the
    /// wave pane's own resize listener keeps working.
    fn attach_window_listeners(&mut self, ctx: &Context<Self>) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let link = ctx.link().clone();
        let listener = Closure::<dyn FnMut()>::new(move || {
            link.send_message(Msg::Resize);
        });
        if window
            .add_event_listener_with_callback("resize", listener.as_ref().unchecked_ref())
            .is_ok()
        {
            self.resize_listener = Some(listener);
        }
    }

    #[allow(unused_variables)] // `ctx` is only used by the tauri/server transports
    fn handle(&mut self, ctx: &Context<Self>, msg: Msg) -> bool {
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
                                Ok(waveform) => onwaveform.emit(DumpHandle::new(waveform)),
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

            Msg::Action(action) => {
                // A dispatch never re-renders by itself: subscribers only enqueue
                // Yew messages, so the shell only renders if *its* slice moved.
                let effects = self.store.dispatch(action);
                debug_assert!(effects.is_empty(), "R2 step 1 has no effects yet");
                false
            }
            Msg::Layout(layout) => {
                self.layout = layout;
                true
            }
            Msg::MainMeasured(height) => {
                if (height - self.main_h).abs() > 0.5 {
                    self.main_h = height;
                    // the derived --editor-h changed
                    true
                } else {
                    false
                }
            }
            Msg::Resize => {
                let height = self.measure_main();
                if height > 1.0 && (height - self.main_h).abs() > 0.5 {
                    self.main_h = height;
                    true
                } else {
                    false
                }
            }
            Msg::SplitterDown(e) => {
                e.prevent_default();
                if e.detail() == 2 {
                    //double click restores the default width
                    self.splitter_start = None;
                    self.sidebar_preview = None;
                    self.store.dispatch(Action::ResizeSidebar {
                        width: state::types::SIDEBAR_DEFAULT,
                        max: sidebar_max(),
                    });
                    false
                }else {
                    self.splitter_start = Some((e.x() as f64, self.layout.sidebar_w));
                    // the dragged class and the shield are part of the render
                    true
                }
            }
            Msg::SplitterMove(e) => {
                if let Some((start_x, start_width)) = self.splitter_start {
                    let width = state::types::clamp_sidebar(
                        start_width + (e.x() as f64 - start_x),
                        sidebar_max(),
                    );
                    // Tier A (design doc §4.1): write the CSS variable straight
                    // to the DOM and return `false`, so a 60 Hz drag never diffs
                    // a single node. The store is told once, on mouseup.
                    self.sidebar_preview = Some(width);
                    let editor_h = self.current_editor_height();
                    self.write_pane_variables(width, editor_h);
                }
                false
            }
            Msg::SplitterUp => {
                self.splitter_start = None;
                match self.sidebar_preview.take() {
                    Some(width) => {
                        // the commit goes through the store; the layout
                        // subscription re-renders the shell once, so returning
                        // `false` here keeps the commit to a single render
                        self.store.dispatch(Action::ResizeSidebar { width, max: sidebar_max() });
                        false
                    }
                    // a click without a drag still has to remove the shield
                    None => true,
                }
            }
            Msg::WaveSplitterDown(e) => {
                e.prevent_default();
                if e.detail() == 2 {
                    // double click restores the default editor ratio
                    self.wave_splitter_start = None;
                    self.editor_preview = None;
                    self.store
                        .dispatch(Action::SetEditorRatio(Layout::default().editor_ratio));
                    false
                } else {
                    let current = self.current_editor_height();
                    self.wave_splitter_start = Some((e.y() as f64, current));
                    // the dragged class and the shield are part of the render
                    true
                }
            }
            Msg::WaveSplitterMove(e) => {
                if let Some((start_y, start_height)) = self.wave_splitter_start {
                    let (low, high) = self.editor_bounds(self.inspector_visible());
                    let height = (start_height + (e.y() as f64 - start_y)).clamp(low, high);
                    // Tier A: the drag follows the pointer through one CSS
                    // variable; the store is told once, as a ratio, on mouseup.
                    self.editor_preview = Some(height);
                    self.write_pane_variables(self.layout.sidebar_w, height);
                }
                false
            }
            Msg::WaveSplitterUp => {
                let preview = self.editor_preview.take();
                self.wave_splitter_start = None;
                match preview {
                    Some(height) => {
                        // the store keeps the *ratio*; the pixel height stays
                        // derived from the measured pane (design doc 3.2)
                        let ratio = (height / self.main_h.max(1.0)).clamp(0.0, 1.0) as f32;
                        self.store.dispatch(Action::SetEditorRatio(ratio));
                        false
                    }
                    // a click without a drag still has to remove the shield
                    None => true,
                }
            }
            Msg::SidebarAdd(path) => {
                // the sidebar is the explicit "add and show me where it is"
                // gesture, unlike a click in the source
                self.add_path(&path, true);
                true
            }
            Msg::GetWaveform(waveform) => {
                // the dump knows which module it was made from; that is the best
                // guess for the design top
                let top_hint = waveform
                    .waveform()
                    .hierarchy()
                    .root()
                    .children()
                    .first()
                    .map(|scope| scope.name().to_string());
                self.design.borrow_mut().set_top_hint(top_hint);
                // the store owns the handle and derives the time range from it
                // (that is what the ruler and the fit zoom read)
                self.store
                    .dispatch(Action::DumpOpened { waveform: waveform.clone() });
                // the inlay hints can only show values once a dump is open
                crate::lsp::inlay::set_waveform(Some(waveform));
                self.refresh_tree();
                self.sync_lsp_top();
                self.note(Status::info("waveform loaded"));
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
                // names that resolved to nothing before may resolve now
                crate::lsp::inlay::refresh();
                self.note(Status::info(format!("parsed {file}")));
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
                    self.note(Status::info(format!("closed {file}")));
                }
                true
            }
            Msg::Error(message) => {
                self.note(Status::error(message));
                true
            }
            Msg::SourceClick(click) => {
                let candidates = self.candidates_at(&click);
                match candidates.len() {
                    0 => {
                        self.note(Status::info(format!(
                            "`{}` is not a signal of the elaborated design", click.word)));
                    }
                    1 => {
                        // Verdi behaviour: clicking a name in the source adds it
                        // to the wave view, and does not move the editor
                        self.add_path(&candidates[0], false);
                    }
                    _ => {
                        self.note(Status::info(format!(
                            "{} signals match `{}`", candidates.len(), click.word)));
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
                self.jump_to_declaration(&path);
                true
            }
            Msg::Locate((load, index, path)) => {
                self.selected = Some(path.clone());
                self.locate_reference(&path, load, index);
                true
            }
            Msg::Goto((file, line, col)) => {
                self.goto_to(file, line, col);
                true
            }
            Msg::PickPath(index) => {
                if let Some(picker) = self.picker.take() {
                    if let Some(path) = picker.paths.get(index).cloned() {
                        // the picker was opened by a click in the source: same
                        // rule, add without moving the editor
                        self.add_path(&path, false);
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

    /// One line of feedback for the top bar (through the store, so `TopBar`
    /// re-renders on its own subscription instead of the whole shell).
    fn note(&self, status: Status) {
        self.store.dispatch(Action::Status(status));
    }

    /// The editor height the shell currently shows: the live drag preview while
    /// the editor/wave splitter moves, the derived height otherwise.
    fn current_editor_height(&self) -> f64 {
        self.editor_preview.unwrap_or_else(|| {
            self.editor_height(self.verilog_source.is_empty(), self.inspector_visible())
        })
    }

    /// Write the two pane variables straight to the DOM while a splitter is
    /// dragged: no Yew render, no virtual DOM diff (design doc §4.1). The values
    /// are committed to the store on mouseup.
    fn write_pane_variables(&self, sidebar_w: f64, editor_h: f64) {
        let Some(app) = self.root.cast::<web_sys::Element>() else {
            return;
        };
        // the inline style of the app div is exactly these two variables, so
        // rewriting the attribute is equivalent to setProperty() on them and
        // needs no extra web-sys feature
        let _ = app.set_attribute(
            "style",
            &format!("--sidebar-w:{sidebar_w:.0}px; --editor-h:{editor_h:.0}px"),
        );
    }

    /// Publish the shell re-render counter for the QA gate.
    ///
    /// Two channels on purpose: `__waveRenderCount` survives a canvas redraw
    /// (`wave_show::canvas::publish_debug` copies it into its object), while the
    /// best-effort patch into the existing `__waveDebug` makes the field appear
    /// before the first redraw.
    fn publish_render_count(&self) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let count = wasm_bindgen::JsValue::from_f64(f64::from(self.render_count.get()));
        let _ = js_sys::Reflect::set(
            &window,
            &wasm_bindgen::JsValue::from_str("__waveRenderCount"),
            &count,
        );
        if let Ok(debug) = js_sys::Reflect::get(
            &window,
            &wasm_bindgen::JsValue::from_str("__waveDebug"),
        ) {
            if debug.is_object() {
                let _ = js_sys::Reflect::set(
                    &debug,
                    &wasm_bindgen::JsValue::from_str("render_count"),
                    &count,
                );
            }
        }
    }

    /// Rebuild the sidebar tree: the dump hierarchy wins, the design is the
    /// fallback so the module list is usable before a dump is loaded.
    fn refresh_tree(&mut self) {
        self.tree = match self.dump() {
            Some(waveform) => Some(SidebarTree::from_hierarchy(
                waveform.waveform().hierarchy(),
            )),
            None => self.design.borrow().sidebar_tree(),
        };
    }

    /// The opened dump, read from the store (the store owns the only handle).
    fn dump(&self) -> Option<DumpHandle> {
        self.store.select(state::select::dump_waveform)
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
            self.note(Status::error(format!(
                "{file} is not open in the source pane — open it from the file picker again")));
            return;
        }
        self.goto_token = self.goto_token.wrapping_add(1);
        self.note(Status::info(format!("{file}:{line}:{col}")));
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
        if let Some(width) = self
            .dump()
            .and_then(|waveform| waveform.waveform().signal_by_path(path).map(|info| info.width()))
        {
            signal.size = width.max(1) as usize;
        }
        signal
    }

    /// Add one signal to the wave view.
    ///
    /// `path` can be a *design* path (a click in the source, a picker choice) or
    /// a *dump* path (the sidebar tree). The row must carry the dump path, because
    /// the wave view looks the decoded values up by it; when the two hierarchies
    /// spell their root scope differently the dump side is resolved by
    /// [`match_dump_candidates`], and an ambiguous name opens the picker instead
    /// of guessing.
    ///
    /// `show_declaration` is for the explicit "add and show me where it is"
    /// gestures (the sidebar). A click in the source adds the row and leaves the
    /// editor where it is, which is the Verdi behaviour the user asked for.
    fn add_path(&mut self, path: &str, show_declaration: bool) {
        let mut dump_candidates = Vec::new();
        if let Some(waveform) = self.dump() {
            dump_candidates = match_dump_candidates(waveform.waveform(), path);
            match dump_candidates.len() {
                0 => {
                    // the dump is the source of values: adding a row that cannot
                    // decode is exactly the "the added signal draws nothing" bug
                    self.note(Status::error(format!(
                        "`{path}` is not in the loaded waveform"
                    )));
                    return;
                }
                1 => {}
                _ => {
                    self.note(Status::info(format!(
                        "{} signals in the waveform match `{path}`",
                        dump_candidates.len()
                    )));
                    self.picker = Some(Picker {
                        title: format!(
                            "{} signals in the waveform match `{path}`",
                            dump_candidates.len()
                        ),
                        paths: dump_candidates,
                    });
                    return;
                }
            }
        }
        let row_path = dump_candidates
            .first()
            .cloned()
            .unwrap_or_else(|| path.to_string());

        // the row list is store state now: resolve what the reducer cannot know
        // (width from the dump/design, driver + load locations) and dispatch
        let signal = self.signal_for_path(&row_path);
        self.store.dispatch(Action::RowAdded {
            path: row_path.clone(),
            name: row_path.clone(),
            width: signal.size,
            load: signal.load.clone(),
            drive: signal.drive.clone(),
        });
        self.selected = Some(row_path.clone());
        self.note(Status::info(format!("added {row_path} to the waveform")));
        if show_declaration {
            // only jump when the source pane can show the file: a closed tab
            // would otherwise turn a successful add into an error status
            if let Some((file, line, col)) = self.declare_target(&row_path) {
                if self.verilog_source.iter().any(|(name, _)| name == &file) {
                    self.goto_to(file, line, col);
                }
            }
        }
    }

    /// Navigate the source pane to where `path` is declared.
    ///
    /// The lookup is fuzzy on purpose (the dump root and the elaboration root can
    /// disagree). An ambiguous path is reported with its candidates; a design that
    /// cannot answer says why (no sources, no elaborated top, or a genuinely
    /// missing declaration) instead of pretending the declaration does not exist.
    fn jump_to_declaration(&mut self, path: &str) {
        if let Some((file, line, col)) = self.declare_target(path) {
            self.goto_to(file, line, col);
            return;
        }
        let state = self.design.borrow();
        if state.is_empty() {
            self.note(Status::error(format!(
                "no sources loaded — `{path}` is declared in the .v/.sv files; load them to navigate"
            )));
            return;
        }
        let candidates = state.resolve_candidates(path);
        if candidates.len() > 1 {
            let names: Vec<&str> = candidates
                .iter()
                .take(4)
                .map(|signal| signal.path.as_str())
                .collect();
            let more = if candidates.len() > names.len() { ", ..." } else { "" };
            self.note(Status::error(format!(
                "ambiguous: {} signals match `{path}` — {}{more}",
                candidates.len(),
                names.join(", ")
            )));
            return;
        }
        match state.top() {
            Some(top) => self.note(Status::error(format!(
                "the loaded sources do not declare `{path}` (elaborated top: {top})"
            ))),
            None => self.note(Status::error(
                "the loaded sources do not elaborate to a top module, so declarations cannot be resolved".to_string(),
            )),
        }
    }

    /// Jump to a driver or a load of `path` (the settings dialog's buttons).
    ///
    /// One reference navigates straight to it. Several are listed in the
    /// Drivers / loads inspector — the panel the row selection already drives —
    /// and the requested one is opened, so the button never silently picks a
    /// reference while pretending there is only one.
    fn locate_reference(&mut self, path: &str, load: bool, index: usize) {
        let kind = if load { "load" } else { "driver" };
        let (targets, has_signal) = {
            let design = self.design.borrow();
            let signal = design.resolve_path(path);
            let targets = signal
                .map(|signal| {
                    let spans = if load { &signal.loads } else { &signal.drivers };
                    spans
                        .iter()
                        .map(|usage| {
                            (
                                usage.span.file.display().to_string(),
                                usage.span.start_line,
                                usage.span.start_col,
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            (targets, signal.is_some())
        };
        if !has_signal {
            let message = if self.design.borrow().is_empty() {
                format!("no sources loaded — load the .v/.sv files to find the {kind}s of `{path}`")
            } else {
                format!("the loaded sources do not contain `{path}` — cannot find its {kind}s")
            };
            self.note(Status::error(message));
            return;
        }
        let count = targets.len();
        if count == 0 {
            self.note(Status::error(format!(
                "no {kind} found for `{path}` in the loaded sources"
            )));
            return;
        }
        let chosen = index.min(count - 1);
        let (file, line, col) = targets[chosen].clone();
        if !self.verilog_source.iter().any(|(name, _)| name == &file) {
            self.note(Status::error(format!(
                "{file} is not open in the source pane — open it from the file picker again"
            )));
            return;
        }
        self.goto_to(file, line, col);
        if count > 1 {
            self.note(Status::info(format!(
                "{count} {kind}s for `{path}` — opened #{}; all of them are listed in Drivers / loads",
                chosen + 1
            )));
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
            if let Some(waveform) = self.dump() {
                found = waveform
                    .waveform()
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
