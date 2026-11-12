use std::cell::RefCell;
use std::rc::Rc;

#[cfg(feature = "tauri")]
use wasm_bindgen_futures::spawn_local;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wave_insight_lib::waveform::{Change, DecodedSignal, ShowType, TimeRange};
use yew::prelude::*;

use crate::state::DumpHandle;
use std::cell::Cell;

use crate::nav::ValueFormat;
use crate::state::{
    self, select, Cursors, Dialog, RowsSlice, Status, Store, Subscription, View, Viewport,
};

use super::canvas;
use super::ctrl::Ctrl;
use super::export::{self, ExportFormat, ExportScope};
use super::geometry::{self, content_height, visible_rows, Time, ROW_HEIGHT};
use super::memory;
use super::settings::Settings;
use super::signal::SignalName;
use super::time::{build_axis, Time as TimeBar};
use super::util::signal_things::{derive_bit, leaf_name, RowValues, SignalItem, SignalThings};
use super::windowed::{self, WindowRequest};

/// Rows around the viewport that are kept decoded once they were shown, so a
/// scroll back and forth does not decode again. In row units.
///
/// The decoded budget itself is global now ([~memory::DECODE_BUDGET_BYTES~],
/// split over the open dumps): one budget for every dump and the split pane,
/// instead of 64 MB per renderer instance.
const KEEP_ROWS: usize = geometry::PREFETCH_ROWS * 4;
/// Rows outside the viewport that are rendered into the DOM name column, so a
/// fast scroll does not pop.
const NAME_OVERSCAN: usize = 4;
/// Change budget of one export row / one value-search row.
///
/// Drawing is windowed, but export and find are whole-history operations: they
/// are refused with an actionable message instead of materialising a history
/// that would not fit in the wasm memory.
const HISTORY_CHANGE_LIMIT: usize = 1_000_000;
/// Width of the label column (signal name + cursor value), in css pixels. Also
/// what a double click on the column's grip restores. Must agree with
/// `--names-w` in `styles.css`: the inline style this component writes always
/// wins, so a different number here would make the stylesheet a lie.
const DEFAULT_NAMES_W: f64 = 220.0;
/// The range the label-column splitter may drag to, in css pixels.
const NAMES_W_MIN: f64 = 200.0;
const NAMES_W_MAX: f64 = 640.0;

/// Share of the label column the cursor value column(s) may take together, so
/// the signal name always keeps the majority of the cell.
const VALUE_COLUMN_SHARE: f64 = 0.40;
/// How many characters the widest fixed-point reading can print: a 64-bit
/// two's-complement value scaled with one decimal (`-9223372036854775808.0`).
const VALUE_FIXED_POINT_CHARS: usize = 21;
/// Pixel estimate of one value-font character (the 10.5 px monospace slot).
/// Only the 40 % cap is derived from it; the drawn width is `ch` based (see
/// `styles.css`), so the estimate being a little generous is harmless.
const VALUE_CHAR_PX: f64 = 6.4;
/// Padding, separator and slack of one `.wave-value` column, in css pixels: the
/// `11px` in the `.wave-value` flex basis in `styles.css`.
const VALUE_COLUMN_PAD_PX: f64 = 11.0;
/// Narrowest value column, in characters: one digit plus a little air.
const VALUE_COLUMN_MIN_CHARS: usize = 2;
/// Largest horizontal pan one wheel event may apply, in css pixels: a single
/// notch must not jump the whole trace.
const WHEEL_PAN_MAX_PX: f64 = 240.0;
/// How a wheel delta reported in lines / pages is converted to css pixels
/// (`WheelEvent::delta_mode` 1 / 2, matching the DOM constants).
const WHEEL_LINE_PX: f64 = 16.0;
const WHEEL_PAGE_PX: f64 = 400.0;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct WaveShowProps {
    /// The app store: the rows (order, settings, selection) are subscribed from
    /// here, and every row change is dispatched back as an action (R2 step 2 B1).
    /// The opened dump lives in the store too (R2 step 3), so the pane takes no
    /// waveform prop any more.
    pub store: Store,

    /// The selected signal, for the driver/load panel.
    #[prop_or_default]
    pub onselect: Callback<Option<String>>,
    /// Open the declaration of one signal in the source pane.
    #[prop_or_default]
    pub onjump: Callback<String>,
    /// The settings dialog asked for a driver/load: `(load?, index, path)`.
    /// The app resolves the span and decides between "jump to the one target"
    /// and "the several are listed in the inspector".
    #[prop_or_default]
    pub onlocate: Callback<(bool, usize, String)>,

    /// The wave frame shows the other tab (loads/drivers): the pane is hidden
    /// with `display:none` but stays mounted, so the zoom, the scroll position
    /// and the decoded rows survive the switch.
    #[prop_or_default]
    pub hidden: bool,

    /// Native (desktop) dump: when set, the rows are drawn from bounded IPC
    /// windows instead of a wasm waveform (read-only MVP, tauri build only).
    #[prop_or_default]
    pub remote: Option<crate::remote::RemoteSession>,
}

pub enum Msg {
    /// The store's rows changed (add/remove/reorder/settings/selection).
    Rows(RowsSlice),
    /// The store's settings-dialog state changed (open/closed).
    Dialog(Option<Dialog>),
    /// The committed viewport/cursors changed (this is where a gesture lands).
    Viewport(Viewport),
    /// The opened dump's time range changed.
    TimeRange(Option<TimeRange>),
    /// The opened dump changed (the store owns the handle).
    Dump(Option<DumpHandle>),
    Wheel(WheelEvent),
    /// the row list was scrolled
    Scroll,
    /// the label-column splitter was grabbed
    NamesDown(MouseEvent),
    /// the splitter moved (tier A: writes the css variable, no dispatch)
    NamesMove(MouseEvent),
    NamesUp,
    ShowMenu(usize),
    /// Expand the bits of a bus row, or collapse them again (the settings
    /// dialog's "expand bits" button).
    ToggleBits(usize),
    SetSignal((bool, Settings)),
    DeleteSig,
    KeyEvent(u32),
    MouseDown(MouseEvent),
    MouseMove(MouseEvent),
    MouseUp(MouseEvent),
    NameClick(usize),
    /// the little arrow on a name cell: open its declaration
    Jump(usize),
    /// the settings dialog asked for a load (`true`) or a driver (`false`)
    Locate((bool, usize)),
    /// the layout moved, everything has to be measured again
    Resize,
    /// rows were decoded after the last render: the value column has values to
    /// show now, so re-render the name column once (see `rendered`)
    ValuesReady,
    /// The settings dialog asked for a download: one row, or every row.
    Export((ExportFormat, ExportScope)),

    /// Native windows arrived: (epoch, request, show, row/slot/width, answers).
    #[cfg(feature = "tauri")]
    RemoteWindows(
        u32,
        WindowRequest,
        String,
        Vec<(usize, usize, usize)>,
        Vec<crate::remote::WindowDto>,
    ),
    /// The native backend refused a query.
    #[cfg(feature = "tauri")]
    RemoteFailed(u32, String),
    /// One rendered value at the cursor arrived from dump_value_at.
    #[cfg(feature = "tauri")]
    RemoteCursorValue(usize, i32, String),
}

/// Geometry of the wave area, measured from the dom.
///
/// It is measured instead of guessed, so it stays correct with a fixed width
/// sidebar, a zoomed page, a scrollbar appearing/disappearing or a high dpi
/// screen. `surface_*` is the wave canvas, `body_top` is the top of the
/// scrolling row list. The canvas itself is placed by css (`--names-w`,
/// `--timebar-h`, `--scrollbar-w`), so there is nothing to position here.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Metrics {
    surface_w: f64,
    surface_h: f64,
    surface_left: f64,
    body_top: f64,
    dpr: f64,
}

impl Metrics {
    fn is_usable(&self) -> bool {
        self.surface_w > 1.0 && self.surface_h > 1.0
    }

    fn approximately(&self, other: &Metrics) -> bool {
        (self.surface_w - other.surface_w).abs() <= 0.5
            && (self.surface_h - other.surface_h).abs() <= 0.5
            && (self.surface_left - other.surface_left).abs() <= 0.5
            && (self.body_top - other.body_top).abs() <= 0.5
            && (self.dpr - other.dpr).abs() <= 0.001
    }
}

/// Which cursor a press that landed on a cursor line is dragging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorDrag {
    One,
    Two,
}

pub struct WaveShow {
    /// The app store the rows are read from and dispatched to.
    store: Store,
    signal_things: SignalThings,
    /// The dump the rows are decoded from.
    waveform: Option<DumpHandle>,
    /// The rows the store owns, as the last snapshot the pane saw.
    rows: RowsSlice,
    /// The committed cursors, mirrored here for drawing.
    cursors: Cursors,
    /// Cursors being dragged: a local preview, committed on mouseup (tier A).
    cursor_preview: Option<Cursors>,
    /// Which cursor the current press grabbed, if the press drags one.
    cursor_drag: Option<CursorDrag>,
    /// The value-change edges on screen when the press started; cursors snap to
    /// these. Collected once per gesture, never per mouse move.
    snap_edges: Vec<Time>,
    /// Rows were decoded after the last render, so the value column has new
    /// cell values: one extra render is sent from `rendered`.
    values_need_render: bool,
    /// The opened dump's range, mirrored here for the ruler and the fit zoom.
    time_range: Option<TimeRange>,
    /// Kept so the subscriptions live exactly as long as the component.
    _rows_sub: Subscription,
    _viewport_sub: Subscription,
    _dump_sub: Subscription,
    _waveform_sub: Subscription,
    _layout_sub: Subscription,
    /// The pane's own re-render count, published for the tier-A QA gate.
    pane_render_count: Cell<u32>,

    /// Zoom/pan state of the wave body.
    view: View,
    /// `true` once the fit-to-trace zoom was applied for this dump.
    fitted: bool,
    /// `true` once the computed fit zoom was also committed to the store.
    ///
    /// The fit is applied locally in [`Self::redraw`] (it needs the measured
    /// width), but the store owns the committed viewport: without this commit a
    /// later cursor commit would hand back the store's default view and reset
    /// the zoom, and the time bar would draw a ruler for a view nobody sees.
    fit_committed: bool,
    /// Vertical scroll offset of the row list, in css pixels.
    scroll_top: f64,

    /// Width of the label column, dragged by the splitter at its right edge.
    names_w: f64,
    /// Splitter drag: pointer x and width at the press, plus the live width.
    names_drag: Option<(f64, f64)>,
    names_preview: Cell<f64>,

    /// The settings dialog, mirrored from the store (which owns whether it is
    /// open and for which rows).
    dialog: Option<Dialog>,
    /// Kept so the subscription lives exactly as long as the component.
    _dialog_sub: Subscription,

    key_press: Rc<RefCell<Vec<bool>>>,

    /// Mouse drag in the wave area: `mouse_start_x` is where the button went
    /// down, `mouse_current_x` follows the pointer for the selection band.
    mouse_press: bool,
    mouse_start_x: f64,
    mouse_current_x: f64,

    name_choose_valid: bool,
    name_choose: usize,
    /// where the mouse went down on the names column, so a plain click (which
    /// also fires mousedown/mouseup) is not mistaken for a reorder drag
    name_start_y: f64,

    /// what the last `view` was measured with, so `rendered` can notice a moved layout
    used: RefCell<Metrics>,
    /// rows whose window was fetched by the last redraw, for the debug hook
    last_decoded: usize,
    /// bytes the cached row windows hold (this pane's drawing cache, budgeted
    /// through the global decode budget)
    window_bytes: usize,
    draws: u64,

    /// the listeners are created once and dropped in `destroy`; forgetting them
    /// on every render leaked one set of them per render
    keydown: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    keyup: Option<Closure<dyn FnMut(web_sys::KeyboardEvent)>>,
    onresize: Option<Closure<dyn FnMut(JsValue)>>,
    /// Repaints the canvas whenever its box changes, including the tier-A
    /// drags that write css variables without a Yew render.
    resize_observer: Option<web_sys::ResizeObserver>,
    resize_callback: Option<Closure<dyn FnMut()>>,
    /// Last `hidden` prop, so `changed` knows whether the *view* has to re-run.
    hidden: bool,
    /// Whether the label column shows the full instance path. The layout slice
    /// owns the option (it is persisted with the window layout); it is mirrored
    /// here so the view never borrows the store.
    show_path: bool,

