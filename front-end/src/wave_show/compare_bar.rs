//! The comparison toolbar: load extra dumps under the active tab's rows.
//!
//! One dump is the base (slot 1, the tab itself); the toolbar loads up to seven
//! further dumps as *members* (slots 2..=8). A member is not a tab: it has no
//! rows of its own, it is never persisted, and it is drawn as one extra series
//! on every row whose signal it has, in [`compare`](crate::state::compare)'s
//! colour for its slot.
//!
//! The picker is a plain `<input type=file>` on purpose: it works in the browser
//! and in the desktop webview alike, and the bytes are parsed by the same
//! `Waveform::open_bytes` path the wasm loader uses. The native (read-only,
//! mmap) mode has no wasm waveform to compare against, so the toolbar shows its
//! refusal there instead of a button.
//!
//! The rule the feature is built around: adding a signal never asks which dumps
//! to take it from. A row is keyed by its hierarchical path, and the pane draws
//! one series per member that resolves that path - so "add it from every dump
//! that has it" is what the model does, not a special case in this toolbar.

use wasm_bindgen_futures::{spawn_local, JsFuture};
use yew::prelude::*;

use crate::state::compare::{self, CompareSet};
use crate::state::types::{DumpHandle, Status};
use crate::state::{select, Action, Store, Subscription};
use wave_insight_lib::waveform::Waveform;

/// The largest comparison dump the wasm path reads in one go.
///
/// The bytes are parsed into the same wasm memory the base dump lives in, and
/// this is a side feature, so it does not get to exhaust the tab: a bigger file
/// is refused with what to do about it (compare a smaller dump, or open it as
/// the base dump instead).
pub const MAX_COMPARE_BYTES: u64 = 128 * 1024 * 1024;

pub enum Msg {
    /// The active tab's set changed (loaded, removed, tab switch).
    Compare(CompareSet),
    /// A dump became active / every dump closed (the toolbar is hidden then).
    Dumps(bool),
    /// A read started (`Some(file name)`) or finished/dispatching (`None`).
    Busy(Option<String>),
}

#[derive(Properties, PartialEq, Clone)]
pub struct CompareBarProps {
    pub store: Store,
    /// Native (read-only, mmap) mode: no wasm waveform exists to overlay, so the
    /// toolbar explains why instead of offering a picker.
    #[prop_or_default]
    pub disabled: bool,
}

pub struct CompareBar {
    store: Store,
    compare: CompareSet,
    has_dump: bool,
    busy: Option<String>,
    _compare_sub: Subscription,
    _dumps_sub: Subscription,
}

impl Component for CompareBar {
    type Message = Msg;
    type Properties = CompareBarProps;

