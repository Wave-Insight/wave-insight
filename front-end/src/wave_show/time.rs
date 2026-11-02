//! The time ruler above the wave area.
//!
//! R2 step 2 (B2): the pane *subscribes* to the store's committed viewport and
//! cursors instead of being handed numbers as props. A zoom/pan or a cursor
//! click is committed to the store once (on mouseup / one wheel notch); this
//! component re-renders from that subscription, while the wave pane only
//! repaints its canvas - the row list is never diffed for a gesture.
//!
//! The ruler is also an input device, the way it is in Verdi: dragging it pans
//! the trace, a click puts cursor 1 there (right click cursor 2), and the
//! readout on its left takes a time to jump to.

use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;

#[cfg(feature = "tauri")]
use wasm_bindgen_futures::spawn_local;
use wave_insight_lib::waveform::TimeRange;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::state::{self, select, Cursors, Store, Subscription, View, Viewport};

///A computed time ruler.
///
///The wave area draws its vertical grid on `labels`, so the grid and the numbers
///above it always line up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Axis {
    ///the tick comb drawn by the time bar
    pub points: String,
    ///x position and time value of the ticks that get a number
    pub labels: Vec<(f64, i64)>,
}

/// A ruler drag in progress: where the pointer went down and the pan the view had
/// at that moment.
///
/// Tier A (docs/state-store.md §4.1): the drag only writes a preview transform
/// on the ruler's content group, so a 60 Hz pan costs no Yew render and no store
/// dispatch. The single commit happens on mouseup.
#[derive(Clone, Copy)]
struct Drag {
    /// client x of the mousedown
    client_x: f64,
    /// `view.x_axis` at the mousedown
    x_axis: f64,
    /// the button (0 = cursor 1 on a click, 2 = cursor 2)
    button: i16,
    /// `true` once the pointer moved far enough to be a pan, not a click
    moved: bool,
    /// the pan the preview shows; committed on mouseup
    preview_axis: Option<f64>,
}

pub enum Msg {
    /// The committed viewport/cursors changed.
    Viewport(Viewport),
    /// The dump's time range (the ruler clamps pans to it).
    TimeRange(Option<TimeRange>),
    /// The ruler was grabbed.
    RulerDown(MouseEvent),
    /// The pointer moved while the ruler was grabbed (window listener).
    RulerMove(MouseEvent),
    /// The button came up: either the pan is finished or it was a click.
    RulerUp(MouseEvent),
    /// The ruler box was submitted: an integer is a time to go to, anything
    /// else is a value to find. `backward` is the Shift+Enter direction.
    Goto { text: String, backward: bool },
    /// A native value search answered: (hit or error, row name, query, backward).
    #[cfg(feature = "tauri")]
    RemoteFind(
        Result<Option<crate::remote::FindHitDto>, String>,
        String,
        String,
        bool,
    ),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct TimeProps {
    /// The app store: the ruler subscribes to `viewport` itself.
    pub store: Store,
    /// Width of the wave surface, measured by the pane.
    pub width: f64,
    /// The native dump being drawn read-only (desktop build), when one is open:
    /// the value box then searches it over IPC instead of the wasm store path.
    #[prop_or_default]
    pub remote: Option<crate::remote::RemoteSession>,
}

pub struct Time {
    points: String,
    labels: Vec<(f64, i64)>,
    viewport: Viewport,
    width: f64,
    store: Store,
    time_range: Option<TimeRange>,
    /// The native dump the value box searches (desktop build only).
    remote: Option<crate::remote::RemoteSession>,
    drag: Option<Drag>,
    /// window listeners: the pan must follow the pointer outside the 30 px ruler
    on_move: Option<Closure<dyn FnMut(MouseEvent)>>,
    on_up: Option<Closure<dyn FnMut(MouseEvent)>>,
    _viewport_sub: Subscription,
    _range_sub: Subscription,
}

impl Component for Time {
    type Message = Msg;
    type Properties = TimeProps;

    fn create(ctx: &Context<Self>) -> Self {
        let store = ctx.props().store.clone();
        let subscription = store.subscribe(select::viewport, ctx.link().callback(Msg::Viewport));
        let range_sub = store.subscribe(select::time_range, ctx.link().callback(Msg::TimeRange));
        let viewport = store.select(select::viewport);
        let axis = build_axis(viewport.view.x_axis, viewport.view.size, ctx.props().width);
        let time_range = store.select(select::time_range);
        let mut this = Self {
            points: axis.points,
            labels: axis.labels,
            viewport,
            width: ctx.props().width,
            store,
            time_range,
            remote: ctx.props().remote.clone(),
            drag: None,
            on_move: None,
            on_up: None,
            _viewport_sub: subscription,
            _range_sub: range_sub,
        };
        this.attach_listeners(ctx);
        this
    }