    /// The native dump being drawn read-only, when the app opened one.
    #[cfg(feature = "tauri")]
    remote: Option<crate::remote::RemoteSession>,
    /// Bumped per native window request; an answer from an older request is
    /// dropped (the view moved on while it was in flight).
    #[cfg(feature = "tauri")]
    remote_epoch: u32,
    /// One error line per native backend failure, not one per redraw.
    #[cfg(feature = "tauri")]
    remote_error: bool,
    /// Rendered values at the cursor times, keyed by (row, time): the value
    /// column shows them when a cursor is outside the fetched window.
    #[cfg(feature = "tauri")]
    native_cursor: crate::remote::CursorCache,
    /// `(row, time)` pairs whose dump_value_at is in flight (no duplicates).
    #[cfg(feature = "tauri")]
    native_cursor_inflight: std::collections::HashSet<(usize, i32)>,
    /// The component's link: async IPC answers arrive after the update.
    #[cfg(feature = "tauri")]
    link: Option<yew::html::Scope<Self>>,
}

impl Component for WaveShow {
    type Message = Msg;
    type Properties = WaveShowProps;

    fn create(ctx: &Context<Self>) -> Self {
        let store = ctx.props().store.clone();
        let rows_sub = store.subscribe(select::rows, ctx.link().callback(Msg::Rows));
        let viewport_sub = store.subscribe(select::viewport, ctx.link().callback(Msg::Viewport));
        let dump_sub = store.subscribe(select::time_range, ctx.link().callback(Msg::TimeRange));
        // the dump handle is store state now: no prop, one subscription
        let waveform_sub = store.subscribe(select::dump_waveform, ctx.link().callback(Msg::Dump));
        // The pane's height depends on the editor ratio: when the source pane
        // expands (for example a dump was loaded before any source), the wave
        // pane shrinks without any props changing, so without this subscription
        // the canvas keeps its old backing store size and the drawing is drawn
        // squashed until something else forces a redraw.
        let layout_sub = store.subscribe(select::layout, ctx.link().callback(|_| Msg::Resize));
        // the settings dialog is store state now (R2 step 3b)
        let dialog = store.select(select::dialog);
        let dialog_sub = store.subscribe(select::dialog, ctx.link().callback(Msg::Dialog));
        let mut this = Self {
            store: store.clone(),
            signal_things: SignalThings::new(),
            waveform: store.select(select::dump_waveform),
            rows: store.select(select::rows),
            cursors: store.select(select::cursors),
            cursor_preview: None,
            cursor_drag: None,
            snap_edges: Vec::new(),
            values_need_render: false,
            time_range: store.select(select::time_range),
            _rows_sub: rows_sub,
            _viewport_sub: viewport_sub,
            _dump_sub: dump_sub,
            _waveform_sub: waveform_sub,
            _layout_sub: layout_sub,
            pane_render_count: Cell::new(0),

            view: View::default(),
            fitted: false,
            fit_committed: false,
            scroll_top: 0.0,

            dialog,
            _dialog_sub: dialog_sub,

            key_press: Rc::new(RefCell::new(vec![false; 256])),

            mouse_press: false,
            mouse_start_x: 0.0,
            mouse_current_x: 0.0,

            name_choose_valid: false,
            name_choose: 0,
            name_start_y: 0.0,

            used: RefCell::new(Metrics::default()),
            last_decoded: 0,
            window_bytes: 0,
            draws: 0,
            names_w: DEFAULT_NAMES_W,
            names_drag: None,
            names_preview: Cell::new(DEFAULT_NAMES_W),

            keydown: None,
            keyup: None,
            onresize: None,
            resize_observer: None,
            resize_callback: None,
            hidden: ctx.props().hidden,
            show_path: store.select(select::layout).show_path,
            #[cfg(feature = "tauri")]
            remote: ctx.props().remote.clone(),
            #[cfg(feature = "tauri")]
            remote_epoch: 0,
            #[cfg(feature = "tauri")]
            remote_error: false,
            #[cfg(feature = "tauri")]
            native_cursor: crate::remote::CursorCache::new(),
            #[cfg(feature = "tauri")]
            native_cursor_inflight: std::collections::HashSet::new(),
            #[cfg(feature = "tauri")]
            link: Some(ctx.link().clone()),
        };
        this.sync_rows();
        this.attach_listeners(ctx);
        this
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let changed = self.handle(ctx, msg);
        if changed {
            self.pane_render_count
                .set(self.pane_render_count.get().wrapping_add(1));
        }
        changed
    }

    fn changed(&mut self, ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
        // every slice the pane reads (rows, viewport, dump, time range) arrives
        // through its own subscription, so a prop change alone never re-renders.
        // The pane's *size*, however, changes when the source pane grows (the
        // editor expands as soon as the first file is opened) and that arrives
        // as neither a prop nor a slice this component subscribes to. Repainting
        // here keeps the canvas backing store in step with its css size; without
        // it the whole trace is drawn squashed until something else forces a
        // redraw.
        self.redraw();
        // `false` means "props changed but the view is unchanged", which is the
        // usual case here. The tab strip is the exception: `hidden` is rendered
        // into the root class, so the view *must* re-run or the pane stays
        // hidden after switching back from the loads/drivers view.
        let hidden = ctx.props().hidden;
        let rerender = hidden != self.hidden;
        self.hidden = hidden;
        #[cfg(feature = "tauri")]
        if self.remote != ctx.props().remote {
            // a new native dump: drop the previous rows' windows and fit again
            self.remote = ctx.props().remote.clone();
            self.remote_epoch = self.remote_epoch.wrapping_add(1);
            self.remote_error = false;
            self.signal_things.refresh(None);
            self.native_cursor.clear();
            self.native_cursor_inflight.clear();
            self.fitted = false;
            self.fit_committed = false;
            self.scroll_top = 0.0;
        }
        rerender
    }

