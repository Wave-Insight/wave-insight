use std::cell::Cell;

use yew::prelude::*;

use crate::code_reader::CodeReader;
use crate::lsp::LspBridge;
use crate::module_struct::ModuleStruct;
use crate::nav::{match_dump_candidates, PathPicker, SidebarTree, SignalInspector, SourceClick};
use crate::state::{
    self, Action, Cursors, DumpHandle, DumpId, DumpsTabsSlice, Effect, Focus, Goto, Layout, Picker,
    RowsSlice, SourcesSlice, Split, Status, Store, Subscription,
};
use crate::wave_show::{SplitPane, WaveShow};

use crate::top_bar::TopBar;

use crate::file_load::{FileLoad, LoadReport};

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

#[cfg(feature = "tauri")]
use crate::file_load::{current_webview, invoke};
#[cfg(feature = "tauri")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "tauri")]
use serde_wasm_bindgen::{from_value, to_value};
#[cfg(feature = "tauri")]
use wasm_bindgen::prelude::*;
#[cfg(feature = "tauri")]
use wasm_bindgen_futures::spawn_local;

pub enum Msg {
    #[cfg(feature = "tauri")]
    FileDrop(Vec<String>),

    /// One typed change for the store; everything the app *does* is an action.
    Action(Action),
    /// The store's layout slice changed (the app subscribes to it).
    Layout(Layout),
    /// The store's sources slice changed (the app subscribes to it).
    Sources(SourcesSlice),
    /// The store's focus slice changed (the app subscribes to it).
    Focus(Focus),
    /// The store's pending jump request changed (the app subscribes to it).
    GotoChanged(Option<Goto>),
    /// The store's path picker changed (the app subscribes to it).
    PickerChanged(Option<Picker>),
    /// The store's sidebar tree changed (the app subscribes to it).
    Tree(Option<SidebarTree>),
    /// The dump tab list or the active tab changed (tab-only snapshot: a
    /// viewport/cursor commit never reaches the shell).
    DumpTabs(DumpsTabsSlice),
    /// A dump tab was clicked: it becomes the active dump.
    DumpTabClicked(usize),
    /// A dump tab was closed.
    DumpClose(usize),
    /// A key press inside the dump tab strip (Ctrl+W closes the active dump).
    DumpTabsKey(KeyboardEvent),
    /// The store's split slice changed (open/closed, right dump, linked).
    SplitChanged(Option<Split>),
    /// The split toggle button.
    SplitToggled,
    /// The right pane should show this dump index.
    SplitRight(usize),
    /// Linked time axis toggled.
    SplitLinkedToggled,
    /// A module-tree row was clicked: the sidebar selection moved.
    InstanceSelected(String),
    /// The editor buffer changed; the store re-syncs the document and the LSP.
    SourceEdited((String, String)),
    /// A source tab was clicked.
    TabActivated(usize),
    /// The editor applied a jump request; the store clears the pending one.
    JumpHandled(u32),
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
    /// A line of progress or a note from the file loader (info tone).
    Note(String),
    /// What the last successful parse cost; the success line carries it.
    LoadReport(LoadReport),
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
    /// The editor/wave splitter was grabbed
    WaveSplitterDown(MouseEvent),
    /// The mouse moved while dragging that splitter
    WaveSplitterMove(MouseEvent),
    WaveSplitterUp,
    /// A tab of the wave frame was chosen (the waveform or the load/drive view).
    WaveFrameTab(WaveFrameTab),
}

/// Which view the wave frame shows.
///
/// The driver/load view used to be a dock between the editor and the waveform,
/// so opening the sources shrank the waveform by the height of a panel the user
/// had not asked for. Both views now share the frame and are switched by a tab
/// strip, like Verdi's waveform window and its signal info pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveFrameTab {
    Waveform,
    LoadDrive,
}

impl WaveFrameTab {
    fn class(self, active: WaveFrameTab) -> &'static str {
        if self == active {
            "dock-tab active"
        } else {
            "dock-tab"
        }
    }
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
/// Height of the tab strip that switches the wave frame between the waveform
/// and the load/drive view (kept in sync with `--dock-tabs-h` in styles.css).
const DOCK_TABS: f64 = 28.0;

/// The widest the sidebar may get for the current window.
fn sidebar_max() -> f64 {
    web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .map(|w| (w * 0.45).max(state::types::SIDEBAR_MIN + 40.0))
        .unwrap_or(640.0)
}