    #[allow(unused_variables)] // ctx is used by the tauri-gated native find arm
    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            #[cfg(feature = "tauri")]
            Msg::RemoteFind(result, name, query, backward) => {
                match result {
                    Ok(Some(hit)) => {
                        let mut cursors = self.viewport.cursors;
                        cursors.one = Some(hit.time.min(i32::MAX as u64) as i32);
                        self.store.dispatch(state::Action::CursorsChanged(cursors));
                        // bring the match into the left quarter, like the wasm find
                        let view = crate::remote::view_for_time(
                            self.viewport.view,
                            self.width,
                            hit.time as i64,
                        );
                        self.commit_view(view);
                        self.store
                            .dispatch(state::Action::Status(state::Status::info(
                                crate::remote::find_status(&name, &query, &hit),
                            )));
                    }
                    Ok(None) => {
                        self.store
                            .dispatch(state::Action::Status(state::Status::error(
                                crate::remote::find_missing_status(&name, &query, backward),
                            )));
                    }
                    Err(error) => {
                        self.store
                            .dispatch(state::Action::Status(state::Status::error(format!(
                                "native dump: value search failed ({error})"
                            ))));
                    }
                }
                true
            }
            Msg::Viewport(viewport) => {
                self.viewport = viewport;
                self.rebuild();
                true
            }
            Msg::TimeRange(range) => {
                self.time_range = range;
                false
            }
            Msg::RulerDown(e) => {
                e.prevent_default();
                self.drag = Some(Drag {
                    client_x: e.x() as f64,
                    x_axis: self.viewport.view.x_axis,
                    button: e.button(),
                    moved: false,
                    preview_axis: None,
                });
                true
            }
            Msg::RulerMove(e) => {
                let Some(mut drag) = self.drag else {
                    return false;
                };
                let delta = e.x() as f64 - drag.client_x;
                if delta.abs() < 2.0 && !drag.moved {
                    return false;
                }
                drag.moved = true;
                // the pan the preview shows, clamped exactly like the commit
                // would be, so the ruler cannot slide past the end of the dump
                let mut view = self.viewport.view;
                view.x_axis = (drag.x_axis - delta / view.size.max(1e-9)).max(0.0);
                let view = self.clamp_view(view);
                drag.preview_axis = Some(view.x_axis);
                self.drag = Some(drag);
                // tier A: the content group moves, nothing is dispatched
                let shift = (drag.x_axis - view.x_axis) * self.viewport.view.size;
                set_content_offset(shift);
                false
            }
            Msg::RulerUp(e) => {
                let Some(drag) = self.drag.take() else {
                    return false;
                };
                // the preview transform is not part of the virtual dom, so it
                // must be reset by hand before the committed ruler renders
                set_content_offset(0.0);
                if drag.moved {
                    // one dispatch for the whole pan, on mouseup
                    if let Some(x_axis) = drag.preview_axis {
                        let mut view = self.viewport.view;
                        view.x_axis = x_axis;
                        self.commit_view(view);
                    }
                    return true;
                }
                // a click on the ruler places a cursor (Verdi's time marker)
                let Some(local_x) = self.local_x(e.x() as f64) else {
                    return false;
                };
                let time = self.viewport.view.time_at(local_x.max(0.0));
                let mut cursors: Cursors = self.viewport.cursors;
                if drag.button == 2 {
                    cursors.two = Some(time);
                } else {
                    cursors.one = Some(time);
                }
                self.store.dispatch(state::Action::CursorsChanged(cursors));
                true
            }
            Msg::Goto { text, backward } => {
                // an integer is a time (the old goto), anything else is a value
                // to find in the selected row (Verdi's find-value box)
                let Ok(time) = text.trim().parse::<i64>() else {
                    // the desktop build may be drawing a native dump: search it
                    // over IPC (same box, same Shift+Enter direction)
                    #[cfg(feature = "tauri")]
                    if self.remote.is_some() {
                        self.find_native(ctx, text, backward);
                        return false;
                    }
                    let direction = if backward {
                        super::find::Direction::Previous
                    } else {
                        super::find::Direction::Next
                    };
                    super::find::find_value(&self.store, &text, direction, self.width);
                    return false;
                };
                let time = time.max(0);
                let mut cursors: Cursors = self.viewport.cursors;
                cursors.one = Some(time.min(i32::MAX as i64) as i32);
                self.store.dispatch(state::Action::CursorsChanged(cursors));
                // bring it into the left quarter of the window
                let mut view = self.viewport.view;
                let span = view.time_span(self.width);
                view.x_axis = (time as f64 - span * 0.25).max(0.0);
                self.commit_view(view);
                true
            }
        }
    }

    fn changed(&mut self, ctx: &Context<Self>, _old_props: &Self::Properties) -> bool {
        let mut changed = false;
        if (self.width - ctx.props().width).abs() > 0.5 {
            self.width = ctx.props().width;
            self.rebuild();
            changed = true;
        }
        if self.remote != ctx.props().remote {
            self.remote = ctx.props().remote.clone();
            changed = true;
        }
        changed
    }

    fn destroy(&mut self, _ctx: &Context<Self>) {
        if let Some(window) = web_sys::window() {
            if let Some(handler) = self.on_move.as_ref() {
                let _ = window.remove_event_listener_with_callback(
                    "mousemove",
                    handler.as_ref().unchecked_ref(),
                );
            }
            if let Some(handler) = self.on_up.as_ref() {
                let _ = window.remove_event_listener_with_callback(
                    "mouseup",
                    handler.as_ref().unchecked_ref(),
                );
            }
        }
        self.on_move = None;
        self.on_up = None;
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let view = self.viewport.view;
        let cursors = self.viewport.cursors;
        let delta = cursors.one.zip(cursors.two).map(|(x, y)| y - x);
        let goto = {
            let link = ctx.link().clone();
            Callback::from(move |e: KeyboardEvent| {
                let input = e.target_unchecked_into::<HtmlInputElement>();
                // Escape leaves the box without jumping and without clearing the
                // wave row selection (the page wide handler must not see it)
                if e.key() == "Escape" {
                    e.stop_propagation();
                    e.prevent_default();
                    input.set_value("");
                    let _ = input.blur();
                    return;
                }
                if e.key() != "Enter" {
                    return;
                }
                let value = input.value();
                let backward = e.shift_key();
                input.set_value("");
                // the wave pane listens for keys page wide; a digit typed into
                // this box must not reach it
                e.stop_propagation();
                e.prevent_default();
                // and give the keyboard back to the wave area: after a goto the
                // next key (n, del, ctrl+a) is a waveform shortcut, not typing
                let _ = input.blur();
                link.send_message(Msg::Goto {
                    text: value,
                    backward,
                });
            })
        };
        html! {
            <div class="timebar">
                <div class="timebar-readout">
                    <input class="timebar-goto" type="text"
                        placeholder="time / value"
                        aria-label="Go to time or find a value: type an integer to move cursor 1 there, or a signal value and press Enter to jump to the next change with that value (Shift+Enter for the previous one)"
                        title="Type a time and press Enter: cursor 1 goes there. Type a value (e.g. 'h2a) and press Enter: jump to the next change with that value; Shift+Enter for the previous one"
                        onkeydown={goto} />
                    <code>{
                        match delta {
                            Some(cur) => format!("Δ {cur}"),
                            None => "Δ -".to_string(),
                        }
                    }</code>
                </div>
                <svg class={if self.drag.is_some() {"timebar-svg dragging"} else {"timebar-svg"}}
                    role="slider" aria-label="Time ruler: drag to pan, click to set cursor 1"
                    aria-valuemin="0"
                    aria-valuenow={format!("{:.0}", self.viewport.view.x_axis)}
                    tabindex="0"
                    onmousedown={ctx.link().callback(Msg::RulerDown)}
                    oncontextmenu={Callback::from(|e: MouseEvent| e.prevent_default())}>
                    // the tier-A drag preview translates this group instead of
                    // re-rendering the ~100 labels it contains
                    <g class="timebar-content">
                    {
                        for self.labels.iter().map(|&(x, time)| {
                            html!{
                                <text class="axis-text" x={format!("{x:.2}")} y={14}>
                                    {format!("{time}")}
                                </text>
                            }
                        })
                    }
                    {
                        if let Some(c1) = cursors.one {html!{
                            <text class="axis-text cursor-a"
                                x={format!("{}",((c1 as f64) - view.x_axis)*view.size)}
                                y={format!("{}",13)}>
                                {format!("{c1}")}
                            </text>
                        }}else {
                            html!{}
                        }
                    }
                    {
                        if let Some(c2) = cursors.two {html!{
                            <text class="axis-text cursor-b"
                                x={format!("{}",((c2 as f64) - view.x_axis)*view.size)}
                                y={format!("{}",13)}>
                                {format!("{c2}")}
                            </text>
                        }}else {
                            html!{}
                        }
                    }
                    <polyline class="axis-line" points={self.points.clone()} />
                    </g>
                </svg>
            </div>
        }
    }
}