    fn rendered(&mut self, ctx: &Context<Self>, _first_render: bool) {
        // watch the canvas box: the horizontal splitter and the sidebar splitter
        // are tier-A (they write css variables straight to the dom), so a resize
        // can happen with no render at all; without this the backing store keeps
        // the old size and the trace is stretched over the new box
        self.observe_canvas(ctx);
        let measured = self.measure();
        if measured.is_usable() && !measured.approximately(&self.used.borrow()) {
            ctx.link().send_message(Msg::Resize);
        }
        *self.used.borrow_mut() = measured;
        // Publish *before* the repaint: `redraw` copies the counter into
        // `window.__waveDebug`, and publishing afterwards made the tier-A gate
        // see every render one repaint late (the count moved inside the next
        // gesture window instead of at its own commit).
        self.publish_pane_render_count();
        self.redraw();
        // the fit zoom is applied on the first real measurement; commit it once
        // so the store, the ruler and every later commit agree on the viewport
        if self.fitted && !self.fit_committed {
            self.fit_committed = true;
            self.dispatch(state::Action::ViewCommitted(self.view));
        }
        // Rows decoded by that redraw now have values for the value column. A
        // render happens *before* `rendered`, so the column would show blanks
        // after a scroll until the next unrelated render; ask for exactly one
        // more render (never one per mouse move).
        if self.values_need_render {
            self.values_need_render = false;
            ctx.link().send_message(Msg::ValuesReady);
        }
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let Some(observer) = self.resize_observer.take() {
            observer.disconnect();
        }
        self.resize_callback = None;
        if let Some(window) = web_sys::window() {
            if let Some(keydown) = self.keydown.as_ref() {
                let _ = window.remove_event_listener_with_callback(
                    "keydown",
                    keydown.as_ref().unchecked_ref(),
                );
            }
            if let Some(keyup) = self.keyup.as_ref() {
                let _ = window
                    .remove_event_listener_with_callback("keyup", keyup.as_ref().unchecked_ref());
            }
            if let Some(onresize) = self.onresize.as_ref() {
                let _ = window.remove_event_listener_with_callback(
                    "resize",
                    onresize.as_ref().unchecked_ref(),
                );
            }
        }
        self.keydown = None;
        self.keyup = None;
        self.onresize = None;
        // Wave 12: the pane is going away: whatever it decoded for the dump it
        // shows can be given back (the dump itself stays alive in the store, so
        // the parsed body is only released when no other pane draws it).
        if let Some(handle) = self.waveform.take() {
            handle.waveform().unload_all_signals();
            if !memory::is_drawn(&self.store, &handle) {
                handle.waveform().release_cache();
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let metrics = self.measure();
        let rows = self.signal_things.len();
        let (draw, _fetch) = visible_rows(self.scroll_top, metrics.surface_h, rows);
        let has_signal = rows > 0;

        //virtual name column: only the rows around the viewport exist in the dom
        let name_first = draw.start.saturating_sub(NAME_OVERSCAN);
        let name_last = (draw.end + NAME_OVERSCAN).min(rows);

        // The cursor value columns end the label cell (Verdi style: the value
        // sits right against the waveform). Their width is the widest value the
        // rows on screen can print, computed once per render: one shared width
        // keeps the two cursor columns aligned vertically, and sizing it from
        // the rows' formats rather than from decoded text means it does not
        // jump while the cursor moves (a drag only repaints the canvas; the view
        // renders once when the cursor commits). No cursor, no columns, no
        // reserved space.
        let value_columns =
            usize::from(self.cursors.one.is_some()) + usize::from(self.cursors.two.is_some());
        let value_chars = if value_columns == 0 {
            0
        } else {
            value_column_chars(
                self.widest_visible_value_chars(&draw),
                value_columns,
                self.names_w,
            )
        };

        let rows_height = format!("{:.1}px", content_height(rows));
        // the settings dialog can expand a bus into its bits
        let dialog_indices = self.dialog_indices();
        let bits_idx = dialog_indices.first().copied().unwrap_or(0);
        let (bit_width, bits_expanded) = self.bits_state(bits_idx);
        let dialog_row = dialog_indices
            .first()
            .and_then(|index| self.signal_things.iter().nth(*index));

        html! {
            <div class={if ctx.props().hidden {"wave-pane hidden"} else {"wave-pane"}} id="wave-pane"
                // The value column lives in the label area, so the shared label
                // width (used by the name cells, the canvas and the ruler) is
                // adjustable: the splitter writes it during the drag and it is
                // committed here on mouseup.
                style={format!(
                    "--names-w: {:.0}px;--wave-value-ch: {}",
                    self.names_w, value_chars
                )}>
                {if let Some(row) = &dialog_row {
                    html!{
                    <Ctrl name={row.name.clone()}
                        path={row.path.clone()}
                        setting={row.setting.clone()}
                        load={row.load.clone()}
                        drive={row.driver.clone()}
                        bits={bit_width}
                        expanded={bits_expanded}
                        onbits={link.callback(move |()| Msg::ToggleBits(bits_idx))}
                        onset={link.callback(Msg::SetSignal)}
                        onlocate={link.callback(Msg::Locate)}
                        onexport={link.callback(Msg::Export)}
                        delete={link.callback(|_| Msg::DeleteSig)} />
                    }
                }else {
                    html!{}
                }}
                <TimeBar store={self.store.clone()} width={metrics.surface_w}
                    remote={self.remote_session()} />
                // The grip for the label column. It has to be visible and to
                // cover the seam: the old divider was an empty `<div>` with no
                // css at all, i.e. a zero-height element nobody could hit, so
                // the column *looked* undraggable even though the fallback in
                // `MouseDown` worked.
                <div class={if self.names_drag.is_some() {"wave-names-splitter dragging"} else {"wave-names-splitter"}}
                    title="Drag to resize the label column, double click to reset"
                    onmousedown={link.callback(Msg::NamesDown)} />
                <div class="wave-body" id="wave-body"
                    // Focusable so a press inside the pane can move the keyboard
                    // focus here (tabindex -1: programmatic only, not in the tab
                    // order). Without it Delete kept going to monaco's hidden
                    // textarea and deleted the *source*, not the wave row.
                    tabindex="-1"
                    onscroll={link.callback(|_| Msg::Scroll)}
                    onwheel={link.callback(Msg::Wheel)}
                    onmousedown={link.callback(Msg::MouseDown)}
                    onmouseup={link.callback(Msg::MouseUp)}
                    onmousemove={link.callback(Msg::MouseMove)}
                    // right click sets the second cursor; the browser menu
                    // must not open on top of the wave area
                    oncontextmenu={Callback::from(|event: MouseEvent| event.prevent_default())}>
                    <div class="wave-rows" style={format!("height:{rows_height}")}
                        role="listbox" aria-label="Wave signals">
                    {if self.names_drag.is_some() {
                        html!{
                            <div class="splitter-shield"
                                onmousemove={link.callback(Msg::NamesMove)}
                                onmouseup={link.callback(|_| Msg::NamesUp)}
                                onmouseout={link.callback(|_| Msg::NamesUp)} />
                        }
                    }else {
                        html!{}
                    }}
                        {
                            for (name_first..name_last).map(|idx| {
                                let item = &self.signal_things[idx];
                                // The cursor values live in the label column now,
                                // not on the canvas next to the cursor line. A
                                // value is `Some` only when its cursor exists:
                                // the cell then adds one shared-width column,
                                // even while the row's window is still decoding
                                // (an empty string keeps the columns aligned).
                                let value = self
                                    .cursors
                                    .one
                                    .map(|_| self.value_text_at(idx, item, self.cursors.one).unwrap_or_default());
                                let value2 = self
                                    .cursors
                                    .two
                                    .map(|_| self.value_text_at(idx, item, self.cursors.two).unwrap_or_default());
                                // the full label (instance path + bus range) is
                                // always the tooltip; the visible text is the
                                // leaf name unless the "full path" option is on
                                let full = item.name.clone();
                                let name = if self.show_path {
                                    full.clone()
                                } else {
                                    leaf_name(&full).to_string()
                                };
                                html!{<SignalName
                                    name={name}
                                    full={full}
                                    choose={item.choose}
                                    top={idx as f64 * ROW_HEIGHT}
                                    value={value}
                                    value2={value2}
                                    menu={link.callback(move |()| Msg::ShowMenu(idx))}
                                    onjump={link.callback(move |()| Msg::Jump(idx))}
                                    onclick={link.callback(move |()| Msg::NameClick(idx))}/>}
                            })
                        }
                    </div>
                </div>
                <div class="wave-canvas" id="wave-canvas"
                    // the canvas overlay has `pointer-events: none`, so a real
                    // right click lands on .wave-body; this covers synthetic
                    // events and any future change to that, and keeps the
                    // browser menu off the waveform either way
                    oncontextmenu={Callback::from(|event: MouseEvent| event.prevent_default())}>
                    <canvas id="wave-surface" class="wave-surface"></canvas>
                    {if has_signal {html!{}} else {html!{
                        <div class="empty-hint">
                            <div>{"No signals yet"}</div>
                            <div>{"Pick one from the signal list, drag a range to zoom, "}<br/>
                                {"or Ctrl+Enter over a name in the source to add it here."}</div>
                        </div>
                    }}}
                </div>
            </div>
        }
    }
}

impl WaveShow {
    /// The real message handling; the trait's `update` only wraps it so the
    /// QA seam can count the pane's re-renders (R2 step 2 B2).
    fn handle(&mut self, ctx: &Context<Self>, msg: Msg) -> bool {
        match msg {
            Msg::Rows(rows) => {
                self.rows = rows;
                self.sync_rows();
                // the cursor cache is keyed by row index: a reorder or removal
                // invalidates every entry
                #[cfg(feature = "tauri")]
                {
                    self.native_cursor.clear();
                    self.native_cursor_inflight.clear();
                }
                // the name column, the settings and the canvas all follow the store
                true
            }
            Msg::Viewport(viewport) => {
                // a committed gesture landed: mirror it and repaint the canvas.
                // A zoom/pan needs no Yew render — the time bar has its own
                // subscription and the row list did not change (tier A/C) — but
                // the value column shows the cursor values, so a *cursor* commit
                // has to re-render the name column once.
                let cursors_changed = viewport.cursors != self.cursors;
                self.view = viewport.view;
                self.cursors = viewport.cursors;
                if cursors_changed {
                    return true;
                }
                self.redraw();
                false
            }
            Msg::TimeRange(time_range) => {
                self.time_range = time_range;
                // the ruler and the fit zoom read it
                true
            }
            Msg::Dump(waveform) => {
                // a new dump means every row has to be resolved and decoded
                // again; the handle itself lives in the store now
                if self.waveform != waveform {
                    // Wave 12: give back what the dump we are leaving held. Its
                    // decoded histories are only needed again when the user
                    // switches back, and its parsed body is only kept when
                    // another pane (the split) still draws that dump.
                    if let Some(previous) = self.waveform.take() {
                        previous.waveform().unload_all_signals();
                        if !memory::is_drawn(&self.store, &previous) {
                            previous.waveform().release_cache();
                        }
                    }
                    self.waveform = waveform;
                    self.fitted = false;
                    self.fit_committed = false;
                    self.signal_things
                        .refresh(self.waveform.as_ref().map(|waveform| waveform.waveform()));
                    self.scroll_top = scroll_top_from_dom();
                    self.sync_rows();
                    true
                } else {
                    false
                }
            }
            Msg::Wheel(e) => {
                let metrics = self.measure();
                // Over the name column the wheel keeps scrolling the rows, like
                // any list: the browser does it, the event is not stopped.
                if (e.x() as f64) < metrics.surface_left {
                    return false;
                }
                // The key state is the historical path; the event's own
                // modifiers also arrive on a wheel event and keep a synthetic
                // event (or a missed keydown) working.
                let ctrl = self.key(17) || e.ctrl_key();
                let shift = self.key(16) || e.shift_key();
                if ctrl {
                    // ctrl: zoom at the pointer, unchanged. Verdi zooms where
                    // you look; a left-edge zoom slides the interesting part
                    // away.
                    e.prevent_default();
                    let local_x = e.x() as f64 - metrics.surface_left;
                    let factor = if e.delta_y() < 0.0 { 1.25 } else { 0.8 };
                    let mut view = self.view;
                    view.zoom_at_px(local_x, factor, metrics.surface_w);
                    self.commit_view(view);
                } else if shift {
                    // shift: scroll the row list (this used to pan the trace;
                    // the plain wheel pans now, as in Verdi).
                    e.prevent_default();
                    scroll_rows_by(wheel_delta_px(e.delta_y(), e.delta_x(), e.delta_mode()));
                } else {
                    // plain wheel: pan the time axis, wheel down = forward in
                    // time, exactly the Verdi gesture. A horizontal trackpad
                    // swipe pans too, but only one axis is used per event
                    // (`wheel_delta_px`), so a diagonal gesture is never
                    // applied twice.
                    let dx = wheel_pan_px(e.delta_y(), e.delta_x(), e.delta_mode());
                    if dx == 0.0 {
                        return false;
                    }
                    e.prevent_default();
                    let mut view = self.view;
                    view.pan_px(dx);
                    self.commit_view(view);
                }
                false
            }
            Msg::Scroll => {
                self.scroll_top = scroll_top_from_dom();
                true
            }
            Msg::ToggleBits(idx) => {
                let Some(row) = self.rows.items.get(idx) else {
                    return true;
                };
                let path = row.path.clone();
                let expanded = self
                    .rows
                    .items
                    .iter()
                    .any(|candidate| candidate.path == path && candidate.bit.is_some());
                if expanded {
                    self.dispatch(state::Action::RowsBitCollapsed { index: idx });
                } else {
                    let width = self
                        .signal_things
                        .iter()
                        .nth(idx)
                        .map(|item| item.width)
                        .unwrap_or(1);
                    if width > 1 && width <= 64 {
                        self.dispatch(state::Action::RowsBitExpanded { index: idx, width });
                    }
                }
                // the dialog was the entry point; the expanded rows are the
                // answer, so close it
                self.dispatch(state::Action::DialogClosed);
                true
            }
            Msg::Dialog(dialog) => {
                self.dialog = dialog;
                true
            }
            Msg::ShowMenu(idx) => {
                let choosed_idx = self.signal_things.get_choose_idx();
                // The button belongs to one row: unless that row is part of the
                // selection, its settings are what the dialog must show (the
                // dialog used to open for whatever row happened to be selected).
                let indices = if choosed_idx.contains(&idx) {
                    choosed_idx
                } else {
                    self.dispatch(state::Action::RowSelected {
                        index: idx,
                        ctrl: false,
                        shift: false,
                    });
                    // the snapshot arrives through the subscription; settings read
                    // the next frame, exactly like before
                    self.sync_rows();
                    vec![idx]
                };
                self.dispatch(state::Action::DialogOpened { indices });
                true
            }
            Msg::SetSignal((close, set)) => {
                let indices = self.dialog_indices();
                if close {
                    self.dispatch(state::Action::DialogClosed);
                }
                self.dispatch(state::Action::RowSettingChanged {
                    indices,
                    setting: set,
                });
                // the native cursor readout is rendered text in the old format:
                // a format change invalidates it (the row windows hold parsed
                // bits, so they stay valid and no window refetch is needed)
                #[cfg(feature = "tauri")]
                {
                    self.native_cursor.clear();
                    self.native_cursor_inflight.clear();
                }
                true
            }
            Msg::DeleteSig => {
                let indices = self.dialog_indices();
                self.dispatch(state::Action::DialogClosed);
                self.dispatch(state::Action::RowsRemoved { indices });
                ctx.props().onselect.emit(None);
                true
            }
            Msg::NamesDown(e) => {
                e.prevent_default();
                // double click restores the default width, like the other
                // splitters in the app
                if e.detail() >= 2 {
                    self.names_drag = None;
                    self.names_w = DEFAULT_NAMES_W;
                    self.names_preview.set(DEFAULT_NAMES_W);
                    return true;
                }
                self.names_drag = Some((e.x() as f64, self.names_w));
                self.names_preview.set(self.names_w);
                true
            }
            Msg::NamesMove(e) => {
                if let Some((start_x, start_w)) = self.names_drag {
                    let width =
                        (start_w + (e.x() as f64 - start_x)).clamp(NAMES_W_MIN, NAMES_W_MAX);
                    self.names_preview.set(width);
                    // tier A: write the css variable directly, no re-render
                    write_names_width(width);
                }
                false
            }
            Msg::NamesUp => {
                if self.names_drag.take().is_some() {
                    self.names_w = self.names_preview.get();
                }
                true
            }
            Msg::KeyEvent(idx) => {
                let mut view = self.view;
                if idx == 90 {
                    //z
                    if self.key(16) {
                        //shift
                        view.zoom(1.25 * 1.25);
                    } else {
                        view.zoom(0.64);
                    }
                } else if idx == 70 {
                    //f
                    let metrics = self.measure();
                    let end = self.trace_end(self.effective_time_range());
                    view = View::new(0.0, View::fit_size(end, metrics.surface_w));
                    self.fitted = true;
                } else if idx == 78 {
                    // n / shift+n: Verdi's "next / previous transition". The
                    // cursor walks the value changes of the selected rows, or of
                    // every visible row when nothing is selected.
                    let direction = if self.key(16) { -1 } else { 1 };
                    self.jump_to_transition(direction);
                    return false;
                } else if idx == 65 {
                    //ctrl+a: select every row (one Delete then removes them all)
                    if self.key(17) {
                        self.dispatch(state::Action::RowsAllSelected);
                        return true;
                    }
                } else if idx == 27 {
                    // esc: the settings dialog owns the key while it is open
                    // (closing it must not also clear the row selection); with
                    // no dialog it clears the selection, as before
                    if self.dialog.is_some() {
                        self.dispatch(state::Action::DialogClosed);
                        return true;
                    }
                    self.dispatch(state::Action::RowsCleared);
                    return true;
                } else if let Some(key) = wave_key_action(idx, self.key(17), self.key(16)) {
                    // arrow keys / Home / End move the row selection, so the
                    // wave list is usable without the mouse
                    self.move_selection(key);
                    return true;
                } else if idx == 46 || idx == 8 {
                    //delete / backspace removes the selected rows
                    let indices: Vec<usize> = self
                        .rows
                        .items
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.chosen)
                        .map(|(index, _)| index)
                        .collect();
                    if !indices.is_empty() {
                        self.dispatch(state::Action::RowsRemoved { indices });
                        ctx.props().onselect.emit(None);
                        return true;
                    }
                }
                self.commit_view(view);
                false
            }
            Msg::MouseDown(e) => {
                e.prevent_default();
                // The keyboard follows the mouse: a press in the wave pane moves
                // the focus here, so Delete removes the selected wave rows instead
                // of deleting text in the source editor that kept the focus.
                self.focus_body();
                let metrics = self.measure();
                let x = e.x() as f64;
                // within a few pixels of the label column's edge the press drags
                // the column width instead of starting a row reorder or a
                // selection. Doing it here keeps the hit test on the body, which
                // already owns the mouse, instead of on a divider the canvas
                // overlay can swallow.
                if (x - metrics.surface_left).abs() <= 5.0 {
                    self.names_drag = Some((x, self.names_w));
                    self.names_preview.set(self.names_w);
                    return false;
                }
                if x < metrics.surface_left {
                    //the name column: start a reorder drag
                    self.name_choose_valid = true;
                    self.name_start_y = e.y() as f64;
                    self.name_choose = self.row_at(e.y() as f64);
                    return false;
                }
                // the edges the cursors can snap to are frozen at the press; a
                // drag of the view must not re-collect them per mouse move
                self.snap_edges = self.collect_snap_edges(&metrics);
                let local_x = x - metrics.surface_left;
                // pressing on a cursor line grabs it (the Verdi gesture: drag
                // the line to move the cursor); anywhere else starts a zoom band
                if let Some(which) = self.cursor_at(local_x) {
                    self.cursor_drag = Some(which);
                    self.cursor_preview = None;
                    self.mouse_press = false;
                    return false;
                }
                self.cursor_drag = None;
                self.cursor_preview = None;
                self.mouse_press = true;
                self.mouse_start_x = local_x;
                self.mouse_current_x = local_x;
                // tier A: the band is a local preview, never a Yew render
                self.redraw();
                false
            }
            Msg::MouseMove(e) => {
                if let Some((start_x, start_w)) = self.names_drag {
                    // tier A: the css variable moves, the component does not render
                    let width =
                        (start_w + (e.x() as f64 - start_x)).clamp(NAMES_W_MIN, NAMES_W_MAX);
                    self.names_preview.set(width);
                    write_names_width(width);
                    return false;
                }
                if let Some(which) = self.cursor_drag {
                    let metrics = self.measure();
                    let x = e.x() as f64 - metrics.surface_left;
                    let time = self.snapped_time(x);
                    let mut preview = self.cursor_preview.unwrap_or(self.cursors);
                    match which {
                        CursorDrag::One => preview.one = Some(time),
                        CursorDrag::Two => preview.two = Some(time),
                    }
                    self.cursor_preview = Some(preview);
                    // tier A: repaint the dragged line, no dispatch and no render
                    self.redraw();
                    return false;
                }
                if !self.mouse_press {
                    return false;
                }
                let metrics = self.measure();
                self.mouse_current_x = e.x() as f64 - metrics.surface_left;
                // tier A: repaint the band straight on the canvas, no Yew render
                self.redraw();
                false
            }
            Msg::MouseUp(e) => {
                e.prevent_default();
                if self.names_drag.take().is_some() {
                    self.names_w = self.names_preview.get();
                    return true;
                }
                if let Some(which) = self.cursor_drag.take() {
                    let preview = self.cursor_preview.take().unwrap_or(self.cursors);
                    self.snap_edges.clear();
                    if preview == self.cursors {
                        // a press without a move is still a click: it clears
                        // the cursor it landed on
                        let mut cursors = self.cursors;
                        match which {
                            CursorDrag::One => cursors.one = None,
                            CursorDrag::Two => cursors.two = None,
                        }
                        self.commit_cursors(cursors);
                    } else {
                        // one dispatch for the whole drag, on mouseup
                        self.commit_cursors(preview);
                    }
                    // one render after the commit, so the value column follows
                    // the moved cursor; the drag itself never rendered
                    return true;
                }
                if self.name_choose_valid {
                    self.name_choose_valid = false;
                    // A click also fires mousedown and mouseup; only a real drag
                    // reorders. Without this every click on a name swapped it
                    // with its neighbour, because the drop position rounded to
                    // the next row.
                    if (e.y() as f64 - self.name_start_y).abs() < ROW_HEIGHT / 2.0 {
                        return true;
                    }
                    let metrics = self.measure();
                    let location = (e.y() as f64 - metrics.body_top + self.scroll_top) / ROW_HEIGHT;
                    if location < 0.0 {
                        return true;
                    }
                    let current = self.name_choose;
                    let dest = if location > current as f64 + 0.5 {
                        (location - 0.5) as usize
                    } else {
                        (location + 0.5) as usize
                    };
                    if current < self.rows.len() && dest < self.rows.len() && dest != current {
                        self.dispatch(state::Action::RowsReordered {
                            from: current,
                            to: dest,
                        });
                    }
                    return true;
                }
                if !self.mouse_press {
                    return false;
                }
                self.mouse_press = false;
                let metrics = self.measure();
                let x = e.x() as f64 - metrics.surface_left;
                if (self.mouse_start_x - x).abs() <= 2.0 {
                    //a click, not a drag: set (or clear) one of the cursors,
                    //snapped to the closest value edge on screen
                    let cursor_time = self.snapped_time(x);
                    self.snap_edges.clear();
                    let mut cursors = self.cursors;
                    if e.button() == 0 {
                        cursors.one = if cursors.one == Some(cursor_time) {
                            None
                        } else {
                            Some(cursor_time)
                        };
                    } else {
                        cursors.two = if cursors.two == Some(cursor_time) {
                            None
                        } else {
                            Some(cursor_time)
                        };
                    }
                    self.commit_cursors(cursors);
                    // one render after the commit, for the value column (tier A
                    // only forbids renders while the pointer moves)
                    true
                } else {
                    //drag a range to zoom into it
                    let mut view = self.view;
                    view.zoom_to_px(self.mouse_start_x, x, metrics.surface_w);
                    self.commit_view(view);
                    false
                }
            }
            Msg::NameClick(idx) => {
                let ctrl = self.key(17);
                let shift = self.key(16);
                // the store owns the selection; read the path before it changes
                let path = self
                    .signal_things
                    .iter()
                    .nth(idx)
                    .map(|item| item.path.clone());
                self.dispatch(state::Action::RowSelected {
                    index: idx,
                    ctrl,
                    shift,
                });
                self.sync_rows();
                if let Some(path) = path {
                    // the inspector follows the last clicked row
                    ctx.props().onselect.emit(Some(path.clone()));
                    if !ctrl && !shift {
                        // plain click = "show me where this is declared"
                        ctx.props().onjump.emit(path);
                    }
                }
                true
            }
            Msg::Jump(idx) => {
                if let Some(item) = self.signal_things.iter().nth(idx) {
                    let path = item.path.clone();
                    ctx.props().onselect.emit(Some(path.clone()));
                    ctx.props().onjump.emit(path);
                }
                true
            }
            Msg::Locate((load, index)) => {
                // the dialog belongs to the first row of its target set; the app
                // resolves the driver/load span (or lists the several) from there
                let path = self
                    .dialog_indices()
                    .first()
                    .and_then(|idx| self.signal_things.iter().nth(*idx))
                    .map(|item| item.path.clone());
                if let Some(path) = path {
                    self.dispatch(state::Action::DialogClosed);
                    ctx.props().onlocate.emit((load, index, path));
                }
                true
            }
            Msg::Resize => {
                // The layout slice also carries the label display option: when
                // it flips, the name column has to re-render (the message is
                // named after the resize because that is what it mostly is).
                let show_path = self.store.select(select::layout).show_path;
                if show_path != self.show_path {
                    self.show_path = show_path;
                    return true;
                }
                true
            }
            Msg::Export((format, scope)) => {
                // the download is the answer, so the dialog closes either way and
                // the status line says what happened
                let result = self.export(format, scope);
                self.dispatch(state::Action::DialogClosed);
                match result {
                    Ok(note) => self.dispatch(state::Action::Status(Status::info(note))),
                    Err(note) => self.dispatch(state::Action::Status(Status::error(note))),
                }
                true
            }
            // rows were decoded after the last render: the value column now has
            // the values to show
            Msg::ValuesReady => true,
            #[cfg(feature = "tauri")]
            Msg::RemoteWindows(epoch, req, show, rows, windows) => {
                self.apply_remote_windows(epoch, req, &show, &rows, windows);
                false
            }
            #[cfg(feature = "tauri")]
            Msg::RemoteCursorValue(row, time, text) => {
                self.native_cursor_inflight.remove(&(row, time));
                self.native_cursor.insert(row, time, text);
                // the value column shows the new readout: one extra render
                true
            }
            #[cfg(feature = "tauri")]
            Msg::RemoteFailed(epoch, error) => {
                if epoch == self.remote_epoch && !self.remote_error {
                    self.remote_error = true;
                    self.dispatch(state::Action::Status(Status::error(format!(
                        "native dump: {error}"
                    ))));
                }
                false
            }
        }
    }

    /// Move cursor 1 to the next (`direction > 0`) or previous value change of
    /// the selected rows, or of every visible row when nothing is selected.
    ///
    /// This is Verdi's "next transition": the cursor walks the edges of what you
    /// are looking at, and the view follows when the edge is off screen. The
    /// histories are sorted, so each row costs one binary search, not a walk.
    fn jump_to_transition(&mut self, direction: i64) {
        let metrics = self.measure();
        if !metrics.is_usable() {
            return;
        }
        let (draw, _) = visible_rows(self.scroll_top, metrics.surface_h, self.signal_things.len());
        let selected: Vec<usize> = self
            .rows
            .items
            .iter()
            .enumerate()
            .filter(|(_, row)| row.chosen)
            .map(|(index, _)| index)
            .collect();
        let rows: Vec<usize> = if selected.is_empty() {
            draw.collect()
        } else {
            selected
        };
        let current = self.cursors.one.unwrap_or(-1).max(0) as Time;
        // how far outside the window one bounded fetch reaches for a row whose
        // cached window does not contain the cursor
        let reach = (self.view.time_span(metrics.surface_w).ceil() as u64)
            .max(1)
            .saturating_mul(4);
        let mut best: Option<Time> = None;
        for row in rows {
            let changes = self.near_changes(row, current, reach);
            let candidate = if direction > 0 {
                let index = changes.partition_point(|(time, _)| *time <= current);
                changes.get(index).map(|(time, _)| *time)
            } else {
                let index = changes.partition_point(|(time, _)| *time < current);
                index
                    .checked_sub(1)
                    .and_then(|index| changes.get(index))
                    .map(|(time, _)| *time)
            };
            let Some(candidate) = candidate else {
                continue;
            };
            best = Some(match best {
                Some(best) if direction > 0 => best.min(candidate),
                Some(best) => best.max(candidate),
                None => candidate,
            });
        }
        let Some(target) = best else {
            return;
        };
        // keep the cursor on screen: when the edge is outside the window, move
        // the view so it lands a quarter from the left
        let x = self.view.x_of(target);
        if !(8.0..=metrics.surface_w - 8.0).contains(&x) {
            let span = self.view.time_span(metrics.surface_w);
            let axis = (target as f64 - span * 0.25).max(0.0);
            self.commit_view(View::new(axis, self.view.size));
        }
        let mut cursors = self.cursors;
        cursors.one = Some(target.min(i32::MAX as Time) as i32);
        self.commit_cursors(cursors);
    }

    /// Commit a zoom/pan: mirror it for the current frame, dispatch it once and
    /// repaint. No Yew render — the canvas is the only thing that changes, and
    /// the time bar re-renders from its own store subscription.
    ///
    /// The view is kept inside the dump: panning or zooming out past the end
    /// used to scroll the trace off to the left and leave the rest of the
    /// surface empty (and, worse, holding the last value over it).
    fn commit_view(&mut self, view: View) {
        let view = self.clamp_view(view);
        self.view = view;
        self.dispatch(state::Action::ViewCommitted(view));
        self.redraw();
    }

    /// `view`, pulled back so its right edge is at or before the end of the
    /// trace. Formats without a known range (GHW) are left alone.
    fn clamp_view(&self, view: View) -> View {
        match self.time_range {
            Some(range) => view.clamp_to_end(range.end as i64, self.measure().surface_w),
            None => view,
        }
    }

    /// Commit the cursors (one dispatch per click, never per mousemove).
    fn commit_cursors(&mut self, cursors: Cursors) {
        self.cursors = cursors;
        self.dispatch(state::Action::CursorsChanged(cursors));
        self.redraw();
    }

    /// The cursors to draw: the drag preview while a cursor line is being moved.
    fn shown_cursors(&self) -> Cursors {
        self.cursor_preview.unwrap_or(self.cursors)
    }

    /// The formatted value of one row at a cursor, for the value column.
    ///
    /// `None` when the cursor is not set, the row has no decoded value (it is
    /// outside the decode window) or the value has no textual form (an event).
    #[allow(unused_variables)] // row is used by the tauri-gated readout cache
    fn value_text_at(&self, row: usize, item: &SignalItem, cursor: Option<i32>) -> Option<String> {
        let cursor = cursor?;
        if cursor < 0 {
            return None;
        }
        // a native cursor outside the fetched window has its rendered value
        // cached from dump_value_at (already formatted with the row's show type)
        #[cfg(feature = "tauri")]
        if let Some(text) = self.native_cursor.get(row, cursor) {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
        let value = item.values.value_at(cursor as Time)?;
        canvas::value_text(value, item.format_width(), &item.setting)
    }

    /// The widest value text (in characters) the rows now on screen can print.
    ///
    /// Derived from the row's bit width and number format, not from decoded
    /// text: the value column must not re-size while the cursor moves or while
    /// a scroll decodes the row that happens to hold the longest number. Only
    /// the visible rows count, so a narrow list does not pay for a wide row
    /// outside the viewport.
    fn widest_visible_value_chars(&self, draw: &std::ops::Range<usize>) -> usize {
        draw.clone()
            .filter_map(|row| self.signal_things.iter().nth(row))
            .map(|item| {
                let fixed_point = item
                    .setting
                    .fixed_active
                    .then_some(item.setting.fixed_point);
                value_text_chars(
                    item.format_width(),
                    item.is_bool,
                    &item.setting.show_type,
                    fixed_point,
                )
            })
            .max()
            .unwrap_or(1)
    }

    /// The time a click at `x` (surface pixels) places a cursor at: the raw
    /// time, snapped onto a visible value edge when one is a few pixels away.
    ///
    /// The distance is measured in **pixels from the click**, not by converting
    /// the click to a time and back: that round trip truncates to whole ticks.
    /// [`geometry::snap_x_to_edges`] also measures to the whole pixel the edge is
    /// drawn on, so the tolerance is the same on both sides of an edge.
    fn snapped_time(&self, x: f64) -> i32 {
        match geometry::snap_x_to_edges(x, &self.snap_edges, &self.view, geometry::SNAP_PX) {
            Some(edge) => edge.min(i32::MAX as u64) as i32,
            None => self.view.time_at(x).max(0),
        }
    }

    /// The cursor whose line is under `x` (surface pixels), if any.
    fn cursor_at(&self, x: f64) -> Option<CursorDrag> {
        let mut best: Option<(f64, CursorDrag)> = None;
        for (cursor, which) in [
            (self.cursors.one, CursorDrag::One),
            (self.cursors.two, CursorDrag::Two),
        ] {
            let Some(cursor) = cursor else { continue };
            let distance = (self.view.x_of(cursor.max(0) as Time) - x).abs();
            if distance <= geometry::CURSOR_GRAB_PX
                && best.is_none_or(|(best_distance, _)| distance < best_distance)
            {
                best = Some((distance, which));
            }
        }
        best.map(|(_, which)| which)
    }

    /// The value-change edges of the rows on screen, sorted and de-duplicated.
    ///
    /// Collected once when a press starts, then reused for the whole gesture: a
    /// drag must not walk the decoded histories per mouse move (tier A).
    fn collect_snap_edges(&self, metrics: &Metrics) -> Vec<Time> {
        let rows = self.signal_things.len();
        let (draw, _) = visible_rows(self.scroll_top, metrics.surface_h, rows);
        let (start, end) = self.view.window(metrics.surface_w);
        let mut edges = Vec::new();
        self.signal_things.edges(draw, start, end, &mut edges);
        edges.sort_unstable();
        edges.dedup();
        edges
    }

    /// Publish the pane's re-render count next to the shell's, for the QA gate.
    fn publish_pane_render_count(&self) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let count = JsValue::from_f64(f64::from(self.pane_render_count.get()));
        let _ = js_sys::Reflect::set(&window, &JsValue::from_str("__wavePaneRenderCount"), &count);
    }

    /// Move the keyboard focus into the wave pane.
    ///
    /// The pane has no input elements: the store owns the row selection and a
    /// window keydown listener reads the keys. That listener deliberately skips
    /// keys typed into a text field (monaco's hidden textarea, the settings
    /// dialog), so without this the focus stayed in the editor and Delete
    /// deleted the source text under the cursor instead of the wave row.
    fn focus_body(&self) {
        let Some(body) = document().and_then(|document| {
            document
                .get_element_by_id("wave-body")
                .and_then(|element| element.dyn_into::<web_sys::HtmlElement>().ok())
        }) else {
            return;
        };
        let _ = body.focus();
    }

    /// One store change; the resulting snapshot arrives as `Msg::Rows`.
    fn dispatch(&self, action: state::Action) {
        let effects = self.store.dispatch(action);
        debug_assert!(effects.is_empty(), "R2 step 2 has no effects yet");
    }

    /// Map the store's rows onto the render model, keeping decoded values for
    /// rows that survived and unloading the ones that are gone.
    fn sync_rows(&mut self) {
        let waveform = self.waveform.as_ref().map(|waveform| waveform.waveform());
        let unload = self.signal_things.sync(&self.rows.items, waveform);
        if let Some(waveform) = waveform {
            for index in unload {
                let _ = waveform.unload_signal(index);
            }
        }
    }

    /// The whole decoded history of one row, for an export or a value search.
    ///
    /// Drawing is windowed now; export/find are user-initiated whole-history
    /// operations, so they are *bounded* here: a dense trace that would
    /// materialise more than `HISTORY_CHANGE_LIMIT` changes is refused with an
    /// actionable message instead of building a multi-hundred-MB vector and
    /// aborting the tab.
    ///
    /// An expanded bit row exports the bit, not its parent bus: the derived
    /// history is the one the row draws.
    fn decoded_for(&self, index: usize) -> Result<DecodedSignal, String> {
        let item = self
            .signal_things
            .iter()
            .nth(index)
            .ok_or_else(|| "the row is gone — open the settings again".to_string())?;
        let waveform = self
            .waveform
            .as_ref()
            .map(|handle| handle.waveform())
            .ok_or_else(|| "no dump open — load a waveform before exporting".to_string())?;
        let signal = item.index.ok_or_else(|| {
            format!(
                "{}: no values — the signal is not in the open dump",
                item.name
            )
        })?;
        // count first: the cap tells whether a full decode would be too dense
        let probe = waveform
            .window_changes_many(&[signal], 0, None, HISTORY_CHANGE_LIMIT)
            .map_err(|error| error.message())?;
        if probe.first().is_some_and(|row| row.capped) {
            return Err(format!(
                "{}: more than {} value changes — too dense for an in-browser whole-history operation; narrow the dump or use the desktop build",
                item.name, HISTORY_CHANGE_LIMIT
            ));
        }
        let decoded = waveform
            .decoded_changes(signal)
            .map_err(|error| error.message())?;
        Ok(match item.bit {
            Some(bit) => derive_bit(&decoded, bit as usize),
            None => decoded,
        })
    }

    /// Build the requested export and hand it to the browser as a download.
    ///
    /// Returns the status line to show: the success summary, or an actionable
    /// error (no dump, no decoded values, too large, browser refused). Never
    /// panics on a user action.
    fn export(&self, format: ExportFormat, scope: ExportScope) -> Result<String, String> {
        #[cfg(feature = "tauri")]
        if self.remote.is_some() {
            return Err(
                "native (read-only, mmap) mode: export is not available yet — reopen the dump through the wasm path (files up to 64 MB) to export"
                    .to_string(),
            );
        }
        let Some(waveform) = self.waveform.as_ref() else {
            return Err("no dump open — load a waveform before exporting".to_string());
        };
        let dump = waveform.waveform();
        let stem = self
            .store
            .select(select::dump_name)
            .map(|name| export::dump_stem_name(&name))
            .unwrap_or_else(|| export::dump_stem(Some(dump)));
        match scope {
            ExportScope::Row => {
                let index = self.dialog_indices().first().copied().ok_or_else(|| {
                    "no row selected — open a row's settings and try again".to_string()
                })?;
                let item = self
                    .signal_things
                    .iter()
                    .nth(index)
                    .ok_or_else(|| "the row is gone — open the settings again".to_string())?;
                let name = item.name.clone();
                let row_format = ValueFormat::from_setting(&item.setting);
                let width = item.format_width();
                let decoded = self.decoded_for(index)?;
                let row = export::ExportRow {
                    name: name.clone(),
                    format: row_format,
                    width,
                    changes: decoded.changes(),
                };
                let file = export::file_name(&stem, Some(&name), format);
                if format.is_binary() {
                    let bytes = export::row_mat(&row).map_err(|error| error.message())?;
                    export::download_bytes(&file, format.mime(), &bytes)?;
                    return Ok(export::row_status(&name, row.changes.len(), bytes.len()));
                }
                let text = match format {
                    ExportFormat::Csv => export::row_csv(&row),
                    ExportFormat::Txt => export::row_txt(&row),
                    ExportFormat::Mat => unreachable!("binary formats return above"),
                }
                .map_err(|error| error.message())?;
                export::download(&file, format.mime(), &text)?;
                Ok(export::row_status(&name, row.changes.len(), text.len()))
            }
            ExportScope::AllRows => {
                if format.is_binary() {
                    return Err(
                        "MAT export is one row at a time — open a row's settings and export it there"
                            .to_string(),
                    );
                }
                // rows in row order; a row that is not in the dump contributes
                // nothing instead of aborting the whole export
                let mut decoded_rows: Vec<DecodedSignal> = Vec::new();
                let mut meta: Vec<(String, ValueFormat, usize)> = Vec::new();
                for index in 0..self.signal_things.len() {
                    // a row that is not in the dump contributes nothing; a
                    // too-dense trace is a hard, actionable error
                    if self.signal_things[index].index.is_none() {
                        continue;
                    }
                    let decoded = self.decoded_for(index)?;
                    let item = &self.signal_things[index];
                    meta.push((
                        item.name.clone(),
                        ValueFormat::from_setting(&item.setting),
                        item.format_width(),
                    ));
                    decoded_rows.push(decoded);
                }
                if decoded_rows.is_empty() {
                    return Err("nothing to export: no row has decoded values".to_string());
                }
                let rows: Vec<export::ExportRow<'_>> = meta
                    .iter()
                    .zip(decoded_rows.iter())
                    .map(|((name, format, width), decoded)| export::ExportRow {
                        name: name.clone(),
                        format: format.clone(),
                        width: *width,
                        changes: decoded.changes(),
                    })
                    .collect();
                let changes: usize = decoded_rows
                    .iter()
                    .map(|decoded| decoded.changes().len())
                    .sum();
                let text = export::rows_csv(&rows).map_err(|error| error.message())?;
                let file = export::file_name(&stem, None, ExportFormat::Csv);
                export::download(&file, ExportFormat::Csv.mime(), &text)?;
                Ok(export::rows_status(rows.len(), changes, text.len()))
            }
        }
    }

    /// Move the row selection by one of the arrow keys / Home / End.
    ///
    /// The selection model is the store's: a plain `RowSelected` selects exactly
    /// one row, which is what an arrow key means. The list does not scroll here
    /// (the row list scrolls from its own dom node); a selection moved off screen
    /// is still selected, and the next render shows it once it is scrolled to.
    fn move_selection(&mut self, key: WaveKey) {
        let count = self.rows.items.len();
        if count == 0 {
            return;
        }
        let current = self
            .rows
            .items
            .iter()
            .enumerate()
            .filter(|(_, row)| row.chosen)
            .map(|(index, _)| index)
            .next_back();
        let target = match key {
            WaveKey::Up => current.map(|index| index.saturating_sub(1)).unwrap_or(0),
            WaveKey::Down => current.map(|index| (index + 1).min(count - 1)).unwrap_or(0),
            WaveKey::Home => 0,
            WaveKey::End => count - 1,
        };
        if Some(target) != current {
            self.dispatch(state::Action::RowSelected {
                index: target,
                ctrl: false,
                shift: false,
            });
        }
    }

    fn key(&self, code: usize) -> bool {
        self.key_press.borrow().get(code).copied().unwrap_or(false)
    }

    /// End of the trace for the fit zoom: the cheap range when the dump knows
    /// one, otherwise the newest decoded history (GHW has no cheap range).
    fn trace_end(&self, time_range: Option<TimeRange>) -> i64 {
        if let Some(range) = time_range {
            return range.end as i64;
        }
        self.signal_things
            .iter()
            .filter_map(|item| match &item.values {
                RowValues::Window(window) => window.span_end().map(|end| end as i64),
                RowValues::Empty => None,
            })
            .max()
            .unwrap_or(0)
    }

    /// Changes of one row around `current`, for the "next transition" walk.
    ///
    /// The row's cached window answers when it contains the cursor (the common
    /// case: the cursor is on screen). Outside it, one *bounded* window fetch is
    /// made for that row, so the walk never needs the whole history of a dense
    /// trace. The fetch is capped by the surface width like a redraw.
    fn near_changes(&self, row: usize, current: Time, reach: Time) -> Vec<Change> {
        let Some(item) = self.signal_things.iter().nth(row) else {
            return Vec::new();
        };
        if let Some(window) = item.values.cached() {
            if window.req.start <= current && window.req.end >= current {
                return window.changes().to_vec();
            }
        }
        let (Some(index), Some(handle)) = (item.index, self.waveform.as_ref()) else {
            return Vec::new();
        };
        // a row whose values are not cached yet still has to be able to walk:
        // ask for a bounded window around the cursor
        let width = self.measure().surface_w;
        let req = WindowRequest {
            start: current.saturating_sub(reach),
            end: current.saturating_add(reach),
            visible_start: current.saturating_sub(reach),
            visible_end: current.saturating_add(reach),
            cap: super::windowed::window_cap(width),
        };
        match windowed::fetch(handle.waveform(), &[index], req) {
            Ok(mut windows) => windows
                .pop()
                .flatten()
                .map(|window| window.changes().to_vec())
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// Indices of the rows the settings dialog is open for; empty when closed.
    fn dialog_indices(&self) -> Vec<usize> {
        self.dialog
            .as_ref()
            .map(|dialog| dialog.indices.clone())
            .unwrap_or_default()
    }

    /// `(width, expanded)` of the row at `index`, for the expand-bits button:
    /// only a bus that is really in the dump can be expanded, and only once.
    fn bits_state(&self, index: usize) -> (usize, bool) {
        let width = self
            .signal_things
            .iter()
            .nth(index)
            .filter(|item| item.index.is_some() && item.bit.is_none())
            .map(|item| item.width)
            .unwrap_or(1);
        let expanded = self.rows.items.get(index).is_some_and(|row| {
            self.rows
                .items
                .iter()
                .any(|candidate| candidate.path == row.path && candidate.bit.is_some())
        });
        (width, expanded)
    }

    /// Row index under a client y coordinate (including the scroll offset).
    fn row_at(&self, client_y: f64) -> usize {
        let metrics = self.measure();
        let content_y = client_y - metrics.body_top + self.scroll_top;
        if content_y <= 0.0 {
            0
        } else {
            (content_y / ROW_HEIGHT).floor() as usize
        }
    }

    /// Fetch the window that covers the visible time range for the rows that
    /// are (or are about to be) on screen.
    ///
    /// One bounded backend pass for all of them (`window_changes_many`): the
    /// visible range plus a margin, with a per-signal cap derived from the
    /// surface width. A dense signal therefore costs a few changes per pixel
    /// column instead of its whole history, which is what keeps the memory
    /// proportional to the pixels and not to the dump. A row whose cached window
    /// still covers the view (a small pan) is not fetched again.
    fn decode_pending(&mut self, fetch: std::ops::Range<usize>, width: f64) {
        // the desktop build can draw a dump opened by the native backend: the
        // rows come from bounded IPC windows, not from a wasm waveform
        #[cfg(feature = "tauri")]
        if self.remote.is_some() {
            self.decode_remote(fetch, width);
            return;
        }
        // clone the handle so the backend borrow does not block the cache writes
        let Some(handle) = self.waveform.clone() else {
            self.last_decoded = 0;
            return;
        };
        let waveform = handle.waveform();
        let req = WindowRequest::for_view(self.view, width);
        let mut rows: Vec<usize> = Vec::new();
        let mut signals: Vec<usize> = Vec::new();
        for row in fetch {
            let Some(item) = self.signal_things.iter().nth(row) else {
                break;
            };
            let Some(index) = item.index else {
                continue;
            };
            if !windowed::needs_fetch(item.values.cached(), req) {
                continue;
            }
            rows.push(row);
            signals.push(index);
        }
        if rows.is_empty() {
            self.last_decoded = 0;
            return;
        }
        let fetched = match windowed::fetch(waveform, &signals, req) {
            Ok(fetched) => fetched,
            Err(_error) => {
                //the rows keep their previous window; a later redraw retries
                self.last_decoded = 0;
                return;
            }
        };
        let mut decoded = 0usize;
        for (row, window) in rows.into_iter().zip(fetched) {
            let Some(window) = window else {
                continue;
            };
            // an expanded bit is one bit of its parent's window: derive the bit
            // instead of asking the backend for a signal the dump does not have
            let bit = self.signal_things.iter().nth(row).and_then(|item| item.bit);
            match bit {
                Some(bit) => self
                    .signal_things
                    .set_window(row, windowed::derive_bit_window(&window, bit as usize)),
                None => self.signal_things.set_window(row, window),
            }
            decoded += 1;
        }
        self.last_decoded = decoded;
    }

    /// Fetch the bounded windows for the visible rows from the native backend.
    ///
    /// Same window model as the wasm path ([`windowed::WindowRequest`]: visible
    /// range + margin, cap from the surface width), but:
    ///
    /// * the rows are grouped by their display format, because dump_window
    ///   renders the text with ONE show type: one request per format on screen
    ///   (normally one or two), so row formats work natively exactly like wasm;
    /// * the values come back as text and are parsed back into bit patterns
    ///   ([`crate::remote::window_groups`], [`crate::remote::parse_rendered`]);
    /// * [`apply_remote_windows`] stores them when they arrive and the pane keeps
    ///   drawing the previous windows in the meantime. Answers from an older
    ///   request are dropped by the epoch.
    #[cfg(feature = "tauri")]
    fn decode_remote(&mut self, fetch: std::ops::Range<usize>, width: f64) {
        let Some(session) = self.remote.clone() else {
            return;
        };
        let Some(link) = self.link.clone() else {
            return;
        };
        let req = WindowRequest::for_view(self.view, width);
        // the rows that need a window, with everything the request needs
        let mut pending: Vec<(usize, usize, usize)> = Vec::new();
        for row in fetch {
            let Some(item) = self.signal_things.iter().nth(row) else {
                break;
            };
            let Some(signal) = session.signal_of_path(&item.path) else {
                continue;
            };
            if !windowed::needs_fetch(item.values.cached(), req) {
                continue;
            }
            pending.push((row, signal.index, signal.width));
        }
        if pending.is_empty() {
            self.last_decoded = 0;
            return;
        }
        // one request per display format: the backend renders one show type
        let rows: Vec<usize> = pending.iter().map(|(row, _, _)| *row).collect();
        let groups = crate::remote::window_groups(rows.iter().copied(), |row| {
            self.signal_things
                .iter()
                .nth(row)
                .map(|item| item.setting.show_type.clone())
                .unwrap_or_default()
        });
        // one grouped native request: (display format, signal indices, (row,
        // slot, width) of the rows asking for it). The `allow` keeps clippy's
        // type-complexity lint quiet for this cfg(tauri)-only tuple shape.
        #[allow(clippy::type_complexity)]
        let mut jobs: Vec<(&'static str, Vec<usize>, Vec<(usize, usize, usize)>)> = Vec::new();
        for (show, group_rows) in groups {
            let mut signals: Vec<usize> = Vec::new();
            let mut slot_of: std::collections::HashMap<usize, usize> =
                std::collections::HashMap::new();
            let mut group: Vec<(usize, usize, usize)> = Vec::new();
            for row in group_rows {
                let Some(&(_, signal, row_width)) =
                    pending.iter().find(|(member, _, _)| *member == row)
                else {
                    continue;
                };
                let slot = *slot_of.entry(signal).or_insert_with(|| {
                    signals.push(signal);
                    signals.len() - 1
                });
                group.push((row, slot, row_width));
            }
            if !signals.is_empty() {
                jobs.push((show, signals, group));
            }
        }
        if jobs.is_empty() {
            self.last_decoded = 0;
            return;
        }
        self.remote_epoch = self.remote_epoch.wrapping_add(1);
        let epoch = self.remote_epoch;
        let id = session.id();
        let start = req.start;
        let end = req.end;
        let cap = req.cap;
        for (show, signals, group) in jobs {
            let link = link.clone();
            spawn_local(async move {
                match crate::remote::window(id, &signals, start, Some(end), cap, show).await {
                    Ok(windows) => link.send_message(Msg::RemoteWindows(
                        epoch,
                        req,
                        show.to_string(),
                        group,
                        windows,
                    )),
                    Err(error) => link.send_message(Msg::RemoteFailed(epoch, error)),
                }
            });
        }
    }

    #[cfg(feature = "tauri")]
    fn apply_remote_windows(
        &mut self,
        epoch: u32,
        req: WindowRequest,
        show: &str,
        rows: &[(usize, usize, usize)],
        windows: Vec<crate::remote::WindowDto>,
    ) {
        if epoch != self.remote_epoch {
            return;
        }
        let show = crate::remote::show_type_named(show).unwrap_or_default();
        let mut decoded = 0usize;
        for &(row, slot, width) in rows {
            let Some(dto) = windows.get(slot) else {
                continue;
            };
            let window = crate::remote::row_window_with_width_and_show(dto, req, width, &show);
            // an expanded bit is derived from the parent bus window, exactly like
            // the wasm path
            let bit = self.signal_things.iter().nth(row).and_then(|item| item.bit);
            match bit {
                Some(bit) => self
                    .signal_things
                    .set_window(row, windowed::derive_bit_window(&window, bit as usize)),
                None => self.signal_things.set_window(row, window),
            }
            decoded += 1;
        }
        self.last_decoded = decoded;
        if decoded > 0 && (self.cursors.one.is_some() || self.cursors.two.is_some()) {
            self.values_need_render = true;
        }
        // the answers are for the current view: repaint with them
        self.redraw();
    }

    /// Batch the dump_value_at calls the value column needs.
    ///
    /// When a cursor is outside the row's fetched window the value column has
    /// nothing to format: ask the backend for the rendered value at that exact
    /// time, once per (row, time, format), and cache it. There is no per-row
    /// per-frame request: a redraw with the cursor already fetched does nothing,
    /// and the batch is capped so a huge row list cannot flood the IPC channel.
    #[cfg(feature = "tauri")]
    fn fetch_native_cursor_values(&mut self, rows: std::ops::Range<usize>) {
        const MAX_BATCH: usize = 64;
        let Some(session) = self.remote.clone() else {
            return;
        };
        let Some(link) = self.link.clone() else {
            return;
        };
        let cursors: Vec<i32> = [self.cursors.one, self.cursors.two]
            .into_iter()
            .flatten()
            .filter(|cursor| *cursor >= 0)
            .collect();
        if cursors.is_empty() {
            return;
        }
        let id = session.id();
        let mut batch = 0usize;
        for row in rows {
            if batch >= MAX_BATCH {
                break;
            }
            let Some(item) = self.signal_things.iter().nth(row) else {
                break;
            };
            let Some(signal) = session.signal_of_path(&item.path) else {
                continue;
            };
            // inside the fetched window the value column already has bits
            let inside_window = cursors
                .iter()
                .all(|cursor| item.values.value_at(*cursor as Time).is_some());
            if inside_window {
                continue;
            }
            let show = crate::remote::show_name(&item.setting.show_type).to_string();
            for &cursor in &cursors {
                if item.values.value_at(cursor as Time).is_some() {
                    continue;
                }
                if self.native_cursor.contains(row, cursor)
                    || self.native_cursor_inflight.contains(&(row, cursor))
                {
                    continue;
                }
                self.native_cursor_inflight.insert((row, cursor));
                batch += 1;
                let link = link.clone();
                let show = show.clone();
                spawn_local(async move {
                    // an empty text is cached too: "no change yet" is an answer,
                    // and it stops the row from being asked again for that time
                    let text = match crate::remote::value_at(id, signal.index, cursor as u64, &show)
                        .await
                    {
                        Ok(Some(text)) => text,
                        Ok(None) | Err(_) => String::new(),
                    };
                    link.send_message(Msg::RemoteCursorValue(row, cursor, text));
                });
            }
        }
    }

    /// The native session handed to the children (none in the browser build).
    fn remote_session(&self) -> Option<crate::remote::RemoteSession> {
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            return Some(session.clone());
        }
        None
    }

    /// The time range the ruler and the fit zoom use.
    ///
    /// The wasm dump owns one when it is open; a native dump only has the
    /// summary's range, which is enough for the ruler and the fit zoom.
    fn effective_time_range(&self) -> Option<TimeRange> {
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            if let Some((start, end)) = session.range() {
                return Some(TimeRange { start, end });
            }
        }
        self.time_range
    }

    /// Keep the row window cache inside the budget.
    ///
    /// Drawing no longer fills the backend's decode cache: a row holds one
    /// bounded window (`windowed::RowWindow`). The budget is still one number for
    /// the whole tab: rows outside the viewport give their window back first,
    /// then the rows at the edges of the visible range, and dumps no pane draws
    /// give their decoded data back through `memory::trim`.
    fn trim_cache(&mut self, draw: std::ops::Range<usize>) {
        let rows = self.signal_things.len();
        let keep = draw.start.saturating_sub(KEEP_ROWS)..(draw.end + KEEP_ROWS).min(rows);
        self.signal_things.drop_outside(keep);
        let open_dumps = memory::dump_handles(&self.store).len();
        let budget = memory::per_dump_share(open_dumps);
        self.window_bytes = self.signal_things.trim_windows(draw, budget);
        // the cross-dump budget: what no pane draws is given back
        let _ = memory::trim(&self.store);
    }

    /// Paint the wave canvas from the current state.
    fn redraw(&mut self) {
        let redraw_started = js_sys::Date::now();
        let metrics = self.measure();
        if !metrics.is_usable() {
            return;
        }
        let Some(canvas) = surface_element() else {
            return;
        };
        let rows = self.signal_things.len();
        let (draw, fetch) = visible_rows(self.scroll_top, metrics.surface_h, rows);
        let decode_started = js_sys::Date::now();
        self.decode_pending(fetch, metrics.surface_w);
        let decode_ms = js_sys::Date::now() - decode_started;
        if self.last_decoded > 0 && (self.cursors.one.is_some() || self.cursors.two.is_some()) {
            // one render is needed for the value column to show the new decode
            self.values_need_render = true;
        }
        self.trim_cache(draw.clone());
        // native mode: refresh the backend memory snapshot for the debug hook
        // (throttled to 1/s; the command only reads cached counters)
        #[cfg(feature = "tauri")]
        if let Some(session) = &self.remote {
            crate::remote::maybe_refresh_native_memory(session.id());
        }
        // native mode: fill the value column for cursors outside the fetched
        // windows (batched + cached, never per row per frame)
        #[cfg(feature = "tauri")]
        self.fetch_native_cursor_values(draw.clone());

        if !self.fitted {
            //first real measurement (and the first decoded rows): show the whole
            //trace. GHW has no cheap time range, so fall back to the span of the
            //histories we have.
            let end = self.trace_end(self.effective_time_range());
            self.view = View::new(0.0, View::fit_size(end, metrics.surface_w));
            self.fitted = true;
        }

        let axis = build_axis(self.view.x_axis, self.view.size, metrics.surface_w);
        let visible: Vec<(usize, &SignalItem)> = (draw.start..draw.end)
            .filter_map(|row| self.signal_things.iter().nth(row).map(|item| (row, item)))
            .collect();
        let cursors = self.shown_cursors();

        let frame = canvas::Frame {
            view: self.view,
            width: metrics.surface_w,
            height: metrics.surface_h,
            scroll_top: self.scroll_top,
            rows: &visible,
            grid: &axis.labels,
            cursor1: cursors.one,
            cursor2: cursors.two,
            drag: if self.mouse_press && (self.mouse_current_x - self.mouse_start_x).abs() > 1.0 {
                Some((self.mouse_start_x, self.mouse_current_x))
            } else {
                None
            },
            // rows stop at the end of the dump instead of holding their last
            // value over the empty surface right of it
            trace_end: self.effective_time_range().map(|range| range.end as i64),
            dpr: metrics.dpr,
        };
        let stats = canvas::paint(&canvas, &frame);
        self.draws += 1;
        // drawing caches bounded windows now; the backend's own decode cache is
        // only used by export/find, so the readout reports both
        let backend_bytes = self
            .waveform
            .as_ref()
            .map(|waveform| waveform.waveform().cache_stats().decoded_bytes())
            .unwrap_or(0);
        let decoded_bytes = backend_bytes.saturating_add(self.window_bytes);
        // the readout reports the number drawing is actually budgeted by: the
        // backend cache (export/find) plus this pane's cached row windows
        let mut memory = memory::stats(&self.store);
        memory.decoded_bytes = decoded_bytes;
        canvas::publish_debug(&canvas::DebugInfo {
            redraw_ms: js_sys::Date::now() - redraw_started,
            decode_ms,
            stats,
            visible: visible.len(),
            decoded: self.last_decoded,
            decoded_bytes,
            total_rows: rows,
            scroll_top: self.scroll_top,
            first_row: draw.start,
            cursor1: cursors.one,
            cursor2: cursors.two,
            view: self.view,
            trace_end: self.effective_time_range().map(|range| range.end as i64),
            memory,
        });
    }

    /// Read the layout back from the dom.
    fn measure(&self) -> Metrics {
        let Some(document) = document() else {
            return Metrics::default();
        };
        let dpr = web_sys::window()
            .map(|window| window.device_pixel_ratio())
            .unwrap_or(1.0);
        let surface = document
            .get_element_by_id("wave-canvas")
            .map(|element| element.get_bounding_client_rect());
        let body = document
            .get_element_by_id("wave-body")
            .map(|element| element.get_bounding_client_rect());
        let (surface_w, surface_h, surface_left) = match &surface {
            Some(rect) => (rect.width(), rect.height(), rect.left()),
            None => (0.0, 0.0, 0.0),
        };
        let body_top = body.as_ref().map(|rect| rect.top()).unwrap_or(0.0);
        Metrics {
            surface_w,
            surface_h,
            surface_left,
            body_top,
            dpr,
        }
    }

    /// Repaint whenever the canvas box changes, whatever changed it.
    ///
    /// `rendered` only runs when the component re-renders; the splitters are
    /// tier-A and move the pane by writing a css variable, so during those drags
    /// nothing renders. A `ResizeObserver` on the canvas overlay is the only
    /// signal that sees every box change, and it costs one message per change.
    fn observe_canvas(&mut self, ctx: &Context<Self>) {
        if self.resize_observer.is_some() {
            return;
        }
        let Some(target) =
            document().and_then(|document| document.get_element_by_id("wave-canvas"))
        else {
            return;
        };
        let link = ctx.link().clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            link.send_message(Msg::Resize);
        });
        let Ok(observer) = web_sys::ResizeObserver::new(callback.as_ref().unchecked_ref()) else {
            return;
        };
        observer.observe(&target);
        self.resize_observer = Some(observer);
        self.resize_callback = Some(callback);
    }

