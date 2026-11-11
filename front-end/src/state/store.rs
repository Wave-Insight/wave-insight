//! The store itself: one `Rc`-shared state, a dispatch queue and slice
//! subscriptions.
//!
//! The rules that make it predictable (design doc §2.6):
//!
//! * `dispatch` queues, then drains the queue in one loop, so a callback that
//!   dispatches again is handled in the same pass instead of recursing;
//! * the state borrow is dropped **before** subscribers are notified; a
//!   notification may `select` (immutable borrow) but a `dispatch` from inside
//!   it is queued, never applied re-entrantly, so it cannot panic the `RefCell`;
//! * a subscriber is called only when its selected value changed, compared with
//!   `PartialEq`. Slice values are cheap (`Copy` structs, or a versioned `Rc`
//!   slice in the later steps), so the comparison costs nothing;
//! * a [`Subscription`] removes itself on drop; components keep it in a field
//!   and let `destroy` drop it.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use yew::Callback;

use super::action::{Action, Effect};
use super::persist::Persister;
use super::reduce::reduce;
use super::state::State;

type Notify = Rc<dyn Fn(&State)>;

struct Subscriber {
    id: u64,
    notify: Notify,
}

struct Inner {
    state: RefCell<State>,
    subscribers: RefCell<Vec<Subscriber>>,
    queue: RefCell<VecDeque<Action>>,
    /// `true` while the drain loop is running, i.e. a nested dispatch must only
    /// enqueue.
    draining: Cell<bool>,
    version: Cell<u64>,
    next_id: Cell<u64>,
    /// `None` when the browser denies storage (or on the host target): the
    /// store then behaves exactly like a plain in-memory one.
    persister: Option<Rc<Persister>>,
}

/// The app's store.
///
/// Cheap to clone and compares by identity, so it can be a plain Yew property
/// (exactly like the [`crate::lsp::LspBridge`] handle).
#[derive(Clone)]
pub struct Store(Rc<Inner>);

impl Store {
    pub fn new() -> Self {
        Self::with_persister(None)
    }

    /// The store, with the layout, the per-signal settings and the remembered
    /// row lists restored from the last session and written back when they
    /// change.
    ///
    /// `persister` is `None` when the browser has no usable `localStorage`
    /// (private mode) — the app then starts with its defaults, exactly like
    /// [`Store::new`], and nothing panics.
    pub fn with_persister(persister: Option<Rc<Persister>>) -> Self {
        let mut state = State::default();
        if let Some(persister) = &persister {
            state.layout = persister.restored_layout().clamped();
            state.row_prefs = persister.restored_rows();
            state.sessions = persister.restored_sessions();
        }
        Self(Rc::new(Inner {
            state: RefCell::new(state),
            subscribers: RefCell::new(Vec::new()),
            queue: RefCell::new(VecDeque::new()),
            draining: Cell::new(false),
            version: Cell::new(0),
            next_id: Cell::new(0),
            persister,
        }))
    }

    /// Read one value without subscribing. Cheap; use it in `create` and in
    /// event handlers, not in `view` unless the component already subscribes.
    pub fn select<S>(&self, select: impl Fn(&State) -> S) -> S {
        let state = self.0.state.borrow();
        select(&state)
    }

    /// Number of applied actions so far; useful for asserts and debugging.
    pub fn version(&self) -> u64 {
        self.0.version.get()
    }

    /// Notify `callback` whenever `select` returns something different.
    pub fn subscribe<S>(
        &self,
        select: impl Fn(&State) -> S + 'static,
        callback: Callback<S>,
    ) -> Subscription
    where
        S: PartialEq + Clone + 'static,
    {
        let select = Rc::new(select);
        let last = RefCell::new({
            let state = self.0.state.borrow();
            select(&state)
        });
        let id = self.0.next_id.get() + 1;
        self.0.next_id.set(id);

        let select_for_notify = Rc::clone(&select);
        let notify: Notify = Rc::new(move |state: &State| {
            let next = select_for_notify(state);
            let mut last = last.borrow_mut();
            if *last != next {
                *last = next.clone();
                // the borrow of `last` ends here; the callback may dispatch, and
                // that dispatch is queued by the running drain loop
                drop(last);
                callback.emit(next);
            }
        });
        self.0
            .subscribers
            .borrow_mut()
            .push(Subscriber { id, notify });
        Subscription {
            inner: Rc::clone(&self.0),
            id,
        }
    }