impl Time {
    /// Search the native dump from the ruler box (desktop build).
    ///
    /// The backend command is dump_find { id, signal, query, from, backwards,
    /// show }: one IPC call per Enter, no whole-trace materialisation in wasm.
    /// The answer comes back as a message ([`Msg::RemoteFind`]) which commits the
    /// cursor/view and reports the value or the miss.
    #[cfg(feature = "tauri")]
    fn find_native(&self, ctx: &Context<Self>, query: String, backward: bool) {
        let Some(remote) = self.remote.clone() else {
            return;
        };
        let rows = self.store.select(select::rows);
        let Some(row) = rows.items.iter().find(|row| row.chosen).cloned() else {
            self.store
                .dispatch(state::Action::Status(state::Status::error(
                    "click a wave row first, then type a value to find it",
                )));
            return;
        };
        let Some(signal) = remote.signal_of_path(&row.path) else {
            self.store
                .dispatch(state::Action::Status(state::Status::error(format!(
                    "{} is not in the native dump",
                    row.name
                ))));
            return;
        };
        // dump_find takes show: ShowType (serde names), and its own query form
        let show = crate::remote::show_serde_name(&row.setting.show_type).to_string();
        // forwards starts strictly after the cursor (0 = the start of the trace);
        // backwards starts from the end when there is no cursor
        let from = match self.viewport.cursors.one {
            Some(cursor) => cursor.max(0) as u64,
            None if backward => u64::MAX,
            None => 0,
        };
        let echo = query.clone();
        let query = crate::remote::find_query(&query);
        let link = ctx.link().clone();
        let name = row.name.clone();
        let id = remote.id();
        spawn_local(async move {
            let result = crate::remote::find(id, signal.index, &query, from, backward, &show).await;
            link.send_message(Msg::RemoteFind(result, name, echo, backward));
        });
    }