    fn attach_listeners(&mut self, ctx: &Context<Self>) {
        let window = match web_sys::window() {
            Some(window) => window,
            None => return,
        };

        //key state for the ctrl/shift modifiers and for `z`/`f`
        let key_press = Rc::clone(&self.key_press);
        let on_key = ctx.link().callback(Msg::KeyEvent);
        let keydown = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            let code = e.key_code();
            //keyCode goes past 255 with some layouts, do not index blindly
            if let Some(slot) = key_press.borrow_mut().get_mut(code as usize) {
                *slot = true;
            }
            // The zoom shortcuts are page wide, but typing `z` into the source
            // editor or a filter box must not zoom the waveform.
            if !typing_in_text_field(&e) {
                on_key.emit(code);
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        //`addEventListener`, not the `onkeydown` property: monaco installs its own
        //window handlers when the editor takes focus, and a property handler is
        //simply replaced (which is why Delete stopped working once the source
        //pane existed)
        let _ =
            window.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref());

        let key_press = Rc::clone(&self.key_press);
        let keyup = Closure::wrap(Box::new(move |e: web_sys::KeyboardEvent| {
            if let Some(slot) = key_press.borrow_mut().get_mut(e.key_code() as usize) {
                *slot = false;
            }
        }) as Box<dyn FnMut(web_sys::KeyboardEvent)>);
        let _ = window.add_event_listener_with_callback("keyup", keyup.as_ref().unchecked_ref());

        let on_resize = ctx.link().callback(|_: ()| Msg::Resize);
        let onresize = Closure::wrap(Box::new(move |_: JsValue| {
            on_resize.emit(());
        }) as Box<dyn FnMut(JsValue)>);
        let _ =
            window.add_event_listener_with_callback("resize", onresize.as_ref().unchecked_ref());

        self.keydown = Some(keydown);
        self.keyup = Some(keyup);
        self.onresize = Some(onresize);
    }
}