    /// Apply one action (and anything it cascades) and return the effects the
    /// caller has to execute.
    pub fn dispatch(&self, action: Action) -> Vec<Effect> {
        self.0.queue.borrow_mut().push_back(action);
        if self.0.draining.replace(true) {
            // a nested dispatch (from a subscriber): the running loop picks it up
            return Vec::new();
        }

        let mut effects = Vec::new();
        loop {
            let next = self.0.queue.borrow_mut().pop_front();
            let Some(action) = next else { break };
            {
                // the borrow ends here, before any subscriber runs
                let mut state = self.0.state.borrow_mut();
                effects.extend(reduce(&mut state, action));
                self.0.version.set(self.0.version.get().wrapping_add(1));
            }
            self.notify();
        }
        self.0.draining.set(false);
        // Persist the committed values once the queue has drained, never during
        // the gesture: the persister writes only the slices that changed since
        // the last write, so a drag committed on mouseup writes once and a
        // selection change writes nothing.
        if let Some(persister) = &self.0.persister {
            let state = self.0.state.borrow();
            persister.sync(&state);
        }
        effects
    }

    fn notify(&self) {
        // clone the `Rc`s: a callback may (un)subscribe, which would otherwise
        // borrow `subscribers` while it is iterated
        let subscribers: Vec<Notify> = {
            let subscribers = self.0.subscribers.borrow();
            subscribers
                .iter()
                .map(|subscriber| Rc::clone(&subscriber.notify))
                .collect()
        };
        let state = self.0.state.borrow();
        for notify in subscribers {
            notify(&state);
        }
    }
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Store {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Store")
            .field("version", &self.version())
            .field("subscribers", &self.0.subscribers.borrow().len())
            .finish()
    }
}

