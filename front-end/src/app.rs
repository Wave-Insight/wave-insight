use std::cell::Cell;
#[cfg(feature = "tauri")]
use std::collections::VecDeque;

use yew::prelude::*;

use crate::code_reader::CodeReader;
use crate::lsp::LspBridge;
use crate::module_struct::ModuleStruct;
use crate::module_struct::{RowContext, RowKind, TreeSelect};
use crate::nav::selection_add::{self, EditorSelection};
use crate::nav::{match_dump_candidates, PathPicker, SidebarTree, SignalInspector, SourceClick};
use crate::state::{
    self, Action, Cursors, DumpHandle, DumpId, DumpsTabsSlice, Effect, Focus, Goto, Layout, Picker,
    Row, RowsSlice, SourcesSlice, Split, Status, Store, Subscription, TreeSelection,
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
    /// The split divider was grabbed.
    SplitSeparatorDown(MouseEvent),
    /// The pointer moved while the divider was grabbed.
    SplitSeparatorMove(MouseEvent),
    /// The divider drag finished.
    SplitSeparatorUp,
    /// A key press on the focused divider (arrow keys move it).
    SplitSeparatorKey(KeyboardEvent),
    /// A module-tree row was clicked: the sidebar selection moved.
    InstanceSelected(String),
    /// The store's module-tree multi-selection changed.
    TreeSelectionChanged(TreeSelection),
    /// A module-tree row was clicked with modifiers (bulk selection).
    TreeSelected(TreeSelect),
    /// "Add all signals in this scope" was clicked for this scope path.
    AddScope(String),
    /// "Add selected (N)" / Ctrl+Enter: bulk-add the tree selection.
    AddSelected,
    /// A sidebar row asked for its context menu.
    RowMenuOpened(RowContext),
    /// The context menu closed (Escape / outside click / an item ran).
    RowMenuClosed,
    /// Context menu: add one signal.
    MenuAddSignal(String),
    /// Context menu: add every signal of a scope subtree.
    MenuAddScope(String),
    /// Context menu: toggle one scope in the bulk selection.
    MenuToggleSelect(String),
    /// Context menu: the selection becomes these visible scopes.
    MenuSelectAll(Vec<String>),
    /// Context menu: clear the bulk selection.
    MenuClearSelection,
    /// A key press while the context menu is open (Escape / arrows).
    RowMenuKey(KeyboardEvent),
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
    /// Several signal rows were selected in the signal list and explicitly added
    /// ("Add selected"): their hierarchical paths, dispatched as ONE batch.
    SidebarAddMany(Vec<String>),
    /// A dump was opened: the handle values are decoded from, plus its time
    /// range so the ruler and the fit-to-trace zoom can be drawn.
    GetWaveform(DumpHandle),
    /// One source file was read; it goes to the design database.
    GetVerilog((String, String)),
    /// A source tab was closed; the file leaves the source pane.
    CloseSource(usize),
    /// A file drag entered or left the window: the full-window "drop to load"
    /// overlay is shown while one is over the app. Fed by the browser loader's
    /// window listeners and, in the desktop build, by the webview's native
    /// drag-drop events.
    DropDragActive(bool),
    /// A file could not be opened, or the design could not be parsed.
    Error(String),
    /// A line of progress or a note from the file loader (info tone).
    Note(String),
    /// What the last successful parse cost; the success line carries it.
    LoadReport(LoadReport),
    /// A native (desktop) dump was opened; the wave pane draws it read-only.
    #[cfg(feature = "tauri")]
    RemoteReady(Box<crate::remote::RemoteSession>),
    /// The native backend declined the file (small dump or an error): read it
    /// through the wasm path instead.
    #[cfg(feature = "tauri")]
    RemoteFallback(String),
    /// A desktop reload is reopening the dumps of the previous page, oldest
    /// first: the app opens them one at a time, through the same open path a
    /// drop or a dialog pick takes (see `DesktopRestore`).
    #[cfg(feature = "tauri")]
    DesktopRestore(Vec<String>),
    /// One remembered path could not be reopened (the file is gone, unreadable,
    /// or not a dump any more). It leaves the reopen list; the failures of one
    /// restore are reported in one aggregated line, never one error per file.
    #[cfg(feature = "tauri")]
    DesktopRestoreFailed(String, String),
    /// The companion RTL sources next to the opened dump were read (desktop
    /// build): the app loads them as source documents and says so once.
    #[cfg(feature = "tauri")]
    CompanionRtlLoaded {
        /// The discovery generation this answer belongs to; an answer for a dump
        /// that was replaced meanwhile is dropped.
        generation: u32,
        /// The `rtl` (or `generated`/`verilog`) directory the files came from.
        dir: String,
        /// `(absolute path, text)` of each source not already open.
        files: Vec<(String, String)>,
    },
    /// A plain click on an identifier in the source editor.
    SourceClick(SourceClick),
    /// Ctrl+Enter in the source editor: add the signal under the cursor to the
    /// wave view. The click itself only selects, so this is the explicit add.
    SourceAddSignal(SourceClick),
    /// A block of code was selected and the user asked for every signal it
    /// mentions (Verdi's block add): Ctrl+W with a selection, the Ctrl+Shift+W
    /// alias, or the editor's context-menu entry. One rows batch.
    AddSelectionToWave(EditorSelection),
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

/// The dumps a desktop reload is reopening, one at a time.
///
/// A webview reload (Ctrl+Shift+R) throws the wasm state away while the Rust
/// backend keeps running: the absolute paths of the dumps the previous page had
/// open are remembered (see [`crate::state::persist::DesktopPaths`]) and this
/// state machine opens them again through the very same path a drop or a dialog
/// pick takes — the native backend first, the wasm reader for a small file — so
/// each dump's remembered rows and per-signal settings come back through the
/// existing session mechanism. Restores are sequential on purpose: the native
/// backend is one-dump-at-a-time and the wasm memory budget counts every open.
#[cfg(feature = "tauri")]
struct DesktopRestore {
    /// The paths not tried yet, oldest first.
    pending: VecDeque<String>,
    /// How many dumps of the list were opened.
    opened: usize,
    /// The paths that could not be opened, with the reason, in list order.
    failed: Vec<(String, String)>,
    /// How many paths the list started with (for the aggregate status line).
    total: usize,
    /// The path currently being opened. A wasm read reports a failure through
    /// the normal [`Msg::Error`] path, which uses this to count the dump once.
    active: Option<String>,
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
    /// `window.addEventListener("keydown")` for Ctrl/Cmd+Enter (bulk add).
    keys_listener: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
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
    /// The split container, measured while its divider is dragged.
    split_view: NodeRef,
    /// Pointer x at the divider press.
    split_drag: Option<f64>,
    /// Live ratio while the divider is dragged (tier A: no shell render).
    split_preview: Option<f32>,
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
    /// The module-tree multi-selection (bulk add), mirrored from the store.
    tree_selection: TreeSelection,
    _tree_selection_sub: Subscription,
    /// The sidebar row context menu, when it is open.
    row_menu: Option<RowContext>,
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
    /// A file drag is over the window: the full-window overlay says the files
    /// can be dropped anywhere. The loader owns the browser drag state, the
    /// desktop webview its native drag events; both report it here.
    drop_active: bool,
    /// The native dump the desktop build is drawing read-only, when one is open.
    #[cfg(feature = "tauri")]
    remote: Option<crate::remote::RemoteSession>,
    /// The absolute paths of the dumps the desktop build currently has open,
    /// oldest first: the list a webview reload reopens. Only the desktop build
    /// can know paths (a browser `File` handle dies with the page), which is
    /// why every field of this group is tauri-only.
    #[cfg(feature = "tauri")]
    desktop_paths: crate::state::persist::DesktopPaths,
    /// Where the desktop reopen list is read and written; `None` when the
    /// browser denies `localStorage` (the list then simply does not survive).
    #[cfg(feature = "tauri")]
    desktop_persister: Option<std::rc::Rc<crate::state::persist::Persister>>,
    /// The path of the native dump that is open, when one is. The native backend
    /// is one-dump-at-a-time: opening another native dump replaces this one, and
    /// the replaced dump's path has to leave the reopen list.
    #[cfg(feature = "tauri")]
    native_path: Option<String>,
    /// The absolute path behind each wasm dump tab, by dump identity: closing a
    /// tab forgets its path (the tab list itself keeps only name + size).
    #[cfg(feature = "tauri")]
    tab_paths: Vec<(DumpId, String)>,
    /// The dumps a desktop reload is currently reopening, one at a time.
    #[cfg(feature = "tauri")]
    restore: Option<DesktopRestore>,
    /// Bumped by every companion-RTL discovery. An answer that arrives for a
    /// dump that was replaced meanwhile is dropped instead of loading the wrong
    /// sources next to the new waveform.
    #[cfg(feature = "tauri")]
    companion_gen: Cell<u32>,
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    #[allow(unused_variables)] // `ctx` is only used by the tauri/server transports
    fn create(ctx: &Context<Self>) -> Self {
        #[cfg(feature = "tauri")]
        let file_drop_callback = ctx.link().callback(Msg::FileDrop);
        #[cfg(feature = "tauri")]
        let drag_callback = ctx.link().callback(Msg::DropDragActive);

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
                match kind.as_deref() {
                    // the native webview owns the OS drag: these events are what
                    // draws the same "drop to load" overlay the browser build
                    // shows (the drop everywhere already works here)
                    Some("enter") | Some("over") => {
                        drag_callback.emit(true);
                        return;
                    }
                    Some("leave") => {
                        drag_callback.emit(false);
                        return;
                    }
                    Some("drop") => drag_callback.emit(false),
                    _ => return,
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
        //
        // Wave 19: the desktop build also restores the absolute paths of the
        // dumps the previous page had open, so a webview reload can reopen them
        // (the browser cannot: a `File` handle does not survive a reload). The
        // read is as forgiving as every other key: an absent or corrupt value is
        // an empty list.
        let persister = state::persist::browser_persister();
        #[cfg(feature = "tauri")]
        let (desktop_paths, desktop_persister) = match &persister {
            Some(persister) => (persister.desktop_paths(), Some(persister.clone())),
            None => (crate::state::persist::DesktopPaths::default(), None),
        };
        let store = Store::with_persister(persister);
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
        let tree_selection = store.select(state::select::tree_selection);
        let tree_selection_sub = store.subscribe(
            state::select::tree_selection,
            ctx.link().callback(Msg::TreeSelectionChanged),
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
            keys_listener: None,
            tree,
            _tree_sub: tree_sub,
            dump_tabs,
            _dumps_sub: dumps_sub,
            split,
            _split_sub: split_sub,
            tree_selection,
            _tree_selection_sub: tree_selection_sub,
            row_menu: None,
            split_view: NodeRef::default(),
            split_drag: None,
            split_preview: None,
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
            drop_active: false,
            #[cfg(feature = "tauri")]
            remote: None,
            #[cfg(feature = "tauri")]
            desktop_paths,
            #[cfg(feature = "tauri")]
            desktop_persister,
            #[cfg(feature = "tauri")]
            native_path: None,
            #[cfg(feature = "tauri")]
            tab_paths: Vec::new(),
            #[cfg(feature = "tauri")]
            restore: None,
            #[cfg(feature = "tauri")]
            companion_gen: Cell::new(0),
        };
        // a native dump opened through the desktop file dialog arrives here
        #[cfg(feature = "tauri")]
        {
            let link = ctx.link().clone();
            crate::remote::set_open_listener(Some(std::rc::Rc::new(move |session| {
                link.send_message(Msg::RemoteReady(Box::new(session)));
            })));
        }
        this.attach_window_listeners(ctx);
        // Wave 19: a desktop page start frees the native mappings the previous
        // page left behind (the registry outlives the page) and then reopens the
        // dumps that page had open, oldest first. Nothing happens on a browser
        // start, and nothing happens when the list is empty (the user closed
        // every dump last session).
        #[cfg(feature = "tauri")]
        {
            let link = ctx.link().clone();
            let restored = this.desktop_paths.clone();
            spawn_local(async move {
                // the registry is process state; the ids of the previous page
                // went away with it, so close what it left behind before
                // opening anything new
                let _ = crate::remote::close_all().await;
                // let the first paint happen before the IO starts
                crate::file_load::file::next_tick().await;
                crate::file_load::file::next_tick().await;
                if !restored.is_empty() {
                    link.send_message(Msg::DesktopRestore(restored.paths().to_vec()));
                }
            });
        }
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
        #[cfg(feature = "tauri")]
        {
            crate::remote::set_open_listener(None);
            // releasing the native session here must NOT touch the desktop
            // reopen list: this is the teardown of a webview reload (Ctrl+Shift+R),
            // and the whole point of the list is to open that dump again.
            self.close_remote();
        }
        if let (Some(window), Some(listener)) = (web_sys::window(), self.resize_listener.take()) {
            let _ = window
                .remove_event_listener_with_callback("resize", listener.as_ref().unchecked_ref());
        }
        if let (Some(window), Some(listener)) = (web_sys::window(), self.keys_listener.take()) {
            let _ = window
                .remove_event_listener_with_callback("keydown", listener.as_ref().unchecked_ref());
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
                    // The sidebar stays mounted while it is closed: the file
                    // loader in it owns the window-wide drop listeners and the
                    // read queue, and closing the drawer must not disarm them
                    // (or throw away a read in progress). `hidden` is display:none.
                    <aside class={if self.layout.drawer_open {"sidebar"} else {"sidebar hidden"}}>
                            {self.file_button(ctx)}
                            <ModuleStruct tree={self.sidebar_tree()}
                                focus={self.focus.clone()}
                                oninstance={link.callback(Msg::InstanceSelected)}
                                selection={self.tree_selection.paths.clone()}
                                anchor={self.tree_selection.anchor.clone()}
                                onselection={link.callback(Msg::TreeSelected)}
                                onaddscope={link.callback(Msg::AddScope)}
                                onaddselected={link.callback(|_| Msg::AddSelected)}
                                onclear={link.callback(|_| Msg::MenuClearSelection)}
                                onrowcontext={link.callback(Msg::RowMenuOpened)}
                                onaddmany={link.callback(Msg::SidebarAddMany)}
                                signaladd={link.callback(|(_name, path): (String, String)| Msg::SidebarAdd(path))}/>
                    </aside>
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
                                            {" ) anywhere in this window to inspect its signals."}</div>
                                        <div>{"Drop your "}<kbd>{".sv"}</kbd>{" / "}<kbd>{".v"}</kbd>
                                            {" sources as well to get declarations, drivers and loads."}</div>
                                        <div>{"Or use the ＋ button in the sidebar."}</div>
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
                                onwordclick={link.callback(Msg::SourceClick)}
                                onaddselection={link.callback(Msg::AddSelectionToWave)}
                                onaddsignal={link.callback(Msg::SourceAddSignal)} />
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
                                {self.remote_badge()}
                                // The label column shows the leaf signal name by
                                // default; this checkbox brings the full instance
                                // path back. The option rides with the layout, so
                                // it survives a reload.
                                <label class="dock-option"
                                    title="Show the full instance path in front of each signal name">
                                    <input type="checkbox" checked={self.layout.show_path}
                                        onchange={link.callback(|event: web_sys::Event| {
                                            let input: web_sys::HtmlInputElement = event.target_unchecked_into();
                                            Msg::Action(Action::SetShowPath(input.checked()))
                                        })} />
                                    {"Full path"}
                                </label>
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
                                    let ratio = self.split_preview.unwrap_or(self.layout.split_ratio);
                                    html!{
                                    <div class="split-view" ref={self.split_view.clone()}
                                        style={format!("--split-ratio:{ratio:.3}")}>
                                        {self.wave_pane(link)}
                                        <div class={if self.split_drag.is_some() {"split-separator dragging"} else {"split-separator"}}
                                            role="separator"
                                            aria-orientation="vertical"
                                            aria-label="Split divider"
                                            aria-valuemin={format!("{:.0}", state::types::SPLIT_RATIO_MIN * 100.0)}
                                            aria-valuemax={format!("{:.0}", state::types::SPLIT_RATIO_MAX * 100.0)}
                                            aria-valuenow={format!("{:.0}", ratio * 100.0)}
                                            tabindex="0"
                                            title="Drag to resize the two panes"
                                            onmousedown={link.callback(Msg::SplitSeparatorDown)}
                                            onkeydown={link.callback(Msg::SplitSeparatorKey)} />
                                        <SplitPane
                                            // keyed by the dump only: the pane must survive the
                                            // linked toggle so it can freeze its window on unlink
                                            key={split.right.to_string()}
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
                {self.drop_overlay()}
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
                {if self.split_drag.is_some() {
                    html!{
                        <div class="split-shield"
                            onmousemove={link.callback(Msg::SplitSeparatorMove)}
                            onmouseup={link.callback(|_| Msg::SplitSeparatorUp)}
                            // releasing outside the window never fires mouseup
                            onmouseout={link.callback(|_| Msg::SplitSeparatorUp)} />
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
                {if let Some(menu) = &self.row_menu {
                    let style = format!("left:{}px; top:{}px", menu.x, menu.y);
                    let is_module = menu.kind == RowKind::Module;
                    let scope_path = if is_module {
                        Some(menu.path.clone())
                    } else {
                        state::bulk::parent_scope(&menu.path)
                    };
                    let scope_selected = scope_path
                        .as_ref()
                        .is_some_and(|scope| self.tree_selection.paths.contains(scope));
                    html!{ <>
                        <div class="row-menu-shield"
                            onclick={link.callback(|_| Msg::RowMenuClosed)}
                            oncontextmenu={link.callback(|event: MouseEvent| {
                                event.prevent_default();
                                Msg::RowMenuClosed
                            })} />
                        <div class="row-menu" role="menu" aria-label="Row actions" style={style}
                            onkeydown={link.callback(Msg::RowMenuKey)}>
                            {if is_module {
                                html!{
                                    <button type="button" class="row-menu-item" role="menuitem"
                                        disabled={menu.scope_signals == 0}
                                        onclick={link.callback({ let path = menu.path.clone(); move |_| Msg::MenuAddScope(path.clone()) })}>
                                        {format!("Add all signals in this scope ({})", menu.scope_signals)}
                                    </button>
                                }
                            } else {
                                html!{
                                    <button type="button" class="row-menu-item" role="menuitem"
                                        onclick={link.callback({ let path = menu.path.clone(); move |_| Msg::MenuAddSignal(path.clone()) })}>
                                        {"Add signal"}
                                    </button>
                                }
                            }}
                            {if !is_module {
                                match scope_path.clone() {
                                    Some(scope) => html!{
                                        <button type="button" class="row-menu-item" role="menuitem"
                                            onclick={link.callback(move |_| Msg::MenuAddScope(scope.clone()))}>
                                            {"Add all signals in this scope"}
                                        </button>
                                    },
                                    None => html!{},
                                }
                            } else {
                                html!{}
                            }}
                            <div class="row-menu-sep" role="separator"></div>
                            {match scope_path.clone() {
                                Some(scope) => html!{
                                    <button type="button" class="row-menu-item" role="menuitem"
                                        onclick={link.callback(move |_| Msg::MenuToggleSelect(scope.clone()))}>
                                        {if scope_selected { "Remove scope from bulk selection" } else { "Select for bulk add" }}
                                    </button>
                                },
                                None => html!{},
                            }}
                            {if is_module && !menu.visible.is_empty() {
                                html!{
                                    <button type="button" class="row-menu-item" role="menuitem"
                                        onclick={link.callback({ let paths = menu.visible.clone(); move |_| Msg::MenuSelectAll(paths.clone()) })}>
                                        {format!("Select all visible ({})", menu.visible.len())}
                                    </button>
                                }
                            } else {
                                html!{}
                            }}
                            {if !self.tree_selection.is_empty() {
                                html!{
                                    <button type="button" class="row-menu-item" role="menuitem"
                                        onclick={link.callback(|_| Msg::MenuClearSelection)}>
                                        {"Clear bulk selection"}
                                    </button>
                                }
                            } else {
                                html!{}
                            }}
                        </div>
                    </> }
                } else {
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
        // Ctrl/Cmd+Enter adds the module-tree selection in one batch (the
        // keyboard path for "Add selected"), unless a text field or the editor
        // has the focus and owns the shortcut
        let link = ctx.link().clone();
        let keys = Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(
            move |event: web_sys::KeyboardEvent| {
                if event.key() == "Escape" {
                    // the row context menu is the only global Escape consumer
                    link.send_message(Msg::RowMenuClosed);
                    return;
                }
                if !(event.ctrl_key() || event.meta_key()) || event.key() != "Enter" {
                    return;
                }
                let owns = event
                    .target()
                    .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    .and_then(|element| {
                        element
                            .closest("input, textarea, select, .monaco-editor")
                            .ok()
                            .flatten()
                    })
                    .is_some();
                if owns {
                    return;
                }
                event.prevent_default();
                link.send_message(Msg::AddSelected);
            },
        );
        if window
            .add_event_listener_with_callback("keydown", keys.as_ref().unchecked_ref())
            .is_ok()
        {
            self.keys_listener = Some(keys);
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
                        let file = p.clone();
                        let onready =
                            ctx.link()
                                .callback(|session: Box<crate::remote::RemoteSession>| {
                                    Msg::RemoteReady(session)
                                });
                        let onfallback = ctx.link().callback(Msg::RemoteFallback);
                        let onerror = ctx.link().callback(Msg::Error);
                        spawn_local(async move {
                            // Wave 15/17: the desktop backend maps the file instead
                            // of copying it into wasm memory. A dump the backend
                            // proved small (the >64 MiB threshold) still takes the
                            // wasm path; a native ERROR does not, because copying a
                            // possibly huge file into wasm to retry is exactly the
                            // multi-GB peak this path avoids.
                            match crate::remote::try_open(&file).await {
                                Ok(Some(session)) => onready.emit(Box::new(session)),
                                Ok(None) => onfallback.emit(file),
                                Err(error) => match crate::remote::native_fallback(false) {
                                    crate::remote::NativeFallback::Wasm => {
                                        onfallback.emit(file.clone())
                                    }
                                    crate::remote::NativeFallback::Report => onerror
                                        .emit(crate::remote::native_error_status(&file, &error)),
                                },
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
                // Wave 19: the closed dump is no longer open, so its absolute
                // path leaves the desktop reopen list
                #[cfg(feature = "tauri")]
                self.forget_closed_tab(index);
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
                    #[cfg(feature = "tauri")]
                    self.forget_closed_tab(index);
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
            Msg::SplitSeparatorDown(event) => {
                event.prevent_default();
                self.split_drag = Some(event.client_x() as f64);
                self.split_preview = Some(self.layout.split_ratio);
                true
            }
            Msg::SplitSeparatorMove(event) => {
                let Some(start) = self.split_drag else {
                    return false;
                };
                let Some(container) = self.split_view.cast::<web_sys::Element>() else {
                    return false;
                };
                let rect = container.get_bounding_client_rect();
                if rect.width() <= 1.0 {
                    return false;
                }
                let ratio = ((event.client_x() as f64 - rect.left()) / rect.width()) as f32;
                let ratio = state::types::clamp_split_ratio(ratio);
                self.split_preview = Some(ratio);
                // tier A: write the CSS variable straight to the DOM, no shell
                // render while the pointer moves (the commit is on mouseup)
                let _ = container.set_attribute("style", &format!("--split-ratio:{ratio:.3}"));
                let _ = start;
                false
            }
            Msg::SplitSeparatorUp => {
                self.split_drag = None;
                match self.split_preview.take() {
                    Some(ratio) => {
                        self.apply(Action::SetSplitRatio(ratio));
                        false
                    }
                    None => true,
                }
            }
            Msg::SplitSeparatorKey(event) => {
                let step = 0.02;
                let current = self.layout.split_ratio;
                let next = match event.key().as_str() {
                    "ArrowLeft" => Some(current - step),
                    "ArrowRight" => Some(current + step),
                    "Home" => Some(state::types::SPLIT_RATIO_MIN),
                    "End" => Some(state::types::SPLIT_RATIO_MAX),
                    _ => None,
                };
                match next {
                    Some(ratio) => {
                        event.prevent_default();
                        self.apply(Action::SetSplitRatio(ratio));
                        false
                    }
                    None => false,
                }
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
            Msg::TreeSelectionChanged(selection) => {
                self.tree_selection = selection;
                true
            }
            Msg::TreeSelected(event) => {
                match event {
                    TreeSelect::Reset(path) => self.apply(Action::TreeSelectionReset { path }),
                    TreeSelect::Toggle(path) => self.apply(Action::TreeSelectionToggled { path }),
                    TreeSelect::Range(paths) => self.apply(Action::TreeSelectionRanged { paths }),
                }
                false
            }
            Msg::AddScope(path) => {
                self.bulk_add_scopes(std::slice::from_ref(&path));
                false
            }
            Msg::AddSelected => {
                self.bulk_add_selected();
                false
            }
            Msg::RowMenuOpened(context) => {
                self.row_menu = Some(context);
                true
            }
            Msg::RowMenuClosed => self.row_menu.take().is_some(),
            Msg::MenuAddSignal(path) => {
                self.row_menu = None;
                self.add_path(&path, false);
                false
            }
            Msg::MenuAddScope(path) => {
                self.row_menu = None;
                self.bulk_add_scopes(std::slice::from_ref(&path));
                false
            }
            Msg::MenuToggleSelect(path) => {
                self.row_menu = None;
                self.apply(Action::TreeSelectionToggled { path });
                false
            }
            Msg::MenuSelectAll(paths) => {
                self.row_menu = None;
                self.apply(Action::TreeSelectionRanged { paths });
                false
            }
            Msg::MenuClearSelection => {
                self.row_menu = None;
                self.apply(Action::TreeSelectionCleared);
                false
            }
            Msg::RowMenuKey(event) => match event.key().as_str() {
                "Escape" => {
                    event.prevent_default();
                    self.row_menu = None;
                    true
                }
                "ArrowDown" | "ArrowUp" => {
                    event.prevent_default();
                    // move the focus between the menu items (they are buttons)
                    let Some(menu) = event
                        .current_target()
                        .and_then(|target| target.dyn_into::<web_sys::Element>().ok())
                    else {
                        return false;
                    };
                    let Ok(items) = menu.query_selector_all(".row-menu-item") else {
                        return false;
                    };
                    let count = items.length();
                    if count == 0 {
                        return false;
                    }
                    let focused = web_sys::window()
                        .and_then(|window| window.document())
                        .and_then(|document| document.active_element());
                    let current = focused
                        .as_ref()
                        .and_then(|element| element.closest(".row-menu-item").ok().flatten())
                        .and_then(|element| {
                            (0..count).find(|index| {
                                items
                                    .get(*index)
                                    .map(|node| element.is_same_node(Some(node.as_ref())))
                                    .unwrap_or(false)
                            })
                        });
                    let next = match (event.key().as_str(), current) {
                        ("ArrowDown", Some(index)) => (index + 1) % count,
                        ("ArrowDown", None) => 0,
                        (_, Some(index)) => (index + count - 1) % count,
                        (_, None) => count - 1,
                    };
                    if let Some(item) = items.get(next) {
                        if let Ok(element) = item.dyn_into::<web_sys::HtmlElement>() {
                            let _ = element.focus();
                        }
                    }
                    false
                }
                _ => false,
            },
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
            Msg::SidebarAddMany(paths) => {
                // the signal list's explicit "Add selected (N)": one batch for
                // the whole selection, no declaration jump (many rows have many
                // declarations; the first added row becomes the inspector's)
                self.add_signal_batch(&paths, "signals");
                false
            }
            Msg::GetWaveform(waveform) => {
                // a wasm dump replaces the native one (two modes at once would
                // draw nothing consistently); the replaced native dump's path
                // leaves the desktop reopen list too (Wave 19)
                #[cfg(feature = "tauri")]
                self.forget_native_path();
                self.close_remote();
                // the loader adds a tab: the name and the size are the identity
                // the remembered row list is keyed by (the report is emitted
                // before the handle, so it is already here). A dump that arrived
                // as raw bytes has no name and is never remembered.
                let fingerprint = self.pending_load.as_ref().map(|report| report.fingerprint);
                let id = DumpId::new(
                    self.pending_load
                        .as_ref()
                        .map(|report| report.file_name.clone())
                        .unwrap_or_default(),
                    self.pending_load.as_ref().map(|report| report.bytes),
                );
                // Wave 19: when the open came through an absolute desktop path
                // (a drag&drop fallback or a dialog pick the backend resolved)
                // the app remembers it for the next desktop start. The browser
                // reports no path, so this is `None` there and nothing happens.
                #[cfg(feature = "tauri")]
                let dump_path = self
                    .pending_load
                    .as_ref()
                    .and_then(|report| report.path.clone());
                // a re-opened dump whose identity (name + size) is already open
                // reloads in place when its bytes changed (a re-simulated file):
                // the status line has to say "reloaded" instead of "loaded".
                // Same rule as the reducer: only a known pair of fingerprints that
                // differs is a reload.
                let reload = self.store.select(|state| {
                    state.dumps.index_of(&id).is_some_and(|index| {
                        match (state.dumps.items[index].fingerprint, fingerprint) {
                            (Some(open), Some(incoming)) => open != incoming,
                            _ => false,
                        }
                    })
                });
                self.apply(Action::DumpAdded {
                    id: id.clone(),
                    handle: waveform.clone(),
                    fingerprint,
                });
                // the memory budget may have refused the tab: keep its actionable
                // error instead of overwriting it with a load line
                let opened = self
                    .store
                    .select(|state| state.dumps.items.iter().any(|entry| entry.id.matches(&id)));
                if !opened {
                    // a desktop restore moves on: the memory budget refused this
                    // dump, nothing was opened and no path is remembered
                    #[cfg(feature = "tauri")]
                    self.restore_advance(ctx);
                    return true;
                }
                // Wave 19: the dump is open: remember its absolute path for the
                // next desktop start, and map the tab to it so closing the tab
                // forgets it. A reload in place (Wave 18 fingerprint) keeps the
                // same path, which the list already holds.
                #[cfg(feature = "tauri")]
                self.remember_opened_dump(dump_path.as_deref(), &id);
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
                let status = if reload {
                    // the tab and its rows stayed, the waveform is the new file's
                    match (&range_note, &report) {
                        (Some(note), Some(report)) => {
                            format!("waveform reloaded — {note} — {}", report.suffix())
                        }
                        (Some(note), None) => format!("waveform reloaded — {note}"),
                        (None, Some(report)) => {
                            format!("waveform reloaded — {}", report.suffix())
                        }
                        (None, None) => "waveform reloaded".to_string(),
                    }
                } else {
                    match (range_note, report) {
                        (Some(note), Some(report)) => format!("{note} — {}", report.suffix()),
                        (Some(note), None) => note,
                        (None, Some(report)) => report.status(),
                        (None, None) => "waveform loaded".to_string(),
                    }
                };
                // a remembered session may contain rows this dump does not have
                // (a signal was renamed, or the file is not quite the same one);
                // say so once, next to the load line, instead of failing the open.
                // A reload re-resolves the rows it already had, so the wording
                // there is about the new file, not about a remembered session.
                let status = match self.store.select(state::select::session_restore) {
                    Some(restore) if restore.skipped > 0 => {
                        if reload {
                            format!(
                                "{status} — {} row(s) are not in the reloaded file",
                                restore.skipped
                            )
                        } else {
                            format!(
                                "{status} — {} remembered row(s) are not in this dump",
                                restore.skipped
                            )
                        }
                    }
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
                #[cfg(feature = "tauri")]
                {
                    // a desktop restore counts the dump and opens the next one;
                    // every desktop dump open (drag&drop, dialog, restored) also
                    // looks for SpinalHDL companion RTL next to the file
                    if let Some(restore) = self.restore.as_mut() {
                        restore.opened += 1;
                        restore.active = None;
                    }
                    self.restore_advance(ctx);
                    self.discover_companion_rtl(ctx, dump_path);
                }
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
            Msg::DropDragActive(active) => {
                if self.drop_active == active {
                    return false;
                }
                self.drop_active = active;
                true
            }
            Msg::Error(message) => {
                // a failed open must not leave a stale report behind
                self.pending_load = None;
                #[cfg(feature = "tauri")]
                {
                    if self.restore.is_some() {
                        // a dump being reopened failed through its normal error
                        // path (the wasm read): count it once and move on. The
                        // aggregate line at the end of the restore names it,
                        // instead of printing one error per remembered file.
                        self.restore_failed_current(message);
                        self.restore_advance(ctx);
                        return true;
                    }
                }
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
            #[cfg(feature = "tauri")]
            Msg::RemoteFallback(file) => {
                #[derive(Serialize, Deserialize)]
                struct GetFileListArgs {
                    path: String,
                }
                let args = to_value(&GetFileListArgs { path: file.clone() }).unwrap();
                let onwaveform = ctx.link().callback(Msg::GetWaveform);
                let onreport = ctx.link().callback(Msg::LoadReport);
                let onnote = ctx.link().callback(Msg::Note);
                let onerror = ctx.link().callback(Msg::Error);
                spawn_local(async move {
                    // the wasm path: the command returns the whole file, so there
                    // are no slices to report; it gets the size guard, the parse
                    // line and the load report
                    let raw = invoke("choose_vcd_absolute", args).await;
                    let bytes = match crate::file_load::dump_bytes(&raw) {
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
                    if let Some(message) = crate::file_load::progress::guard_message(&file, size) {
                        match crate::file_load::progress::size_verdict(size) {
                            crate::file_load::progress::SizeVerdict::TooLarge => {
                                onerror.emit(message);
                                return;
                            }
                            crate::file_load::progress::SizeVerdict::Large => onnote.emit(message),
                            crate::file_load::progress::SizeVerdict::Ok => {}
                        }
                    }
                    onnote.emit(crate::file_load::progress::parsing_status(
                        &file, size, None,
                    ));
                    crate::file_load::file::next_tick().await;
                    match crate::file_load::parse_dump(&file, bytes) {
                        Ok((handle, mut report)) => {
                            // Wave 19: this wasm fallback knows the absolute path
                            // (the native open that declined the file carried it),
                            // so the app can remember the dump for the next
                            // desktop start
                            report.path = Some(file.clone());
                            onreport.emit(report);
                            onwaveform.emit(handle);
                        }
                        Err(message) => onerror.emit(message),
                    }
                });
                true
            }
            #[cfg(feature = "tauri")]
            Msg::RemoteReady(session) => {
                let path = session.dump().path.clone();
                // the native dump replaces the previous native one and is one of
                // the open dumps now: adopt_remote maintains the reopen list
                self.adopt_remote(*session);
                if let Some(restore) = self.restore.as_mut() {
                    restore.opened += 1;
                    restore.active = None;
                }
                self.restore_advance(ctx);
                // every desktop dump open looks for companion RTL next to the
                // file, restored ones included (same path as a manual open)
                self.discover_companion_rtl(ctx, Some(path));
                true
            }
            #[cfg(feature = "tauri")]
            Msg::DesktopRestore(paths) => {
                self.start_restore(ctx, paths);
                true
            }
            #[cfg(feature = "tauri")]
            Msg::DesktopRestoreFailed(path, message) => {
                if let Some(restore) = self.restore.as_mut() {
                    restore.failed.push((path, message));
                    restore.active = None;
                }
                self.restore_advance(ctx);
                true
            }
            #[cfg(feature = "tauri")]
            Msg::CompanionRtlLoaded {
                generation,
                dir,
                files,
            } => {
                // a dump that was replaced while the scan/reads ran invalidates
                // this answer; the new dump's discovery bumps the generation
                if generation != self.companion_gen.get() || files.is_empty() {
                    return false;
                }
                let count = files.len();
                for (file, text) in files {
                    // the same flow a dropped source takes: the store parses it
                    // into the design database and asks for the LSP document; the
                    // per-file "parsed ..." status is replaced by the one line
                    // below, so the auto-load stays a single feedback line
                    self.apply(Action::SourceOpened { file, text });
                }
                crate::lsp::inlay::refresh();
                self.note(Status::info(format!(
                    "loaded {count} companion RTL source(s) from {dir}"
                )));
                true
            }
            Msg::SourceClick(click) => {
                // A click *selects*, it never adds: it points the inspector at
                // the signal under the cursor and says how to add it. Reading the
                // source must not grow the wave list by accident — the add is
                // the explicit Ctrl+Enter below (or a sidebar Add gesture).
                let candidates = self.candidates_at(&click);
                match candidates.len() {
                    0 => {
                        self.note(Status::info(format!(
                            "`{}` is not a signal of the elaborated design",
                            click.word
                        )));
                    }
                    1 => {
                        self.apply(Action::FocusChanged {
                            focus: Focus {
                                signal: Some(candidates[0].clone()),
                                instance: self.focus.instance.clone(),
                            },
                        });
                        self.note(Status::info(format!(
                            "`{}` selected — Ctrl+Enter adds it to the waveform",
                            candidates[0]
                        )));
                    }
                    _ => {
                        self.note(Status::info(format!(
                            "{} signals match `{}` — Ctrl+Enter picks one to add",
                            candidates.len(),
                            click.word
                        )));
                    }
                }
                true
            }
            Msg::SourceAddSignal(click) => {
                // Ctrl+Enter: the explicit "add the signal I am looking at".
                // One match is added straight away; an ambiguous name opens the
                // picker instead of guessing (the same rule a sidebar add uses).
                let candidates = self.candidates_at(&click);
                match candidates.len() {
                    0 => {
                        self.note(Status::info(format!(
                            "`{}` is not a signal of the elaborated design",
                            click.word
                        )));
                    }
                    1 => self.add_path(&candidates[0], false),
                    _ => {
                        self.note(Status::info(format!(
                            "{} signals match `{}` — choose the one to add",
                            candidates.len(),
                            click.word
                        )));
                        self.apply(Action::PickerOpened {
                            title: format!(
                                "{} signals match `{}` — add one",
                                candidates.len(),
                                click.word
                            ),
                            paths: candidates,
                        });
                    }
                }
                false
            }
            Msg::AddSelectionToWave(selection) => {
                // Verdi's block add: every identifier the selected code block
                // mentions, resolved through the same lookup a click takes
                // (`candidates_at`), then added as ONE rows batch. A name the
                // design answers ambiguously is not guessed: the status line
                // names it and points at the click that opens the picker.
                let found = selection_add::clicks(&selection);
                if found.clicks.is_empty() {
                    self.note(Status::info(selection_add::nothing_added(
                        &selection,
                        &found,
                        &selection_add::SelectionPlan::default(),
                    )));
                    return false;
                }
                let plan = selection_add::plan(&found.clicks, |click| self.candidates_at(click));
                if plan.is_empty() {
                    self.note(Status::info(selection_add::nothing_added(
                        &selection, &found, &plan,
                    )));
                    return false;
                }
                if plan.paths.len() > crate::state::bulk::BULK_ADD_CAP {
                    self.note(Status::error(format!(
                        "the selection resolves to {} signals — more than the {} rows one batch can add; select a smaller block",
                        plan.paths.len(),
                        crate::state::bulk::BULK_ADD_CAP
                    )));
                    return false;
                }
                self.add_signal_batch(&plan.paths, "selection");
                // the batch already said how many rows it added; append what
                // the selection contributed besides rows, so an ambiguous or
                // unknown name is never silent. A refusal keeps its error line.
                if let Some(status) = self.store.select(state::select::status) {
                    if !status.is_error() {
                        let mut text =
                            format!("{} — {}", status.text, selection_add::success_note(&plan));
                        if found.truncated {
                            text.push_str(
                                " (the selection was too large: only its first part was scanned)",
                            );
                        }
                        self.note(Status::info(text));
                    }
                }
                false
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
                    // the picker was opened by Ctrl+Enter in the source (or a
                    // click, which only selects): add without moving the editor
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
                remote={self.remote_session()}
                onselect={link.callback(Msg::WaveSelect)}
                onjump={link.callback(Msg::WaveJump)}
                onlocate={link.callback(Msg::Locate)} />
        }
    }

    /// The full-window "drop to load" overlay while a file drag is over the app.
    ///
    /// The browser build learns it from the file loader's window listeners, the
    /// desktop build from the webview's native drag-drop events; both feed
    /// `drop_active`. It lives here rather than in the loader because the loader
    /// sits in the sidebar, and a fixed overlay inside a hidden sidebar would
    /// not paint.
    fn drop_overlay(&self) -> Html {
        if self.drop_active {
            crate::file_load::drop_overlay_html()
        } else {
            html! {}
        }
    }

    /// "Add selected (N)": bulk-add every signal of the module-tree selection.
    fn bulk_add_selected(&self) {
        if self.tree_selection.is_empty() {
            return;
        }
        self.bulk_add_scopes(&self.tree_selection.paths);
    }

    /// Bulk-add every signal of `scopes` (and their subtrees) in ONE batch.
    ///
    /// The paths come from the store's sidebar tree; the shared batch path is
    /// [`Self::add_signal_batch`].
    fn bulk_add_scopes(&self, scopes: &[String]) {
        // the sidebar may be showing the native hierarchy (desktop build)
        let Some(tree) = self.sidebar_tree() else {
            self.note(Status::error(
                "load sources or a dump before adding signals".to_string(),
            ));
            return;
        };
        let paths = crate::state::bulk::signals_under_many(&tree, scopes);
        if paths.is_empty() {
            self.note(Status::info(
                "no signals in the selected scopes".to_string(),
            ));
            return;
        }
        self.add_signal_batch(&paths, "modules");
    }

    /// Add a batch of hierarchical signal paths to the wave view in ONE
    /// `RowsAdded`.
    ///
    /// Shared by the module tree's bulk add and the signal list's explicit "Add
    /// selected (N)": each path is resolved exactly like a single add (the dump
    /// wins for the width, the design fills in drivers/loads, the desktop build
    /// resolves against the native session), then the whole batch goes to the
    /// reducer once — dedupe, remembered display settings, one rows-version bump
    /// and one session write. `subject` names the user's selection in the status
    /// lines ("modules"/"signals") so a refusal points at the list to narrow.
    fn add_signal_batch(&self, paths: &[String], subject: &str) {
        // a selection never repeats a path, but a scope caller can (a selected
        // parent and a selected child): dedupe in first-seen order first
        let mut unique: Vec<String> = Vec::with_capacity(paths.len());
        let mut seen = std::collections::HashSet::with_capacity(paths.len());
        for path in paths {
            if seen.insert(path.as_str()) {
                unique.push(path.clone());
            }
        }
        if unique.is_empty() {
            self.note(Status::error("no signals selected".to_string()));
            return;
        }
        if unique.len() > crate::state::bulk::BULK_ADD_CAP {
            self.note(Status::error(format!(
                "too many signals to add at once: {} (limit {}) — select fewer {subject}",
                unique.len(),
                crate::state::bulk::BULK_ADD_CAP
            )));
            return;
        }
        // the desktop build draws native dumps without a wasm handle: resolve the
        // batch against the native hierarchy instead
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            let mut rows: Vec<Row> = Vec::with_capacity(unique.len());
            let mut unresolved = 0usize;
            for path in &unique {
                match session.signal_of_path(path) {
                    Some(native) => rows.push(Row {
                        path: path.clone(),
                        name: path.clone(),
                        width: native.width.max(1),
                        setting: state::Settings::new(),
                        chosen: false,
                        load: Vec::new(),
                        drive: Vec::new(),
                        bit: None,
                    }),
                    None => unresolved += 1,
                }
            }
            if rows.is_empty() {
                self.note(Status::error(format!(
                    "none of the {unresolved} selected signals are in the native dump"
                )));
                return;
            }
            let first_added = rows.first().map(|row| row.path.clone());
            self.apply(Action::RowsAdded { rows });
            if let Some(first) = first_added {
                self.apply(Action::FocusChanged {
                    focus: Focus {
                        signal: Some(first),
                        instance: self.focus.instance.clone(),
                    },
                });
            }
            if unresolved > 0 {
                self.note(Status::info(format!(
                    "added the selected signals from the native dump ({unresolved} not resolvable)"
                )));
            }
            return;
        }
        let dump = self.dump();
        let mut rows: Vec<Row> = Vec::with_capacity(unique.len());
        let mut unresolved = 0usize;
        for path in &unique {
            let row_path = match &dump {
                Some(waveform) => {
                    // the dump is the source of values: a path it cannot resolve
                    // would draw nothing, so it is skipped (and counted)
                    let candidates = match_dump_candidates(waveform.waveform(), path);
                    if candidates.len() != 1 {
                        unresolved += 1;
                        continue;
                    }
                    candidates[0].clone()
                }
                None => path.clone(),
            };
            let signal = self.signal_for_path(&row_path);
            rows.push(Row {
                path: row_path.clone(),
                name: row_path,
                width: signal.size,
                setting: state::Settings::new(),
                chosen: false,
                load: signal.load,
                drive: signal.drive,
                bit: None,
            });
        }
        if rows.is_empty() {
            self.note(Status::error(format!(
                "none of the {unresolved} selected signals are in the loaded dump"
            )));
            return;
        }
        // the first added row becomes the inspector's signal: the batch gives
        // the same feedback a single add did (the status line keeps reporting
        // the batch itself)
        let first_added = rows.first().map(|row| row.path.clone());
        self.apply(Action::RowsAdded { rows });
        if let Some(first) = first_added {
            self.apply(Action::FocusChanged {
                focus: Focus {
                    signal: Some(first),
                    instance: self.focus.instance.clone(),
                },
            });
        }
        if unresolved > 0 {
            // keep the reducer's summary and say how many paths the dump could
            // not resolve, instead of silently dropping them
            if let Some(status) = self.store.select(state::select::status) {
                self.note(Status::info(format!(
                    "{} — {unresolved} not in the loaded dump",
                    status.text
                )));
            }
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
        // A native dump knows its own widths and has no wasm handle: resolve
        // against it first.
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            if let Some(native) = session.signal_of_path(path) {
                signal.size = native.width.max(1);
                return signal;
            }
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
    /// gestures (the sidebar). The source pane's Ctrl+Enter adds the row and
    /// leaves the editor where it is; a plain click in the source only selects,
    /// it never adds.
    fn add_path(&mut self, path: &str, show_declaration: bool) {
        // the desktop build may be drawing a native dump: it has no wasm handle
        // to match against, only the hierarchy's path -> signal table
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            match session.signal_of_path(path) {
                Some(native) => {
                    self.store.dispatch(Action::RowAdded {
                        path: path.to_string(),
                        name: path.to_string(),
                        width: native.width.max(1),
                        load: Vec::new(),
                        drive: Vec::new(),
                        bit: None,
                    });
                    self.apply(Action::FocusChanged {
                        focus: Focus {
                            signal: Some(path.to_string()),
                            instance: self.focus.instance.clone(),
                        },
                    });
                    self.note(Status::info(format!("added {path} to the waveform")));
                }
                None => self.note(Status::error(format!("`{path}` is not in the native dump"))),
            }
            return;
        }
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

    /// Adopt a native dump: close the previous one, clear the rows the wasm dump
    /// had, install the hierarchy and say which mode the window is in.
    #[cfg(feature = "tauri")]
    fn adopt_remote(&mut self, session: crate::remote::RemoteSession) {
        // Wave 19: the native backend is one-dump-at-a-time, so this dump
        // replaces the previous native one; the replaced path is not open any
        // more and leaves the desktop reopen list.
        self.forget_native_path();
        self.close_remote();
        // the rows of the dump that was open would resolve to signal indices of
        // the native dump: start empty, like a fresh session
        let rows = self.store.select(state::select::rows).items.len();
        if rows > 0 {
            self.apply(Action::RowsRemoved {
                indices: (0..rows).collect(),
            });
        }
        let status = session.status_text();
        let id = session.id();
        let path = session.dump().path.clone();
        self.remote = Some(session);
        // and this dump is open now: its absolute path is the only handle a
        // desktop reload has for it (a native dump has no store tab)
        self.native_path = Some(path.clone());
        self.remember_dump_path(Some(&path));
        crate::remote::set_native_active(true);
        // the backend accounting (checkable in __waveDebug.memory.native)
        spawn_local(async move {
            let dump = crate::remote::memory_stats(id).await.ok();
            let process = crate::remote::process_memory().await.ok();
            if dump.is_some() || process.is_some() {
                crate::remote::set_native_memory(Some(crate::remote::NativeMemorySnapshot {
                    dump,
                    process,
                }));
            }
        });
        // the selection held paths of the previous tree
        self.apply(Action::TreeSelectionCleared);
        self.note(Status::info(status));
    }

    /// Release the native dump: close_dump frees the mapping on the Rust side.
    /// (A no-op in the browser build: there is nothing native to close.)
    fn close_remote(&mut self) {
        #[cfg(feature = "tauri")]
        crate::remote::set_native_active(false);
        #[cfg(feature = "tauri")]
        crate::remote::set_native_memory(None);
        #[cfg(feature = "tauri")]
        if let Some(session) = self.remote.take() {
            let id = session.id();
            spawn_local(async move {
                let _ = crate::remote::close(id).await;
            });
        }
    }

    // --- Wave 19: the desktop reopen list ------------------------------------

    /// Remember `path` as one of the dumps the desktop build has open.
    ///
    /// The list is what a webview reload reopens (see
    /// [`crate::state::persist::DesktopPaths`] for the format and the cap).
    /// Writing is throttled by the list itself: `open` answers whether the list
    /// changed, so re-opening the newest dump writes nothing.
    #[cfg(feature = "tauri")]
    fn remember_dump_path(&mut self, path: Option<&str>) {
        let Some(path) = path else {
            return;
        };
        if self.desktop_paths.open(path) {
            self.save_desktop_paths();
        }
    }

    /// Forget one dump path (its tab was closed, or the file disappeared).
    #[cfg(feature = "tauri")]
    fn forget_dump_path(&mut self, path: &str) {
        if self.desktop_paths.close(path) {
            self.save_desktop_paths();
        }
    }

    /// The native dump that is open is being replaced: its path is not open any
    /// more, so it leaves the reopen list.
    #[cfg(feature = "tauri")]
    fn forget_native_path(&mut self) {
        if let Some(path) = self.native_path.take() {
            self.forget_dump_path(&path);
        }
    }

    /// Write the reopen list through the persister (no-op when the browser
    /// denies `localStorage`).
    #[cfg(feature = "tauri")]
    fn save_desktop_paths(&self) {
        if let Some(persister) = &self.desktop_persister {
            persister.save_desktop_paths(&self.desktop_paths);
        }
    }

    /// Remember one freshly opened dump: its absolute path (desktop transports
    /// only; the browser reports none) and the tab identity that closing it
    /// later needs.
    #[cfg(feature = "tauri")]
    fn remember_opened_dump(&mut self, path: Option<&str>, id: &DumpId) {
        let Some(path) = path else {
            return;
        };
        // re-opening the same tab replaces its mapping instead of duplicating it
        self.tab_paths.retain(|(tab, _)| tab != id);
        self.tab_paths.push((id.clone(), path.to_string()));
        if self.desktop_paths.open(path) {
            self.save_desktop_paths();
        }
    }

    /// Drop the path of the wasm tab at `index` (it is about to be closed).
    #[cfg(feature = "tauri")]
    fn forget_closed_tab(&mut self, index: usize) {
        let id = self
            .store
            .select(|state| state.dumps.items.get(index).map(|entry| entry.id.clone()));
        let Some(id) = id else {
            return;
        };
        if let Some(position) = self.tab_paths.iter().position(|(tab, _)| *tab == id) {
            let (_, path) = self.tab_paths.remove(position);
            self.forget_dump_path(&path);
        }
    }

    // --- Wave 19: reopening the dumps of the previous desktop page -----------

    /// Start reopening the dumps a previous desktop page had open.
    #[cfg(feature = "tauri")]
    fn start_restore(&mut self, ctx: &Context<Self>, paths: Vec<String>) {
        if paths.is_empty() {
            return;
        }
        self.restore = Some(DesktopRestore {
            total: paths.len(),
            pending: paths.into(),
            opened: 0,
            failed: Vec::new(),
            active: None,
        });
        self.restore_advance(ctx);
    }

    /// Open the next remembered dump (one at a time) through the normal open
    /// path: the native backend decides (native for a large dump, the wasm
    /// reader for a small one) and the resulting [`Msg::RemoteReady`] /
    /// [`Msg::GetWaveform`] / [`Msg::Error`] continues the sequence.
    ///
    /// A path the native backend cannot even open (missing, unreadable, not a
    /// dump) is counted through [`Msg::DesktopRestoreFailed`] and the sequence
    /// moves on; nothing is reported per file, the end of the restore aggregates.
    #[cfg(feature = "tauri")]
    fn restore_advance(&mut self, ctx: &Context<Self>) {
        let Some(restore) = self.restore.as_mut() else {
            return;
        };
        let Some(path) = restore.pending.pop_front() else {
            self.finish_restore();
            return;
        };
        restore.active = Some(path.clone());
        let link = ctx.link().clone();
        spawn_local(async move {
            match crate::remote::try_open(&path).await {
                Ok(Some(session)) => link.send_message(Msg::RemoteReady(Box::new(session))),
                Ok(None) => link.send_message(Msg::RemoteFallback(path)),
                Err(error) => link.send_message(Msg::DesktopRestoreFailed(path, error)),
            }
        });
    }

    /// Count the dump currently being reopened as failed; the sequence moves on
    /// with [`Self::restore_advance`].
    #[cfg(feature = "tauri")]
    fn restore_failed_current(&mut self, message: String) {
        let Some(restore) = self.restore.as_mut() else {
            return;
        };
        if let Some(path) = restore.active.take() {
            restore.failed.push((path, message));
        }
    }

    /// The end of one restore: report the failures once and drop them from the
    /// list (a missing file must not produce an error line on every start).
    #[cfg(feature = "tauri")]
    fn finish_restore(&mut self) {
        let Some(restore) = self.restore.take() else {
            return;
        };
        if restore.failed.is_empty() {
            // everything came back: the last "waveform loaded" line is enough
            return;
        }
        for (path, _) in &restore.failed {
            self.forget_dump_path(path);
        }
        let (first_path, first_error) = &restore.failed[0];
        let more = match restore.failed.len() {
            1 => String::new(),
            count => format!(" (+{} more)", count - 1),
        };
        // one actionable line: how much came back, which file failed first, and
        // what to do. Never one error per remembered dump.
        self.note(Status::error(format!(
            "reopened {} of {} dump(s) from the last session; could not open {}: {} — {}{more}; they were removed from the reopen list — open them again from \"Open file…\" or by dropping them",
            restore.opened,
            restore.total,
            restore.failed.len(),
            first_path,
            first_error,
        )));
    }

    // --- Wave 19: SpinalHDL companion RTL -------------------------------------

    /// Look for the SpinalHDL companion RTL of a dump and open what is found
    /// (desktop build only: the browser has no filesystem to scan).
    ///
    /// The scan itself is a backend command (see `src-tauri/src/companion.rs`):
    /// the dump's own directory and up to two ancestors are checked for an
    /// `rtl` (or `generated`/`verilog`) sub-directory, whose `.v`/`.sv` files
    /// are collected flat or one level deep, sorted, de-duplicated and capped.
    /// Each file is read through `choose_verilog_absolute` and handed to the
    /// normal [`Action::SourceOpened`] flow, exactly as if the user had dropped
    /// it; files already open (by absolute path or by file name) are skipped, so
    /// re-running after a reload does not duplicate anything.
    ///
    /// Discovery is best effort by design: no path (the browser build), an empty
    /// answer (no companion directory), an unreadable file or a failed command
    /// just means nothing is loaded — a dump open never fails because of it. A
    /// generation counter drops the answer of a dump that was replaced while the
    /// scan/reads were running.
    #[cfg(feature = "tauri")]
    fn discover_companion_rtl(&self, ctx: &Context<Self>, dump_path: Option<String>) {
        let Some(dump_path) = dump_path else {
            return;
        };
        let generation = self.companion_gen.get().wrapping_add(1);
        self.companion_gen.set(generation);
        let link = ctx.link().clone();
        let store = self.store.clone();
        spawn_local(async move {
            #[derive(Serialize)]
            struct CompanionArgs {
                path: String,
            }
            let Ok(args) = to_value(&CompanionArgs { path: dump_path }) else {
                return;
            };
            let sources: Vec<String> =
                match from_value(invoke("dump_companion_sources", args).await) {
                    Ok(sources) => sources,
                    // a failed command means "no companion sources", never an
                    // error: the dump itself is already open
                    Err(_) => return,
                };
            if sources.is_empty() {
                return;
            }
            // files that are already open: a dropped source is named by its
            // absolute path, a dialog one by its file name, so both are checked
            let open = store.select(|state| {
                state
                    .sources
                    .items
                    .iter()
                    .map(|source| source.name.clone())
                    .collect::<Vec<_>>()
            });
            #[derive(Serialize)]
            struct SourceArgs {
                path: String,
            }
            let mut files: Vec<(String, String)> = Vec::new();
            let mut dir = String::new();
            for source in sources {
                let name = crate::state::persist::desktop_file_name(&source);
                if open.iter().any(|open| {
                    open == &source || crate::state::persist::desktop_file_name(open) == name
                }) {
                    continue;
                }
                let Ok(args) = to_value(&SourceArgs {
                    path: source.clone(),
                }) else {
                    continue;
                };
                let text: String =
                    from_value(invoke("choose_verilog_absolute", args).await).unwrap_or_default();
                if text.is_empty() {
                    continue;
                }
                if dir.is_empty() {
                    dir = crate::state::persist::desktop_parent_dir(&source).to_string();
                }
                files.push((source, text));
            }
            if files.is_empty() {
                return;
            }
            link.send_message(Msg::CompanionRtlLoaded {
                generation,
                dir,
                files,
            });
        });
    }

    /// The native session handed to the wave pane (none in the browser build).
    fn remote_session(&self) -> Option<crate::remote::RemoteSession> {
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            return Some(session.clone());
        }
        None
    }

    /// The sidebar tree: the native hierarchy when a native dump is open, else
    /// the store's dump/design tree.
    fn sidebar_tree(&self) -> Option<SidebarTree> {
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            return Some(session.tree());
        }
        self.tree.clone()
    }

    /// A readable marker of the mode: the desktop build draws native dumps
    /// read-only over IPC, and knowing that is half of trusting the window.
    fn remote_badge(&self) -> Html {
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            let dump = session.dump();
            let title = format!(
                "{} — {} MB, {} signals in {} scopes, mapped {} MB (native backend, read-only)",
                dump.path,
                dump.size_mb(),
                dump.signal_count,
                dump.scope_count,
                dump.mapped_bytes / (1024 * 1024)
            );
            return html! {
                <span class="dock-tab native-badge" title={title}>
                    {"native (read-only, mmap)"}
                </span>
            };
        }
        html! {}
    }

    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html! {
            <FileLoad
                ongetwaveform={ctx.link().callback(Msg::GetWaveform)}
                ongetverilog={ctx.link().callback(Msg::GetVerilog)}
                onerror={ctx.link().callback(Msg::Error)}
                onstatus={ctx.link().callback(Msg::Note)}
                onloadreport={ctx.link().callback(Msg::LoadReport)}
                ondragstate={ctx.link().callback(Msg::DropDragActive)} />
        }
    }
}