fn document() -> Option<web_sys::Document> {
    web_sys::window().and_then(|window| window.document())
}

/// The canvas element the wave body is painted into.
fn surface_element() -> Option<web_sys::HtmlCanvasElement> {
    document()?
        .get_element_by_id("wave-surface")?
        .dyn_into::<web_sys::HtmlCanvasElement>()
        .ok()
}

/// Write the label-column width straight to the pane's css variable.
///
/// Tier A: the splitter drags the column at pointer speed and commits the width
/// to the component state (and the store does not own it) only on mouseup, so
/// nothing here may go through Yew.
fn write_names_width(width: f64) {
    let Some(element) = document().and_then(|document| document.get_element_by_id("wave-pane"))
    else {
        return;
    };
    let Ok(style) = js_sys::Reflect::get(&element, &JsValue::from_str("style")) else {
        return;
    };
    let Ok(set_property) = js_sys::Reflect::get(&style, &JsValue::from_str("setProperty"))
        .and_then(|function| function.dyn_into::<js_sys::Function>())
    else {
        return;
    };
    let args = js_sys::Array::new();
    args.push(&JsValue::from_str("--names-w"));
    args.push(&JsValue::from_str(&format!("{width:.0}px")));
    let _ = set_property.apply(&style, &args);
}

/// Scroll the row list by `delta` css pixels (the browser clamps the range).
///
/// Shift+wheel scrolls the rows: the list is the scroller and its `scroll`
/// event already updates `scroll_top` and re-renders the virtualized name
/// column, so the gesture itself never has to write component state.
fn scroll_rows_by(delta: f64) {
    if !delta.is_finite() || delta == 0.0 {
        return;
    }
    let Some(element) = document().and_then(|document| document.get_element_by_id("wave-body"))
    else {
        return;
    };
    element.set_scroll_top((element.scroll_top() as f64 + delta).max(0.0) as i32);
}