    fn create(ctx: &Context<Self>) -> Self {
        let store = ctx.props().store.clone();
        let compare = store.select(select::compare);
        let has_dump = store.select(select::has_dump);
        let compare_sub = store.subscribe(select::compare, ctx.link().callback(Msg::Compare));
        let dumps_sub = store.subscribe(select::has_dump, ctx.link().callback(Msg::Dumps));
        Self {
            store,
            compare,
            has_dump,
            busy: None,
            _compare_sub: compare_sub,
            _dumps_sub: dumps_sub,
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Compare(compare) => {
                self.compare = compare;
                true
            }
            Msg::Dumps(has_dump) => {
                self.has_dump = has_dump;
                true
            }
            Msg::Busy(busy) => {
                self.busy = busy;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        if !self.has_dump {
            return html! {};
        }
        let base_name = self
            .store
            .select(select::dump_name)
            .unwrap_or_else(|| "waveform 1".to_string());
        let base_title = format!("waveform 1 — {base_name}");
        let base = html! {
            <span class="compare-chip compare-base" title={base_title}>
                <span class="compare-slot"
                    style={format!("--slot-color:{}", compare::rgb(compare::color_of(1)))}>
                    {"1"}
                </span>
                <span class="compare-name">{base_name}</span>
            </span>
        };

        let members: Vec<Html> = self
            .compare
            .members()
            .map(|member| {
                let slot = member.slot;
                // the chip's tooltip is the loaded-status line: the file, its
                // size and its range (the number on the chip stays the slot)
                let title =
                    compare::loaded_status(slot, &member.name, member.bytes, member.time_range);
                let remove = ctx.link().callback(move |_: MouseEvent| {
                    // the reducer drops the member; the status says which one left
                    Msg::Busy(None)
                });
                let _ = remove;
                let store = self.store.clone();
                let name = member.name.clone();
                let onclick = Callback::from(move |event: MouseEvent| {
                    event.stop_propagation();
                    store.dispatch(Action::CompareMemberRemoved { slot });
                    store.dispatch(Action::Status(Status::info(compare::removed_status(slot))));
                    let _ = &name;
                });
                html! {
                    <span class="compare-chip" title={title.clone()}>
                        <span class="compare-slot"
                            style={format!("--slot-color:{}", compare::rgb(member.color))}>
                            {slot.to_string()}
                        </span>
                        <span class="compare-name">{member.name.clone()}</span>
                        <button type="button" class="compare-remove"
                            aria-label={format!("Remove waveform {slot} from the comparison")}
                            title={format!("Remove {} from the comparison", title)}
                            onclick={onclick}>
                            {"×"}
                        </button>
                    </span>
                }
            })
            .collect();

        let add = if ctx.props().disabled {
            html! {
                <span class="compare-note" title={compare::NATIVE_REFUSAL}>
                    {compare::NATIVE_REFUSAL}
                </span>
            }
        } else if let Some(busy) = &self.busy {
            html! { <span class="compare-note">{format!("reading {busy}…")}</span> }
        } else if self.compare.is_full() {
            html! {
                <span class="compare-note" title="Remove a waveform to load another one">
                    {"comparison is full (7 waveforms)"}
                </span>
            }
        } else {
            html! {
                <label class="compare-add" title="Load another dump and draw its series under these rows">
                    <input class="file-native" type="file"
                        accept=".vcd,.fst,.ghw"
                        onchange={self.pick(ctx)} />
                    {"+ Compare…"}
                </label>
            }
        };

        html! {
            <div class="compare-toolbar" role="toolbar" aria-label="Compare waveforms">
                <span class="compare-title">{"Compare"}</span>
                {base}
                {for members}
                {add}
            </div>
        }
    }
}

impl CompareBar {
    /// The file picked for comparison: read it, parse it, hand it to the store.
    ///
    /// The read and the parse are one `spawn_local` task; every failure becomes a
    /// status line that names the file, and the toolbar goes back to its button.
    fn pick(&self, ctx: &Context<Self>) -> Callback<Event> {
        let store = self.store.clone();
        let link = ctx.link().clone();
        Callback::from(move |event: Event| {
            let input: web_sys::HtmlInputElement = event.target_unchecked_into();
            let Some(file) = input.files().and_then(|files| files.item(0)) else {
                return;
            };
            let name = file.name();
            let size = file.size() as u64;
            if size > MAX_COMPARE_BYTES {
                store.dispatch(Action::Status(Status::error(format!(
                    "{name}: {:.1} MB is too large to compare in the browser (limit {} MB) — compare a smaller dump, or open it as the base dump instead",
                    size as f64 / (1024.0 * 1024.0),
                    MAX_COMPARE_BYTES / (1024 * 1024)
                ))));
                return;
            }
            // the same file may be picked again right after a removal
            input.set_value("");
            link.send_message(Msg::Busy(Some(name.clone())));
            let store_task = store.clone();
            let link_task = link.clone();
            spawn_local(async move {
                let buffer = match JsFuture::from(file.array_buffer()).await {
                    Ok(buffer) => buffer,
                    Err(_) => {
                        link_task.send_message(Msg::Busy(None));
                        store_task.dispatch(Action::Status(Status::error(format!(
                            "{name}: the file could not be read"
                        ))));
                        return;
                    }
                };
                let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
                let waveform = match Waveform::open_bytes(bytes) {
                    Ok(waveform) => waveform,
                    Err(error) => {
                        link_task.send_message(Msg::Busy(None));
                        store_task.dispatch(Action::Status(Status::error(compare::open_error(
                            &name,
                            &error.message(),
                        ))));
                        return;
                    }
                };
                // the note is computed before the dispatch: the base handle is
                // the active tab's, and the member's timescale is already known
                let base_timescale = store_task
                    .select(select::dump_waveform)
                    .and_then(|handle| handle.waveform().timescale());
                let member_timescale = waveform.timescale();
                let handle = DumpHandle::new(waveform);
                let (time_range, _) = handle.time_range();
                let set = store_task.select(select::compare);
                let Ok(slot) = set.free_slot() else {
                    link_task.send_message(Msg::Busy(None));
                    store_task.dispatch(Action::Status(Status::error(
                        "at most 7 comparison waveforms",
                    )));
                    return;
                };
                store_task.dispatch(Action::CompareMemberAdded {
                    name: name.clone(),
                    bytes: Some(size),
                    handle,
                    time_range,
                });
                let mut text = compare::loaded_status(slot, &name, Some(size), time_range);
                if let (Some(member), Some(base)) = (member_timescale, base_timescale) {
                    if let Some(note) = compare::timescale_note(slot, member, base) {
                        text.push_str(" — ");
                        text.push_str(&note);
                    }
                }
                link_task.send_message(Msg::Busy(None));
                store_task.dispatch(Action::Status(Status::info(text)));
            });
        })
    }
}
