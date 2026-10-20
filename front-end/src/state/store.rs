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
}

/// The app's store.
///
/// Cheap to clone and compares by identity, so it can be a plain Yew property
/// (exactly like the [`crate::lsp::LspBridge`] handle).
#[derive(Clone)]
pub struct Store(Rc<Inner>);

impl Store {
    pub fn new() -> Self {
        Self(Rc::new(Inner {
            state: RefCell::new(State::default()),
            subscribers: RefCell::new(Vec::new()),
            queue: RefCell::new(VecDeque::new()),
            draining: Cell::new(false),
            version: Cell::new(0),
            next_id: Cell::new(0),
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
    pub fn subscribe<S>(&self, select: impl Fn(&State) -> S + 'static, callback: Callback<S>) -> Subscription
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
        self.0.subscribers.borrow_mut().push(Subscriber { id, notify });
        Subscription { inner: Rc::clone(&self.0), id }
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
        effects
    }

    fn notify(&self) {
        // clone the `Rc`s: a callback may (un)subscribe, which would otherwise
        // borrow `subscribers` while it is iterated
        let subscribers: Vec<Notify> = {
            let subscribers = self.0.subscribers.borrow();
            subscribers.iter().map(|subscriber| Rc::clone(&subscriber.notify)).collect()
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
        let _layout_sub = store.subscribe(select::layout, Callback::from(move |_layout| {
            layout_counter.set(layout_counter.get() + 1);
        }));
        let drawer_counter = StdRc::clone(&drawer_calls);
        let _drawer_sub = store.subscribe(select::drawer_open, Callback::from(move |_open: bool| {
            drawer_counter.set(drawer_counter.get() + 1);
        }));

        // a status change touches neither slice
        store.dispatch(Action::Status(crate::state::Status::info("loaded")));
        assert_eq!(layout_calls.get(), 0);
        assert_eq!(drawer_calls.get(), 0);

        // a drawer toggle changes both the layout and the drawer flag
        store.dispatch(Action::ToggleDrawer);
        assert_eq!(layout_calls.get(), 1);
        assert_eq!(drawer_calls.get(), 1);

        // a sidebar resize changes the layout but not the drawer
        store.dispatch(Action::ResizeSidebar { width: 300.0, max: 640.0 });
        assert_eq!(layout_calls.get(), 2);
        assert_eq!(drawer_calls.get(), 1);
    }

    #[test]
    fn dropping_the_subscription_stops_the_notifications() {
        let store = Store::new();
        let calls = StdRc::new(StdCell::new(0));
        let counter = StdRc::clone(&calls);
        let subscription = store.subscribe(select::drawer_open, Callback::from(move |_open: bool| {
            counter.set(counter.get() + 1);
        }));
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
        let _sub = store.subscribe(select::status, Callback::from(move |status: Option<crate::state::Status>| {
            counter.set(counter.get() + 1);
            if let Some(status) = status {
                // cascade once: "the drawer closing clears the status"
                if status.text == "close" {
                    dispatcher.dispatch(Action::ClearStatus);
                }
            }
        }));

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
}