/// Current scroll offset of the row list.
fn scroll_top_from_dom() -> f64 {
    document()
        .and_then(|document| document.get_element_by_id("wave-body"))
        .map(|element| element.scroll_top() as f64)
        .unwrap_or(0.0)
}

/// Is the key event aimed at a text field (the source editor, a filter box, a
/// number input in the settings dialog)?
///
/// The wave area listens on the window, so without this a `z` typed into a
/// comment would also zoom the waveform.
fn typing_in_text_field(event: &web_sys::KeyboardEvent) -> bool {
    let Some(target) = event.target() else {
        return false;
    };
    let Ok(element) = target.dyn_into::<web_sys::Element>() else {
        return false;
    };
    let mut current = Some(element);
    while let Some(element) = current {
        let tag = element.tag_name().to_ascii_lowercase();
        if tag == "input" || tag == "textarea" || tag == "select" {
            return true;
        }
        if element.has_attribute("contenteditable") {
            return true;
        }
        // monaco keeps its input in a hidden textarea inside `.monaco-editor`;
        // this also covers the hover card and the tab strip focus
        if element
            .get_attribute("class")
            .map(|class| class.contains("monaco-editor"))
            .unwrap_or(false)
        {
            return true;
        }
        current = element.parent_element();
    }
    false
}