    fn attach_listeners(&mut self, ctx: &Context<Self>) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let move_link = ctx.link().clone();
        let moving = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            move_link.send_message(Msg::RulerMove(event));
        });
        let up_link = ctx.link().clone();
        let up = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
            up_link.send_message(Msg::RulerUp(event));
        });
        let _ =
            window.add_event_listener_with_callback("mousemove", moving.as_ref().unchecked_ref());
        let _ = window.add_event_listener_with_callback("mouseup", up.as_ref().unchecked_ref());
        self.on_move = Some(moving);
        self.on_up = Some(up);
    }

    fn rebuild(&mut self) {
        let axis = build_axis(
            self.viewport.view.x_axis,
            self.viewport.view.size,
            self.width,
        );
        self.points = axis.points;
        self.labels = axis.labels;
    }

    /// `view`, pulled back so its right edge is at or before the end of the
    /// trace. The drag preview and the commit use the same clamp, so the ruler
    /// does not jump on mouseup.
    fn clamp_view(&self, view: View) -> View {
        match self.time_range {
            Some(range) => view.clamp_to_end(range.end as i64, self.width),
            None => view,
        }
    }

    /// Commit a pan: clamp it inside the dump, then hand it to the store.
    fn commit_view(&mut self, view: View) {
        let view = self.clamp_view(view);
        self.store.dispatch(state::Action::ViewCommitted(view));
    }

    /// Pointer position in ruler pixels (the svg starts right of the readout).
    fn local_x(&self, client_x: f64) -> Option<f64> {
        let document = web_sys::window()?.document()?;
        let element = document.query_selector(".timebar-svg").ok()??;
        Some(client_x - element.get_bounding_client_rect().left())
    }
}

/// Translate the ruler's content group by `shift` css pixels (tier-A preview).
///
/// The transform is deliberately not part of the virtual dom: the drag writes it
/// straight to the element, and `RulerUp` resets it before the committed ruler
/// renders. A missing element (a detached ruler) is a no-op.
fn set_content_offset(shift: f64) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(group) = document.query_selector(".timebar-content").ok().flatten() else {
        return;
    };
    let _ = group.set_attribute("transform", &format!("translate({shift:.2} 0)"));
}

///Build the ruler for one zoom/pan state. Used by the time bar itself and by the
///wave area, which draws its grid on the same tick positions.
pub fn build_axis(x_axis: f64, size: f64, width: f64) -> Axis {
    let mut points = "".to_owned();
    let mut scale = (width / size / 10.0 / 40.0).floor(); //TODO:10 is freq of clk
    scale = if scale < 1.0 { 1.0 } else { scale };

    //one tick every `scale*10` time units, label every n-th tick so the numbers
    //stay readable when zoomed in
    let tick_px = scale * 10.0 * size;
    let label_step = ((44.0 / tick_px).ceil() as usize).max(1);
    let head = (x_axis / scale / 10.0).floor();
    let mut labels = Vec::new();

    for d in 0..100 {
        let time = ((d as f64) - 1.0 + head) * scale * 10.0;
        let x = (time - x_axis) * size;
        points.push_str(&format!("{:.2},{} {:.2},{} {:.2},{} ", x, 30, x, 22, x, 30));
        //TODO:height should base on html
        if d % label_step == 0 {
            labels.push((x, time as i64));
        }
    }
    Axis { points, labels }
}