/// What the shell resolved about one signal before it dispatches `RowAdded`:
/// the width the dump/design reports and the driver/load locations.
struct ResolvedSignal {
    size: usize,
    load: Vec<String>,
    drive: Vec<String>,
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
    /// The sidebar tree, mirrored from the store (the reducer derives it from
    /// the dump hierarchy or the elaborated design).
    tree: Option<SidebarTree>,
    /// Kept so the subscription lives exactly as long as the component.
    _tree_sub: Subscription,
    /// The dump tab strip: labels, sizes and the active index. The entries
    /// themselves (rows/viewport/focus) stay in the store and never re-render
    /// the shell.
    dump_tabs: DumpsTabsSlice,
    _dumps_sub: Subscription,
    /// The split view (right pane dump + linked flag), mirrored from the store.
    split: Option<Split>,
    _split_sub: Subscription,
    /// The language server client the source editor talks to.
    lsp: LspBridge,
    /// Cursor/dump subscriptions for the editor inlay hints; they update the
    /// page-wide hint context without rendering anything.
    _cursors_sub: Subscription,
    _rows_sub: Subscription,
    /// The open sources and the active tab, mirrored from the store.
    sources: SourcesSlice,
    /// Kept so the subscriptions live exactly as long as the component.
    _sources_sub: Subscription,
    _focus_sub: Subscription,
    _goto_sub: Subscription,
    /// The inspector selection, mirrored from the store.
    focus: Focus,
    /// The path picker, mirrored from the store (the store owns whether it is
    /// open; the app still performs the choice, which needs the dump/design).
    picker: Option<Picker>,
    /// Kept so the subscription lives exactly as long as the component.
    _picker_sub: Subscription,
    /// The pending jump request, mirrored from the store.
    goto: Option<Goto>,
    ///(mouse x, sidebar width) while the splitter is dragged
    splitter_start: Option<(f64, f64)>,
    /// Width written straight to the DOM while the splitter is being dragged;
    /// committed to the store on mouseup.
    sidebar_preview: Option<f64>,
    ///(mouse y, editor height) while the editor/wave splitter is dragged
    wave_splitter_start: Option<(f64, f64)>,
    /// Live height of the editor while the splitter is dragged (tier A).
    editor_preview: Option<f64>,
    /// Which view the wave frame shows; the waveform is the default.
    wave_tab: WaveFrameTab,
    /// The report the file loader emitted for the load that is about to arrive
    /// in Msg::GetWaveform; consumed there to extend the success line.
    pending_load: Option<LoadReport>,
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    #[allow(unused_variables)] // `ctx` is only used by the tauri/server transports
    fn create(ctx: &Context<Self>) -> Self {
        #[cfg(feature = "tauri")]
        let file_drop_callback = ctx.link().callback(Msg::FileDrop);

        // Tauri 2 removed the `tauri://file-drop` event: the webview owns the
        // drag-drop API now. `onDragDropEvent` fires for enter/over/drop/leave
        // and only the drop carries the absolute paths, so the existing
        // Msg::FileDrop(Vec<String>) path is unchanged.
        #[cfg(feature = "tauri")]
        {
            let webview = current_webview();
            let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
                let payload = match js_sys::Reflect::get(&event, &JsValue::from_str("payload")) {
                    Ok(payload) => payload,
                    Err(_) => return,
                };
                if payload.is_undefined() {
                    return;
                }
                let kind = js_sys::Reflect::get(&payload, &JsValue::from_str("type"))
                    .ok()
                    .and_then(|kind| kind.as_string());
                if kind.as_deref() != Some("drop") {
                    return;
                }
                let paths: Vec<String> =
                    js_sys::Reflect::get(&payload, &JsValue::from_str("paths"))
                        .ok()
                        .and_then(|paths| paths.dyn_into::<js_sys::Array>().ok())
                        .map(|paths| paths.iter().filter_map(|path| path.as_string()).collect())
                        .unwrap_or_default();
                if !paths.is_empty() {
                    file_drop_callback.emit(paths);
                }
            });
            // the handler lives for the page, like the old event closure did
            let registered = js_sys::Reflect::get(&webview, &JsValue::from_str("onDragDropEvent"))
                .ok()
                .and_then(|function| function.dyn_into::<js_sys::Function>().ok())
                .and_then(|function| {
                    function
                        .call1(&webview, handler.as_ref().unchecked_ref())
                        .ok()
                })
                .is_some();
            if registered {
                handler.forget();
            }
        }

        // R2: the store is created here and owns the layout and the status line
        // from now on; the panes are still wired with props (later steps). It is
        // created before the language server client because that client reports
        // the diagnostic count straight into the store.
        //
        // Wave 3: the store restores the layout and the per-signal display
        // settings from the last session and writes committed changes back (see
        // `state::persist`). `browser_persister` is `None` when the browser
        // denies storage (private mode), which simply means "no persistence".
        let store = Store::with_persister(state::persist::browser_persister());
        // A restored sidebar width may be wider than this window allows (the
        // window was larger last time). Clamp it before the shell reads the
        // slice, so the first render already has a usable width.
        let restored = store.select(state::select::layout);
        let max = sidebar_max();
        if restored.sidebar_w > max {
            let _ = store.dispatch(Action::ResizeSidebar {
                width: restored.sidebar_w,
                max,
            });
        }
        // R2 step 3b: the design database lives in the store; the bridge reads
        // its fallback answers through it (the store is created first on purpose)
        let lsp = LspBridge::new(
            store.clone(),
            ctx.link()
                .callback(|goto: Goto| Msg::Goto((goto.file, goto.line, goto.col))),
            {
                let store = store.clone();
                Callback::from(move |problems: u32| {
                    let _ = store.dispatch(Action::Problems(problems));
                })
            },
        );
        let layout = store.select(state::select::layout);
        let layout_sub = store.subscribe(state::select::layout, ctx.link().callback(Msg::Layout));
        // The editor's inlay hints show the value at cursor 1; the context is
        // page state (the provider runs inside a monaco callback), so it is
        // updated from subscriptions and never triggers a Yew render.
        crate::lsp::inlay::init(store.clone());
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
        // R2 step 3: the store owns the open sources, the inspector focus and the
        // jump request; the shell mirrors the slices it renders with.
        let sources = store.select(state::select::sources);
        let sources_sub =
            store.subscribe(state::select::sources, ctx.link().callback(Msg::Sources));
        let focus = store.select(state::select::focus);
        let focus_sub = store.subscribe(state::select::focus, ctx.link().callback(Msg::Focus));
        let goto = store.select(state::select::goto);
        let goto_sub = store.subscribe(state::select::goto, ctx.link().callback(Msg::GotoChanged));
        let picker = store.select(state::select::picker);
        let picker_sub = store.subscribe(
            state::select::picker,
            ctx.link().callback(Msg::PickerChanged),
        );
        // the sidebar tree is derived by the reducer (dump hierarchy or design)
        let tree = store.select(state::select::tree);
        let tree_sub = store.subscribe(state::select::tree, ctx.link().callback(Msg::Tree));
        let dump_tabs = store.select(state::select::dump_tabs);
        let dumps_sub =
            store.subscribe(state::select::dump_tabs, ctx.link().callback(Msg::DumpTabs));
        let split = store.select(state::select::split);
        let split_sub =
            store.subscribe(state::select::split, ctx.link().callback(Msg::SplitChanged));
        let mut this = Self {
            store,
            layout,
            _layout_sub: layout_sub,
            root: NodeRef::default(),
            render_count: Cell::new(0),
            main: NodeRef::default(),
            main_h: 0.0,
            resize_listener: None,
            tree,
            _tree_sub: tree_sub,
            dump_tabs,
            _dumps_sub: dumps_sub,
            split,
            _split_sub: split_sub,
            lsp,
            _cursors_sub: cursors_sub,
            _rows_sub: rows_sub,
            sources,
            _sources_sub: sources_sub,
            _focus_sub: focus_sub,
            _goto_sub: goto_sub,
            focus,
            picker,
            _picker_sub: picker_sub,
            goto,
            splitter_start: None,
            sidebar_preview: None,
            wave_splitter_start: None,
            editor_preview: None,
            wave_tab: WaveFrameTab::Waveform,
            pending_load: None,
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
            self.render_count
                .set(self.render_count.get().wrapping_add(1));
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
            let _ = window
                .remove_event_listener_with_callback("resize", listener.as_ref().unchecked_ref());
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let design_version = self.store.select(state::select::design_version);
        // while the splitter is dragged the preview is authoritative (it was
        // written straight to the DOM); otherwise the store's layout is
        let sidebar_w = self.sidebar_preview.unwrap_or(self.layout.sidebar_w);
        let empty = self.dump().is_none() && self.sources.is_empty() && self.tree.is_none();
        // R2 step 2/4: with no sources the editor collapses to the tab strip and
        // the wave pane takes the rest (design doc §3.1, constants §3.4)
        let collapsed = self.sources.is_empty();
        let editor_h = self
            .editor_preview
            .unwrap_or_else(|| self.editor_height(collapsed));

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
                                focus={self.focus.clone()}
                                oninstance={link.callback(Msg::InstanceSelected)}
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
                                sources={self.sources.clone()}
                                goto={self.goto.clone()}
                                lsp={self.lsp.clone()}
                                ontab={link.callback(Msg::TabActivated)}
                                onclose={link.callback(Msg::CloseSource)}
                                onshown={link.callback(Msg::JumpHandled)}
                                onchange={link.callback(Msg::SourceEdited)}
                                onwordclick={link.callback(Msg::SourceClick)} />
                        </div>
                        <div class={if self.wave_splitter_start.is_some() {"wave-splitter dragging"} else {"wave-splitter"}}
                            title="Drag to resize, double click to reset"
                            onmousedown={link.callback(Msg::WaveSplitterDown)} />
                        // The waveform and the driver/load view share one frame and
                        // are switched by a tab strip. Both stay mounted (the
                        // hidden one with `display:none`), so switching back does
                        // not lose the zoom, the scroll position or the decode
                        // cache of the waveform.
                        <div class="wave-frame">
                            // The dump tab strip: one tab per open dump, shown
                            // whenever at least one dump is open (so the last tab
                            // can always be closed; the strip disappears with it and
                            // the start hint comes back). A single-dump session now
                            // has this one-tab strip above the dock tabs.
                            {if !self.dump_tabs.items.is_empty() {
                                let active = self.dump_tabs.active_index();
                                let total_mb = self.dump_tabs.total_bytes / (1024 * 1024);
                                let count = self.dump_tabs.len();
                                html!{
                                <div class="dump-tabs" role="tablist" aria-label="Open dumps"
                                    title={format!("{count} dumps open, {total_mb} MB of dump files")}
                                    onkeydown={link.callback(Msg::DumpTabsKey)}>
                                    { for self.dump_tabs.items.iter().enumerate().map(|(index, tab)| {
                                        let is_active = index == active;
                                        let label = if tab.name.is_empty() {
                                            "dump".to_string()
                                        } else {
                                            tab.name.clone()
                                        };
                                        let size = tab
                                            .bytes
                                            .map(|bytes| format!("{:.0} KB", bytes as f64 / 1024.0))
                                            .unwrap_or_else(|| "size unknown".to_string());
                                        let class = if is_active {"dump-tab active"} else {"dump-tab"};
                                        html!{
                                            <div class={class}>
                                                <button type="button" class="dump-tab-name"
                                                    role="tab"
                                                    aria-selected={if is_active {"true"} else {"false"}}
                                                    title={format!("{label} — {size}")}
                                                    onclick={link.callback(move |_| Msg::DumpTabClicked(index))}>
                                                    {label.clone()}
                                                </button>
                                                <button type="button" class="dump-tab-close"
                                                    aria-label={format!("Close {label}")}
                                                    title="Close the dump tab"
                                                    onclick={link.callback(move |event: MouseEvent| {
                                                        event.stop_propagation();
                                                        Msg::DumpClose(index)
                                                    })}>{"×"}</button>
                                            </div>
                                        }
                                    })}
                                </div>
                                }
                            }else {
                                html!{}
                            }}
                            <div class="dock-tabs">
                                <button type="button"
                                    class={WaveFrameTab::Waveform.class(self.wave_tab)}
                                    title="The waveform (cursor values, drag to zoom)"
                                    onclick={link.callback(|_| Msg::WaveFrameTab(WaveFrameTab::Waveform))}>
                                    {"Waveform"}
                                </button>
                                <button type="button"
                                    class={WaveFrameTab::LoadDrive.class(self.wave_tab)}
                                    title="Declaration, drivers and loads of the selected signal"
                                    onclick={link.callback(|_| Msg::WaveFrameTab(WaveFrameTab::LoadDrive))}>
                                    {"Drivers / loads"}
                                </button>
                            </div>
                            {if self.dump_tabs.len() >= 2 {
                                let split_open = self.split.is_some();
                                let linked = self.split.map(|split| split.linked).unwrap_or(true);
                                let right = self.split.map(|split| split.right).unwrap_or(0);
                                let active = self.dump_tabs.active_index();
                                html!{
                                <div class="split-toolbar" role="toolbar" aria-label="Split view">
                                    <button type="button"
                                        class={if split_open {"split-btn active"} else {"split-btn"}}
                                        aria-label="Split view"
                                        aria-pressed={if split_open {"true"} else {"false"}}
                                        title="Show a second dump side by side"
                                        onclick={link.callback(|_| Msg::SplitToggled)}>
                                        {"Split view"}
                                    </button>
                                    {if split_open {
                                        html!{
                                        <>
                                            { for self.dump_tabs.items.iter().enumerate()
                                                .filter(|(index, _)| *index != active)
                                                .map(|(index, tab)| {
                                                    let label = if tab.name.is_empty() {
                                                        "dump".to_string()
                                                    } else {
                                                        tab.name.clone()
                                                    };
                                                    let class = if index == right {
                                                        "split-btn split-right-choice active"
                                                    } else {
                                                        "split-btn split-right-choice"
                                                    };
                                                    html!{
                                                        <button type="button" class={class}
                                                            aria-label={format!("Show {label} on the right")}
                                                            title={format!("Show {label} on the right")}
                                                            onclick={link.callback(move |_| Msg::SplitRight(index))}>
                                                            {label}
                                                        </button>
                                                    }
                                                })}
                                            <button type="button"
                                                class={if linked {"split-btn active"} else {"split-btn"}}
                                                aria-label="Linked time axis"
                                                aria-pressed={if linked {"true"} else {"false"}}
                                                title="Show the same slice of both traces"
                                                onclick={link.callback(|_| Msg::SplitLinkedToggled)}>
                                                {"Linked"}
                                            </button>
                                            <button type="button" class="split-btn split-close"
                                                aria-label="Close split view"
                                                title="Close the second pane"
                                                onclick={link.callback(|_| Msg::SplitToggled)}>
                                                {"Close split"}
                                            </button>
                                        </>
                                        }
                                    }else {
                                        html!{}
                                    }}
                                </div>
                                }
                            }else {
                                html!{}
                            }}
                            <div class="dock-body">
                                {if let Some(split) = self.split {
                                    html!{
                                    <div class="split-view">
                                        {self.wave_pane(link)}
                                        <SplitPane
                                            key={format!("{}-{}", split.right, split.linked)}
                                            store={self.store.clone()}
                                            dump={split.right}
                                            linked={split.linked} />
                                    </div>
                                    }
                                }else {
                                    self.wave_pane(link)
                                }}
                                <SignalInspector
                                    hidden={self.wave_tab != WaveFrameTab::LoadDrive}
                                    store={self.store.clone()}
                                    version={design_version}
                                    path={self.focus.signal.clone()}
                                    ongoto={link.callback(Msg::Goto)} />
                            </div>
                        </div>
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
    fn editor_height(&self, collapsed: bool) -> f64 {
        if collapsed {
            return TAB_STRIP;
        }
        if self.main_h <= 1.0 {
            // nothing measured yet: a sane expanded height for the first paint
            return 2.0 * (TAB_STRIP + EDITOR_CONTENT_MIN);
        }
        let (min, max) = self.editor_bounds();
        (self.main_h * f64::from(self.layout.editor_ratio)).clamp(min, max)
    }

    /// The editor heights the splitter may not leave: the expanded content is at
    /// least 120 px, and the wave frame keeps its tab strip, its ruler and four
    /// rows (the row minimum wins, so the ratio is trimmed).
    fn editor_bounds(&self) -> (f64, f64) {
        let min = TAB_STRIP + EDITOR_CONTENT_MIN;
        let wave = DOCK_TABS + TIMEBAR + WAVE_MIN_ROWS * ROW_HEIGHT;
        let max = (self.main_h - wave - WAVE_SPLITTER).max(min);
        (min, max)
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
                    if lower.ends_with(".vcd") || lower.ends_with(".fst") || lower.ends_with(".ghw")
                    {
                        let args = to_value(&GetFileListArgs { path: p.clone() }).unwrap();
                        let onwaveform = ctx.link().callback(Msg::GetWaveform);
                        let onreport = ctx.link().callback(Msg::LoadReport);
                        let onnote = ctx.link().callback(Msg::Note);
                        let onerror = ctx.link().callback(Msg::Error);
                        let file = p.clone();
                        spawn_local(async move {
                            // the command returns the whole file, so this path
                            // has no slices to report: it gets the size guard,
                            // the parse line and the load report (the browser
                            // reader is the one with per-chunk progress)
                            let bytes: Vec<u8> =
                                from_value(invoke("choose_vcd_absolute", args).await)
                                    .unwrap_or_default();
                            if bytes.is_empty() {
                                onerror.emit(format!("{file}: the dump could not be read"));
                                return;
                            }
                            let size = bytes.len() as u64;
                            if let Some(message) =
                                crate::file_load::progress::guard_message(&file, size)
                            {
                                match crate::file_load::progress::size_verdict(size) {
                                    // too large: refuse instead of feeding it
                                    // to the parser and freezing the webview
                                    crate::file_load::progress::SizeVerdict::TooLarge => {
                                        onerror.emit(message);
                                        return;
                                    }
                                    crate::file_load::progress::SizeVerdict::Large => {
                                        onnote.emit(message)
                                    }
                                    crate::file_load::progress::SizeVerdict::Ok => {}
                                }
                            }
                            onnote.emit(crate::file_load::progress::parsing_status(
                                &file, size, None,
                            ));
                            // one frame so the status line paints before the
                            // synchronous parse blocks the webview
                            crate::file_load::file::next_tick().await;
                            match crate::file_load::parse_dump(&file, bytes) {
                                Ok((handle, report)) => {
                                    onreport.emit(report);
                                    onwaveform.emit(handle);
                                }
                                Err(message) => onerror.emit(message),
                            }
                        });
                    } else if lower.ends_with(".v") || lower.ends_with(".sv") {
                        let args = to_value(&GetFileListArgs { path: p.clone() }).unwrap();
                        let onverilog = ctx.link().callback(Msg::GetVerilog);
                        let onerror = ctx.link().callback(Msg::Error);
                        spawn_local(async move {
                            let text: String =
                                from_value(invoke("choose_verilog_absolute", args).await)
                                    .unwrap_or_default();
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
                // The reducer returns the outside work (LSP, editor) as effects;
                // this is the one place they are executed (docs/state-store.md §2.5).
                self.apply(action);
                false
            }
            Msg::Sources(sources) => {
                self.sources = sources;
                true
            }
            Msg::Focus(focus) => {
                self.focus = focus;
                true
            }
            Msg::GotoChanged(goto) => {
                self.goto = goto;
                true
            }
            Msg::PickerChanged(picker) => {
                self.picker = picker;
                true
            }
            Msg::Tree(tree) => {
                self.tree = tree;
                true
            }
            Msg::DumpTabs(dump_tabs) => {
                self.dump_tabs = dump_tabs;
                // the inlay hints follow the active dump (the tab slice is the
                // only thing the shell hears about; a switch changes it)
                let waveform = self.store.select(state::select::dump_waveform);
                crate::lsp::inlay::set_waveform(waveform);
                true
            }
            Msg::DumpTabClicked(index) => {
                self.apply(Action::DumpTabActivated { index });
                false
            }
            Msg::DumpClose(index) => {
                self.apply(Action::DumpClosed { index });
                false
            }
            Msg::DumpTabsKey(event) => {
                // Ctrl+W inside the dump strip closes the active dump tab (the
                // source strip keeps its own Ctrl+W; the CodeReader listener
                // ignores keys that start inside `.dump-tabs`)
                if event.ctrl_key() && matches!(event.key().as_str(), "w" | "W") {
                    event.prevent_default();
                    let index = self.dump_tabs.active_index();
                    self.apply(Action::DumpClosed { index });
                }
                false
            }
            Msg::SplitChanged(split) => {
                self.split = split;
                true
            }
            Msg::SplitToggled => {
                if self.split.is_some() {
                    self.apply(Action::SplitClosed);
                } else {
                    let right = self.split_default_right();
                    self.apply(Action::SplitOpened { right });
                }
                false
            }
            Msg::SplitRight(right) => {
                self.apply(Action::SplitRightChanged { right });
                false
            }
            Msg::SplitLinkedToggled => {
                self.apply(Action::SplitLinkedToggled);
                false
            }
            Msg::InstanceSelected(path) => {
                self.apply(Action::FocusChanged {
                    focus: Focus {
                        signal: self.focus.signal.clone(),
                        instance: Some(path),
                    },
                });
                false
            }
            Msg::SourceEdited((file, text)) => {
                // the editor owns its buffer; the store is the document of record
                // and re-syncs the LSP through the effect the reducer returns
                self.apply(Action::SourceReloaded { file, text });
                false
            }
            Msg::TabActivated(index) => {
                self.apply(Action::TabActivated { index });
                false
            }
            Msg::JumpHandled(token) => {
                self.apply(Action::JumpHandled { token });
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
                } else {
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
                        self.store.dispatch(Action::ResizeSidebar {
                            width,
                            max: sidebar_max(),
                        });
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
                    let (low, high) = self.editor_bounds();
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
            Msg::WaveFrameTab(tab) => {
                if self.wave_tab == tab {
                    return false;
                }
                self.wave_tab = tab;
                // opening the load/drive view is a read: the waveform keeps its
                // zoom and scroll, it is only hidden
                true
            }
            Msg::SidebarAdd(path) => {
                // the sidebar is the explicit "add and show me where it is"
                // gesture, unlike a click in the source
                self.add_path(&path, true);
                true
            }
            Msg::GetWaveform(waveform) => {
                // the loader adds a tab: the name and the size are the identity
                // the remembered row list is keyed by (the report is emitted
                // before the handle, so it is already here). A dump that arrived
                // as raw bytes has no name and is never remembered.
                let id = DumpId::new(
                    self.pending_load
                        .as_ref()
                        .map(|report| report.file_name.clone())
                        .unwrap_or_default(),
                    self.pending_load.as_ref().map(|report| report.bytes),
                );
                self.apply(Action::DumpAdded {
                    id: id.clone(),
                    handle: waveform.clone(),
                });
                // the memory budget may have refused the tab: keep its actionable
                // error instead of overwriting it with a load line
                let opened = self
                    .store
                    .select(|state| state.dumps.items.iter().any(|entry| entry.id.matches(&id)));
                if !opened {
                    return true;
                }
                // GHW has no cheap time-range hint: the store fell back to one
                // body scan, so say where the ruler's numbers came from instead
                // of reporting a plain load.
                let range_note = if self.store.select(|state| {
                    state
                        .dumps
                        .items
                        .get(state.dumps.active_index())
                        .is_some_and(|entry| entry.range_from_body)
                }) {
                    let format = format!("{:?}", waveform.waveform().file_format()).to_uppercase();
                    Some(format!("{format}: time range read from the body"))
                } else {
                    None
                };
                // the loader reported what the parse cost (bytes, ms, wasm
                // memory): the QA matrix still asserts the "waveform loaded"
                // prefix, so the numbers are appended to it
                let report = self.pending_load.take();
                let status = match (range_note, report) {
                    (Some(note), Some(report)) => format!("{note} — {}", report.suffix()),
                    (Some(note), None) => note,
                    (None, Some(report)) => report.status(),
                    (None, None) => "waveform loaded".to_string(),
                };
                // a remembered session may contain rows this dump does not have
                // (a signal was renamed, or the file is not quite the same one);
                // say so once, next to the load line, instead of failing the open
                let status = match self.store.select(state::select::session_restore) {
                    Some(restore) if restore.skipped > 0 => format!(
                        "{status} — {} remembered row(s) are not in this dump",
                        restore.skipped
                    ),
                    _ => status,
                };
                // with more than one tab, the status line also reports how many
                // dumps are open and how much file memory they hold
                let status = if self.dump_tabs.len() >= 2 {
                    format!(
                        "{status} — {} dumps open, {} MB",
                        self.dump_tabs.len(),
                        self.dump_tabs.total_bytes / (1024 * 1024)
                    )
                } else {
                    status
                };
                self.note(Status::info(status));
                true
            }
            Msg::GetVerilog((file, text)) => {
                // the store parses the source into the design database, rebuilds
                // the sidebar tree and asks for the LSP didOpen/didChange through
                // the effects the dispatch returns
                self.apply(Action::SourceOpened { file, text });
                // names that resolved to nothing before may resolve now
                crate::lsp::inlay::refresh();
                true
            }
            Msg::CloseSource(index) => {
                // the store drops the document and asks for the didClose; the
                // source stays in the design database, so declaration targets in
                // the closed file keep resolving
                self.apply(Action::SourceClosed { index });
                true
            }
            Msg::Error(message) => {
                // a failed open must not leave a stale report behind
                self.pending_load = None;
                self.note(Status::error(message));
                true
            }
            Msg::Note(text) => {
                // the bar subscribes to the status slice itself, so the shell
                // does not have to re-render for a progress line
                self.note(Status::info(text));
                false
            }
            Msg::LoadReport(report) => {
                self.pending_load = Some(report);
                false
            }
            Msg::SourceClick(click) => {
                let candidates = self.candidates_at(&click);
                match candidates.len() {
                    0 => {
                        self.note(Status::info(format!(
                            "`{}` is not a signal of the elaborated design",
                            click.word
                        )));
                    }
                    1 => {
                        // Verdi behaviour: clicking a name in the source adds it
                        // to the wave view, and does not move the editor
                        self.add_path(&candidates[0], false);
                    }
                    _ => {
                        self.note(Status::info(format!(
                            "{} signals match `{}`",
                            candidates.len(),
                            click.word
                        )));
                        self.apply(Action::PickerOpened {
                            title: format!("{} signals match `{}`", candidates.len(), click.word),
                            paths: candidates,
                        });
                    }
                }
                true
            }
            Msg::WaveSelect(path) => {
                self.apply(Action::FocusChanged {
                    focus: Focus {
                        signal: path,
                        instance: self.focus.instance.clone(),
                    },
                });
                false
            }
            Msg::WaveJump(path) => {
                self.jump_to_declaration(&path);
                true
            }
            Msg::Locate((load, index, path)) => {
                self.apply(Action::FocusChanged {
                    focus: Focus {
                        signal: Some(path.clone()),
                        instance: self.focus.instance.clone(),
                    },
                });
                self.locate_reference(&path, load, index);
                true
            }
            Msg::Goto((file, line, col)) => {
                // an LSP answer or the inspector asked for a position: the store
                // validates it and owns the request from here
                self.apply(Action::JumpRequested { file, line, col });
                false
            }
            Msg::PickPath(index) => {
                // the store owns the open picker; the app resolves the choice
                // (it owns the dump/design) and dispatches the row
                if let Some(path) = self
                    .picker
                    .as_ref()
                    .and_then(|picker| picker.paths.get(index))
                    .cloned()
                {
                    self.apply(Action::PickerChosen { index });
                    // the picker was opened by a click in the source: same rule,
                    // add without moving the editor
                    self.add_path(&path, false);
                }
                true
            }
            Msg::CancelPick => {
                self.apply(Action::PickerCancelled);
                true
            }
        }
    }

    /// One line of feedback for the top bar (through the store, so `TopBar`
    /// re-renders on its own subscription instead of the whole shell).
    fn note(&self, status: Status) {
        self.store.dispatch(Action::Status(status));
    }

    /// Dispatch one action and run the outside work it asked for.
    ///
    /// Every change goes through the store; the effects (LSP document sync, the
    /// top module, the editor seam) are executed here, the one place that owns
    /// the bridge (docs/state-store.md §2.5).
    fn apply(&self, action: Action) {
        let effects = self.store.dispatch(action);
        self.run_effects(effects);
    }

    /// Execute the reducer's effects through the language server bridge.
    ///
    /// The bridge's document table is a mirror of `State.sources`: keeping it in
    /// step here is what makes `window.__waveInsightLsp.stats().documents` report
    /// the open-document count the QA matrix asserts.
    fn run_effects(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::LspOpen { file, text } | Effect::LspChange { file, text } => {
                    self.lsp.open(&file, &text);
                }
                Effect::LspClose { file } => self.lsp.close(&file),
                Effect::LspTop { top } => self.lsp.set_top(top.as_deref()),
                // the jump itself rides on the `nav.goto` slice the editor
                // subscribes to; this effect is the seam a later step uses to
                // focus the editor window explicitly
                Effect::MonacoShow { .. } => {}
            }
        }
    }

    /// The editor height the shell currently shows: the live drag preview while
    /// the editor/wave splitter moves, the derived height otherwise.
    fn current_editor_height(&self) -> f64 {
        self.editor_preview
            .unwrap_or_else(|| self.editor_height(self.sources.is_empty()))
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
        if let Ok(debug) =
            js_sys::Reflect::get(&window, &wasm_bindgen::JsValue::from_str("__waveDebug"))
        {
            if debug.is_object() {
                let _ = js_sys::Reflect::set(
                    &debug,
                    &wasm_bindgen::JsValue::from_str("render_count"),
                    &count,
                );
            }
        }
    }

    /// The interactive wave pane (today's DOM); the split view puts it next to
    /// the read-only [`SplitPane`].
    fn wave_pane(&self, link: &yew::html::Scope<Self>) -> Html {
        html! {
            <WaveShow
                hidden={self.wave_tab != WaveFrameTab::Waveform}
                store={self.store.clone()}
                onselect={link.callback(Msg::WaveSelect)}
                onjump={link.callback(Msg::WaveJump)}
                onlocate={link.callback(Msg::Locate)} />
        }
    }

    /// The dump a fresh split should show on the right: the first one that is
    /// not the active dump.
    fn split_default_right(&self) -> usize {
        let active = self.dump_tabs.active_index();
        (0..self.dump_tabs.len())
            .find(|index| *index != active)
            .unwrap_or(active)
    }

    /// The opened dump, read from the store (the store owns the only handle).
    fn dump(&self) -> Option<DumpHandle> {
        self.store.select(state::select::dump_waveform)
    }

    /// Where `path` is declared, when the design knows it.
    fn declare_target(&self, path: &str) -> Option<(String, u32, u32)> {
        self.store.select(|state| {
            state.design.declaration(path).map(|span| {
                (
                    span.file.display().to_string(),
                    span.start_line,
                    span.start_col,
                )
            })
        })
    }

    /// Ask the source pane to show one position.
    ///
    /// The store validates the request (the file must be open) and owns the
    /// token; the editor follows the resulting `nav.goto` slice.
    fn goto_to(&self, file: String, line: u32, col: u32) {
        self.apply(Action::JumpRequested { file, line, col });
    }

    /// Resolve what a row needs to know about a hierarchical path: its width
    /// (the dump wins when it has the signal) and where it is driven/loaded.
    ///
    /// The reducer has no access to the dump or the design, so the shell does
    /// the lookup and hands it plain data. (This used to build the legacy
    /// `Signal` row type; that type is gone in R3.)
    fn signal_for_path(&self, path: &str) -> ResolvedSignal {
        let mut signal = ResolvedSignal {
            size: 1,
            load: Vec::new(),
            drive: Vec::new(),
        };
        if let Some((width, drive, load)) = self.store.select(|state| {
            state.design.flat_signal(path).map(|flat| {
                (
                    flat.width.unwrap_or(1).max(1) as usize,
                    flat.drivers
                        .iter()
                        .map(|driver| driver.span.to_string())
                        .collect::<Vec<_>>(),
                    flat.loads
                        .iter()
                        .map(|load| load.span.to_string())
                        .collect::<Vec<_>>(),
                )
            })
        }) {
            signal.size = width;
            signal.drive = drive;
            signal.load = load;
        }
        // The dump is what gets drawn, so its width wins when the two disagree
        // (a truncated vector would otherwise be formatted at the declared
        // width, which the formatters cannot do).
        if let Some(width) = self.dump().and_then(|waveform| {
            waveform
                .waveform()
                .signal_by_path(path)
                .map(|info| info.width())
        }) {
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
                    self.apply(Action::PickerOpened {
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
            bit: None,
        });
        self.apply(Action::FocusChanged {
            focus: Focus {
                signal: Some(row_path.clone()),
                instance: self.focus.instance.clone(),
            },
        });
        self.note(Status::info(format!("added {row_path} to the waveform")));
        if show_declaration {
            // only jump when the source pane can show the file: a closed tab
            // would otherwise turn a successful add into an error status
            if let Some((file, line, col)) = self.declare_target(&row_path) {
                if self.sources.index_of(&file).is_some() {
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
        let (is_empty, names, top) = self.store.select(|state| {
            (
                state.design.is_empty(),
                state
                    .design
                    .resolve_candidates(path)
                    .iter()
                    .take(4)
                    .map(|signal| signal.path.clone())
                    .collect::<Vec<_>>(),
                state.design.top().map(str::to_owned),
            )
        });
        if is_empty {
            self.note(Status::error(format!(
                "no sources loaded — `{path}` is declared in the .v/.sv files; load them to navigate"
            )));
            return;
        }
        let count = self
            .store
            .select(|state| state.design.resolve_candidates(path).len());
        if count > 1 {
            let more = if count > names.len() { ", ..." } else { "" };
            self.note(Status::error(format!(
                "ambiguous: {count} signals match `{path}` — {}{more}",
                names.join(", ")
            )));
            return;
        }
        match top {
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
        let (targets, has_signal, design_empty) = self.store.select(|state| {
            let signal = state.design.resolve_path(path);
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
            (targets, signal.is_some(), state.design.is_empty())
        });
        if !has_signal {
            let message = if design_empty {
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
        if self.sources.index_of(&file).is_none() {
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
        let mut found = self.store.select(|state| {
            let exact = state.design.signals_at(&click.file, click.line, click.col);
            if exact.is_empty() {
                state.design.signals_matching(&click.word)
            } else {
                exact
            }
        });
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
        html! {
            <FileLoad
                ongetwaveform={ctx.link().callback(Msg::GetWaveform)}
                ongetverilog={ctx.link().callback(Msg::GetVerilog)}
                onerror={ctx.link().callback(Msg::Error)}
                onstatus={ctx.link().callback(Msg::Note)}
                onloadreport={ctx.link().callback(Msg::LoadReport)} />
        }
    }
}