/// Longest value text, in characters, one row can print.
///
/// The wave view prints bare digits (no Verilog radix prefix), so the length is
/// fully determined by the bit width and the row's number format: hex needs
/// `bits/4` digits, oct `bits/3`, bin one per bit, unsigned decimal
/// `ceil(bits * log10 2)` and signed decimal one more for the sign; ascii is
/// bounded by the bit count (a safe upper bound). A fixed-point row prints
/// `value / 2^f` with one decimal and falls back to the base format when the
/// value has no number, so the worse of the two is taken.
///
/// Pure and host tested: the value column width is computed from this before any
/// value is decoded, so it cannot jump as a scroll decodes a wider row.
pub fn value_text_chars(
    bits: usize,
    is_bool: bool,
    show: &ShowType,
    fixed_point: Option<u32>,
) -> usize {
    let bits = bits.max(1);
    if is_bool || bits <= 1 {
        return 1;
    }
    let base = match show {
        ShowType::Hex => bits.div_ceil(4),
        ShowType::Oct => bits.div_ceil(3),
        ShowType::Bin => bits,
        ShowType::UInt => decimal_chars(bits, false),
        ShowType::SInt => decimal_chars(bits, true),
        ShowType::Ascii => bits,
    };
    match fixed_point {
        Some(_) => base.max(VALUE_FIXED_POINT_CHARS),
        None => base,
    }
}