/// Handle to one subscription; dropping it unsubscribes.
///
/// Components keep it in a field so it lives exactly as long as the component.
pub struct Subscription {
    inner: Rc<Inner>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut subscribers = self.inner.subscribers.borrow_mut();
        subscribers.retain(|subscriber| subscriber.id != self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{select, Action};
    use std::cell::Cell as StdCell;
    use std::rc::Rc as StdRc;

    #[test]
    fn subscribers_only_fire_when_their_slice_changes() {
        let store = Store::new();
        let layout_calls = StdRc::new(StdCell::new(0));
        let drawer_calls = StdRc::new(StdCell::new(0));

        let layout_counter = StdRc::clone(&layout_calls);
        let _layout_sub = store.subscribe(
            select::layout,
            Callback::from(move |_layout| {
                layout_counter.set(layout_counter.get() + 1);
            }),
        );
        let drawer_counter = StdRc::clone(&drawer_calls);
        let _drawer_sub = store.subscribe(
            select::drawer_open,
            Callback::from(move |_open: bool| {
                drawer_counter.set(drawer_counter.get() + 1);
            }),
        );

        // a status change touches neither slice
        store.dispatch(Action::Status(crate::state::Status::info("loaded")));
        assert_eq!(layout_calls.get(), 0);
        assert_eq!(drawer_calls.get(), 0);

        // a drawer toggle changes both the layout and the drawer flag
        store.dispatch(Action::ToggleDrawer);
        assert_eq!(layout_calls.get(), 1);
        assert_eq!(drawer_calls.get(), 1);

        // a sidebar resize changes the layout but not the drawer
        store.dispatch(Action::ResizeSidebar {
            width: 300.0,
            max: 640.0,
        });
        assert_eq!(layout_calls.get(), 2);
        assert_eq!(drawer_calls.get(), 1);
    }

    #[test]
    fn dropping_the_subscription_stops_the_notifications() {
        let store = Store::new();
        let calls = StdRc::new(StdCell::new(0));
        let counter = StdRc::clone(&calls);
        let subscription = store.subscribe(
            select::drawer_open,
            Callback::from(move |_open: bool| {
                counter.set(counter.get() + 1);
            }),
        );
        store.dispatch(Action::ToggleDrawer);
        assert_eq!(calls.get(), 1);
        drop(subscription);
        store.dispatch(Action::ToggleDrawer);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn a_dispatch_from_a_subscriber_is_applied_in_the_same_pass() {
        let store = Store::new();
        let statuses = StdRc::new(StdCell::new(0));
        let counter = StdRc::clone(&statuses);
        let dispatcher = store.clone();
        let _sub = store.subscribe(
            select::status,
            Callback::from(move |status: Option<crate::state::Status>| {
                counter.set(counter.get() + 1);
                if let Some(status) = status {
                    // cascade once: "the drawer closing clears the status"
                    if status.text == "close" {
                        dispatcher.dispatch(Action::ClearStatus);
                    }
                }
            }),
        );

        store.dispatch(Action::Status(crate::state::Status::info("close")));
        // notified for the set, then for the cascade
        assert_eq!(statuses.get(), 2);
        assert_eq!(store.select(select::status), None);
    }

    #[test]
    fn version_counts_applied_actions() {
        let store = Store::new();
        assert_eq!(store.version(), 0);
        store.dispatch(Action::ToggleDrawer);
        store.dispatch(Action::ClearStatus);
        assert_eq!(store.version(), 2);
    }

    #[test]
    fn the_store_is_its_own_identity() {
        let a = Store::new();
        let b = a.clone();
        let c = Store::new();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn a_persisted_store_remembers_the_rows_of_the_open_dump() {
        use crate::state::persist::{MemoryStorage, Persister, SESSIONS_KEY};
        use crate::state::{DumpHandle, DumpId};

        let bytes = include_bytes!("../../../qa/fixtures/wave.vcd").to_vec();

        fn open(store: &Store, bytes: &[u8], size: u64) {
            let waveform = wave_insight_lib::waveform::Waveform::open_bytes(bytes.to_vec())
                .expect("the vcd fixture opens");
            store.dispatch(Action::DumpAdded {
                id: DumpId::new("wave.vcd", Some(size)),
                handle: DumpHandle::new(waveform),
                fingerprint: None,
            });
        }

        let storage = MemoryStorage::new();
        {
            let persister = StdRc::new(Persister::load(Box::new(storage.clone())));
            let store = Store::with_persister(Some(persister));
            open(&store, &bytes, bytes.len() as u64);
            for path in ["top.pc", "top.clk"] {
                store.dispatch(Action::RowAdded {
                    path: path.to_string(),
                    name: path.to_string(),
                    width: 8,
                    load: Vec::new(),
                    drive: Vec::new(),
                    bit: None,
                });
            }
        }
        let stored = storage
            .raw(SESSIONS_KEY)
            .expect("the sessions key is written");
        assert!(
            stored.contains("wave.vcd") && stored.contains("top.pc"),
            "{stored}"
        );

        // a reload: opening the same dump puts the rows back, in order
        let persister = StdRc::new(Persister::load(Box::new(storage.clone())));
        let store = Store::with_persister(Some(persister));
        open(&store, &bytes, bytes.len() as u64);
        let names = |store: &Store| -> Vec<String> {
            store.select(|state| {
                state
                    .rows_items()
                    .iter()
                    .map(|row| row.path.clone())
                    .collect::<Vec<String>>()
            })
        };
        assert_eq!(names(&store), vec!["top.pc", "top.clk"]);

        // a file with the same name but another size is a different dump: it gets
        // its own tab (with no remembered rows), and switching back keeps the
        // first tab's rows
        open(&store, &bytes, bytes.len() as u64 + 1);
        assert_eq!(store.select(|state| state.dumps.len()), 2);
        assert!(names(&store).is_empty());
        store.dispatch(Action::DumpTabActivated { index: 0 });
        assert_eq!(names(&store), vec!["top.pc", "top.clk"]);
    }

    #[test]
    fn a_persisted_store_restores_the_session_and_writes_committed_changes() {
        use crate::state::persist::{MemoryStorage, Persister};
        use crate::state::types::Layout;
        use wave_insight_lib::waveform::ShowType;

        let storage = MemoryStorage::new();
        // a first session: commit a layout and configure one signal
        let mut setting = crate::state::Settings::new();
        setting.show_type = ShowType::Bin;
        setting.analog_active = true;
        {
            let persister = StdRc::new(Persister::load(Box::new(storage.clone())));
            let store = Store::with_persister(Some(persister));
            store.dispatch(Action::ResizeSidebar {
                width: 360.0,
                max: 640.0,
            });
            store.dispatch(Action::ToggleDrawer);
            store.dispatch(Action::RowAdded {
                path: "top.pc".into(),
                name: "top.pc".into(),
                width: 8,
                load: Vec::new(),
                drive: Vec::new(),
                bit: None,
            });
            store.dispatch(Action::RowSettingChanged {
                indices: vec![0],
                setting: setting.clone(),
            });
        }

        // a reload: the layout is back and the row setting is remembered
        let writes_before_reload = storage.writes();
        let persister = StdRc::new(Persister::load(Box::new(storage.clone())));
        let store = Store::with_persister(Some(persister));
        let layout = store.select(select::layout);
        assert_eq!(
            layout,
            Layout {
                sidebar_w: 360.0,
                editor_ratio: Layout::default().editor_ratio,
                drawer_open: false,
                split_ratio: Layout::default().split_ratio,
            }
        );
        // reading the stored values back is not a change: nothing is rewritten
        assert_eq!(storage.writes(), writes_before_reload);
        // an unrelated action writes nothing either (the throttle)
        store.dispatch(Action::Status(crate::state::Status::info("loaded")));
        assert_eq!(storage.writes(), writes_before_reload);

        store.dispatch(Action::RowAdded {
            path: "top.pc".into(),
            name: "top.pc".into(),
            width: 8,
            load: Vec::new(),
            drive: Vec::new(),
            bit: None,
        });
        let restored = store.select(|state| state.rows.items[0].setting.clone());
        assert_eq!(restored, setting);

        // a new commit is written through, and a second reload sees it
        store.dispatch(Action::ResizeSidebar {
            width: 420.0,
            max: 640.0,
        });
        let reloaded = Persister::load(Box::new(storage));
        assert_eq!(reloaded.restored_layout().sidebar_w, 420.0);
        assert_eq!(reloaded.restored_rows().setting_for("top.pc"), setting);
    }

    #[test]
    fn a_source_action_returns_the_lsp_effects_the_app_executes() {
        let store = Store::new();
        let effects = store.dispatch(Action::SourceOpened {
            file: "top.sv".into(),
            text: "// top\n".into(),
        });
        // the store owns the document; the bridge mirror is updated by the effect
        assert_eq!(
            effects,
            vec![
                crate::state::Effect::LspOpen {
                    file: "top.sv".into(),
                    text: "// top\n".into()
                },
                crate::state::Effect::LspTop { top: None },
            ]
        );
        assert_eq!(store.select(|state| state.sources.len()), 1);
        // closing it asks for the didClose, and the document is gone
        let effects = store.dispatch(Action::SourceClosed { index: 0 });
        assert_eq!(
            effects,
            vec![crate::state::Effect::LspClose {
                file: "top.sv".into()
            }]
        );
        assert_eq!(store.select(|state| state.sources.len()), 0);
    }
}