/// Characters of an unsigned/signed decimal rendering of `bits` bits.
fn decimal_chars(bits: usize, signed: bool) -> usize {
    let digits = (bits as f64 * std::f64::consts::LOG10_2).ceil() as usize;
    digits + usize::from(signed)
}

/// Width of the cursor value columns, in characters of the value font.
///
/// All rows share one width, and all value columns together may take at most
/// `VALUE_COLUMN_SHARE` of the label column: a 64-bit binary value must not
/// squeeze the signal name to nothing. A single character's width is only
/// estimated here (the exact width is `1ch` in CSS); the cap is what matters,
/// and it is deliberately a little conservative. Non-finite or tiny label
/// widths degrade to the minimum column instead of a panic.
pub fn value_column_chars(widest: usize, columns: usize, names_w: f64) -> usize {
    let columns = columns.max(1) as f64;
    let budget = (names_w.max(0.0) * VALUE_COLUMN_SHARE / columns) - VALUE_COLUMN_PAD_PX;
    let cap = (budget / VALUE_CHAR_PX)
        .floor()
        .max(VALUE_COLUMN_MIN_CHARS as f64) as usize;
    widest.clamp(VALUE_COLUMN_MIN_CHARS, cap)
}

/// The dominant wheel delta, in css pixels, with `delta_mode` resolved
/// (0 = pixels, 1 = lines, 2 = pages, the DOM constants).
///
/// A trackpad reports a sideways swipe on `delta_x` and a vertical one on
/// `delta_y`; the dominant axis wins so a diagonal gesture is never applied
/// twice. Positive means down / right, i.e. later in time. Non-finite deltas
/// (some synthetic events report them) count as zero.
pub fn wheel_delta_px(delta_y: f64, delta_x: f64, delta_mode: u32) -> f64 {
    if !delta_y.is_finite() || !delta_x.is_finite() {
        return 0.0;
    }
    let dominant = if delta_x.abs() > delta_y.abs() {
        delta_x
    } else {
        delta_y
    };
    let factor = match delta_mode {
        1 => WHEEL_LINE_PX,
        2 => WHEEL_PAGE_PX,
        _ => 1.0,
    };
    dominant * factor
}

/// Horizontal pan of a plain wheel event, in css pixels: wheel down / swipe
/// right = forward in time. Clamped per event so one notch cannot jump the
/// whole trace.
pub fn wheel_pan_px(delta_y: f64, delta_x: f64, delta_mode: u32) -> f64 {
    wheel_delta_px(delta_y, delta_x, delta_mode).clamp(-WHEEL_PAN_MAX_PX, WHEEL_PAN_MAX_PX)
}

/// A keyboard gesture the wave pane owns (row selection movement).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveKey {
    Up,
    Down,
    Home,
    End,
}

/// Map a `keyCode` + modifiers to the row-selection gesture, if any.
///
/// Pure on purpose: the mapping is the part that can be wrong, and it is unit
/// tested on the host. The modifiers are taken so a future Ctrl+Arrow ("move the
/// view") cannot be confused with the plain selection keys.
pub fn wave_key_action(code: u32, ctrl: bool, shift: bool) -> Option<WaveKey> {
    // Ctrl+Arrow is a view gesture in other tools, and Shift+Arrow a range
    // selection; neither is wired, so both fall through to the other shortcuts.
    if ctrl || shift {
        return None;
    }
    match code {
        38 => Some(WaveKey::Up),
        40 => Some(WaveKey::Down),
        36 => Some(WaveKey::Home),
        35 => Some(WaveKey::End),
        _ => None,
    }
}

#[cfg(test)]
mod key_tests {
    use super::wave_key_action;
    use super::WaveKey;

    #[test]
    fn the_arrow_keys_move_the_selection() {
        assert_eq!(wave_key_action(38, false, false), Some(WaveKey::Up));
        assert_eq!(wave_key_action(40, false, false), Some(WaveKey::Down));
        assert_eq!(wave_key_action(36, false, false), Some(WaveKey::Home));
        assert_eq!(wave_key_action(35, false, false), Some(WaveKey::End));
    }

    #[test]
    fn other_keys_and_modifiers_are_left_to_the_existing_shortcuts() {
        // z / f / n / ctrl+a / esc / del keep their own branches
        for code in [90u32, 70, 78, 65, 27, 46, 8, 16, 17] {
            assert_eq!(wave_key_action(code, false, false), None, "keyCode {code}");
        }
        // ctrl+arrow is not a selection gesture
        assert_eq!(wave_key_action(38, true, false), None);
        assert_eq!(wave_key_action(40, true, false), None);
    }
}

#[cfg(test)]
mod value_and_wheel_tests {
    use super::value_column_chars;
    use super::value_text_chars;
    use super::wheel_delta_px;
    use super::wheel_pan_px;
    use super::ShowType;
    use super::VALUE_COLUMN_MIN_CHARS;
    use super::VALUE_FIXED_POINT_CHARS;
    use super::WHEEL_LINE_PX;
    use super::WHEEL_PAGE_PX;
    use super::WHEEL_PAN_MAX_PX;

    #[test]
    fn value_text_chars_counts_bare_digits() {
        // a single bit is one character, whatever the base
        assert_eq!(value_text_chars(1, true, &ShowType::Hex, None), 1);
        assert_eq!(value_text_chars(1, false, &ShowType::Bin, None), 1);
        // radix digits: no Verilog prefix in the wave view any more
        assert_eq!(value_text_chars(8, false, &ShowType::Hex, None), 2);
        assert_eq!(value_text_chars(32, false, &ShowType::Hex, None), 8);
        assert_eq!(value_text_chars(32, false, &ShowType::Oct, None), 11);
        assert_eq!(value_text_chars(32, false, &ShowType::Bin, None), 32);
        // decimal: 32 bits hold 10 digits, signed adds the sign
        assert_eq!(value_text_chars(32, false, &ShowType::UInt, None), 10);
        assert_eq!(value_text_chars(32, false, &ShowType::SInt, None), 11);
        assert_eq!(value_text_chars(64, false, &ShowType::UInt, None), 20);
        // an unknown width is still one bit, not zero characters
        assert_eq!(value_text_chars(0, false, &ShowType::Hex, None), 1);
        // fixed point prints a scaled decimal reading (or falls back to base)
        assert_eq!(
            value_text_chars(32, false, &ShowType::Hex, Some(4)),
            VALUE_FIXED_POINT_CHARS
        );
    }

    #[test]
    fn value_column_chars_is_capped_so_the_name_keeps_room() {
        // the widest value wins when it fits...
        assert_eq!(value_column_chars(8, 1, 220.0), 8);
        // ...but 40 % of the label column is all the columns may take: a
        // 64-bit binary value is truncated instead of squeezing the name out
        assert_eq!(value_column_chars(64, 1, 220.0), 12);
        // two cursor columns share the same budget
        assert_eq!(value_column_chars(64, 2, 220.0), 5);
        // a wide label column lets a 64-bit hex value through
        assert_eq!(value_column_chars(16, 1, 640.0), 16);
        // degenerate widths clamp to the minimum instead of panicking
        assert_eq!(value_column_chars(64, 2, 100.0), VALUE_COLUMN_MIN_CHARS);
        assert_eq!(value_column_chars(64, 1, f64::NAN), VALUE_COLUMN_MIN_CHARS);
    }

    #[test]
    fn wheel_pixel_deltas_pan_by_their_delta() {
        assert_eq!(wheel_pan_px(120.0, 0.0, 0), 120.0);
        assert_eq!(wheel_pan_px(-120.0, 0.0, 0), -120.0);
        // one event is clamped, so one notch cannot jump the whole trace
        assert_eq!(wheel_pan_px(1000.0, 0.0, 0), WHEEL_PAN_MAX_PX);
        assert_eq!(wheel_pan_px(-1000.0, 0.0, 0), -WHEEL_PAN_MAX_PX);
    }

    #[test]
    fn wheel_delta_mode_is_honoured() {
        assert_eq!(wheel_delta_px(3.0, 0.0, 1), 3.0 * WHEEL_LINE_PX);
        assert_eq!(wheel_delta_px(1.0, 0.0, 2), WHEEL_PAGE_PX);
        // line mode is scaled before the clamp applies
        assert_eq!(wheel_pan_px(100.0, 0.0, 1), WHEEL_PAN_MAX_PX);
    }

    #[test]
    fn wheel_uses_the_dominant_axis_only() {
        // a sideways trackpad swipe pans the time axis
        assert_eq!(wheel_pan_px(0.0, -80.0, 0), -80.0);
        // a diagonal gesture is applied once (the larger axis wins)
        assert_eq!(wheel_pan_px(40.0, -80.0, 0), -80.0);
        assert_eq!(wheel_pan_px(-120.0, 30.0, 0), -120.0);
        // the rows gesture uses the same resolution, unscaled by the pan clamp
        assert_eq!(wheel_delta_px(0.0, 200.0, 0), 200.0);
    }

    #[test]
    fn wheel_ignores_non_finite_deltas() {
        assert_eq!(wheel_pan_px(f64::NAN, 0.0, 0), 0.0);
        assert_eq!(wheel_pan_px(0.0, f64::INFINITY, 0), 0.0);
    }
}
