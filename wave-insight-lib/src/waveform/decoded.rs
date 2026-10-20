//! Cached, decoded signal histories - the per frame path for a canvas renderer.
//!
//! [`Waveform::changes_in_window`](super::Waveform::changes_in_window) and
//! [`Waveform::stream_changes`](super::Waveform::stream_changes) re-read the file
//! body on every call, which is fine for a one-off query but impossible to do
//! for every frame. [`DecodedSignal`] is the alternative: a signal is decoded
//! exactly once into an owned, sorted slice of changes, kept in the per signal
//! cache, and handed out as a cheap handle.
//!
//! Why a handle instead of `&[(Time, WaveValue)]`: the cache lives behind a
//! `Mutex` so that [`super::Waveform`] can stay `Send + Sync` and every query
//! can take `&self`. A reference into the cache would have to outlive the lock
//! guard, and [`super::Waveform::unload_signal`] could free the data from another
//! thread while the reference is alive. An `Arc` slice is the honest answer: it
//! is zero copy, the clone is an atomic increment, [`DecodedSignal`] derefs to
//! `&[Change]`, and a handle keeps working even if the signal is unloaded
//! concurrently (you keep the snapshot, the cache stops owning it).
//!
//! Cost per decoded change: `size_of::<Change>()`, which is 40 bytes on a 64 bit
//! target (8 byte time stamp + a 32 byte owned [`WaveValue`]), plus the heap
//! payload of bus values. 500k changes of a single bit signal are therefore
//! ~20 MB; see [`super::Waveform::decoded_changes`] for the trade-off and
//! [`super::Waveform::cache_stats`] to observe it.

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use wellen::Time;

use super::value::WaveValue;

/// A single value change: the time stamp and the value in effect from then on.
///
/// Time stamps are monotonically increasing; delta cycles can produce several
/// changes with the same time stamp.
pub type Change = (Time, WaveValue);

/// A cached, decoded history of one signal.
///
/// Obtained from [`super::Waveform::decoded_changes`]; cheap to clone and to
/// slice, and it derefs to `&[Change]` so it can be used like a plain slice.
#[derive(Clone)]
pub struct DecodedSignal {
    changes: Arc<[Change]>,
}

impl DecodedSignal {
    /// Wrap an already decoded history.
    pub(crate) fn new(changes: Arc<[Change]>) -> Self {
        Self { changes }
    }

    /// Build a handle from changes the caller already decoded.
    ///
    /// The backend hands out handles for signals it decoded itself
    /// ([`super::Waveform::decoded_changes`]); this lets a client wrap its own
    /// history (synthetic data, a cached export, a filtered view) in the same
    /// type, so the drawing code does not need a second representation. Times
    /// must be sorted; a debug assertion checks it.
    pub fn from_changes(changes: impl Into<Arc<[Change]>>) -> Self {
        let changes = changes.into();
        debug_assert!(
            changes.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "a decoded history must be sorted by time"
        );
        Self { changes }
    }

    /// The whole history, in chronological order.
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// Number of value changes.
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// `true` if the signal never changes.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// All changes, in chronological order.
    pub fn iter(&self) -> impl Iterator<Item = &Change> + '_ {
        self.changes.iter()
    }

    /// Time of the first and the last change.
    pub fn span(&self) -> Option<(Time, Time)> {
        let first = self.changes.first()?.0;
        let last = self.changes.last()?.0;
        Some((first, last))
    }

    /// The first change, `None` if the signal never changes.
    pub fn first_change(&self) -> Option<&Change> {
        self.changes.first()
    }

    /// The last change, `None` if the signal never changes.
    pub fn last_change(&self) -> Option<&Change> {
        self.changes.last()
    }

    /// The change in effect at `time`: the last change at or before it.
    ///
    /// This is the value a renderer uses to seed the left edge of a window;
    /// it may be a change *before* the window (that is the point).
    pub fn change_at(&self, time: Time) -> Option<&Change> {
        change_at(&self.changes, time)
    }

    /// The value in effect at `time`, same semantics as [`Self::change_at`].
    pub fn value_at(&self, time: Time) -> Option<&WaveValue> {
        value_at(&self.changes, time)
    }

    /// The changes inside `start ..= end`, as a borrowed sub slice.
    ///
    /// Both bounds are inclusive, so a change exactly at `start` or `end` is
    /// included. `end == None` means "until the end of the history".
    ///
    /// Combine with [`Self::value_at`] to draw a window: seed the left edge with
    /// the value in effect at `start`, then walk this slice for the transitions.
    pub fn window(&self, start: Time, end: Option<Time>) -> &[Change] {
        window(&self.changes, start, end)
    }

    /// The changes after `time` (strictly greater), as a borrowed sub slice.
    pub fn changes_after(&self, time: Time) -> &[Change] {
        window(&self.changes, time.saturating_add(1), None)
    }

    /// `true` if both handles share the same decoded allocation.
    ///
    /// Used to check that the cache hands out the same decode instead of
    /// rebuilding it; two handles that were created by separate decodes are not
    /// shared even if their contents are equal.
    pub fn shares_storage_with(&self, other: &DecodedSignal) -> bool {
        Arc::ptr_eq(&self.changes, &other.changes)
    }

    /// Number of bytes the slice itself occupies (excluding value payloads).
    pub fn size_in_memory(&self) -> usize {
        self.changes.len() * std::mem::size_of::<Change>()
    }
}

impl Deref for DecodedSignal {
    type Target = [Change];

    fn deref(&self) -> &Self::Target {
        &self.changes
    }
}

impl fmt::Debug for DecodedSignal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedSignal")
            .field("changes", &self.changes.len())
            .field("span", &self.span())
            .finish()
    }
}

/// Index of the first change with `time >= needle`.
fn lower_bound(changes: &[Change], needle: Time) -> usize {
    changes.partition_point(|(time, _)| *time < needle)
}

/// Index of the first change with `time > needle`.
fn upper_bound(changes: &[Change], needle: Time) -> usize {
    changes.partition_point(|(time, _)| *time <= needle)
}

pub(crate) fn change_at(changes: &[Change], time: Time) -> Option<&Change> {
    let position = upper_bound(changes, time);
    if position == 0 {
        None
    } else {
        //delta cycles can produce several changes with the same time stamp;
        //the last one wins
        changes.get(position - 1)
    }
}

pub(crate) fn value_at(changes: &[Change], time: Time) -> Option<&WaveValue> {
    change_at(changes, time).map(|(_, value)| value)
}

pub(crate) fn window(changes: &[Change], start: Time, end: Option<Time>) -> &[Change] {
    let from = lower_bound(changes, start);
    let to = match end {
        None => changes.len(),
        Some(end) => upper_bound(changes, end),
    };
    if from >= to {
        &[]
    } else {
        &changes[from..to]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::waveform::Bits;

    fn bit(value: u8) -> WaveValue {
        WaveValue::Bits(Bits::new_msb_first(vec![if value == 0 {
            crate::waveform::BitValue::Zero
        } else {
            crate::waveform::BitValue::One
        }]))
    }

    fn history() -> DecodedSignal {
        let changes: Arc<[Change]> = vec![
            (0, bit(0)),
            (10, bit(1)),
            (20, bit(0)),
            //delta cycle: two changes with the same time stamp
            (30, bit(1)),
            (30, bit(0)),
        ]
        .into();
        DecodedSignal::new(changes)
    }

    #[test]
    fn span_and_bounds() {
        let decoded = history();
        assert_eq!(decoded.len(), 5);
        assert!(!decoded.is_empty());
        assert_eq!(decoded.span(), Some((0, 30)));
        assert_eq!(decoded.first_change().unwrap().0, 0);
        assert_eq!(decoded.last_change().unwrap().0, 30);
        //deref makes it usable as a slice
        assert_eq!(decoded[1].0, 10);
    }

    #[test]
    fn value_at_picks_the_last_change_at_or_before() {
        let decoded = history();
        assert!(decoded.value_at(0).is_some());
        //before the first change there is no value
        assert!(DecodedSignal::new(vec![(10, bit(1))].into()).value_at(9).is_none());
        assert_eq!(decoded.value_at(9).unwrap(), &bit(0));
        assert_eq!(decoded.value_at(10).unwrap(), &bit(1));
        assert_eq!(decoded.value_at(19).unwrap(), &bit(1));
        assert_eq!(decoded.value_at(30).unwrap(), &bit(0));
        assert_eq!(decoded.value_at(1_000).unwrap(), &bit(0));
    }

    #[test]
    fn window_bounds_are_inclusive() {
        let decoded = history();
        let times = |slice: &[Change]| slice.iter().map(|(time, _)| *time).collect::<Vec<_>>();

        assert_eq!(times(decoded.window(0, None)), vec![0, 10, 20, 30, 30]);
        assert_eq!(times(decoded.window(10, Some(20))), vec![10, 20]);
        assert_eq!(times(decoded.window(11, Some(19))), Vec::<u64>::new());
        assert_eq!(times(decoded.window(30, Some(30))), vec![30, 30]);
        assert_eq!(times(decoded.window(0, Some(0))), vec![0]);
        assert_eq!(times(decoded.window(1_000, None)), Vec::<u64>::new());
        assert_eq!(times(decoded.window(21, None)), vec![30, 30]);
        //an empty history never yields anything
        let empty = DecodedSignal::new(Vec::new().into());
        assert!(empty.window(0, None).is_empty());
        assert!(empty.value_at(0).is_none());
        assert_eq!(empty.span(), None);
    }

    #[test]
    fn changes_after_is_strict() {
        let decoded = history();
        let times = |slice: &[Change]| slice.iter().map(|(time, _)| *time).collect::<Vec<_>>();
        assert_eq!(times(decoded.changes_after(10)), vec![20, 30, 30]);
        assert_eq!(times(decoded.changes_after(29)), vec![30, 30]);
        assert_eq!(times(decoded.changes_after(30)), Vec::<u64>::new());
    }

    #[test]
    fn handles_share_storage_only_after_a_clone() {
        let decoded = history();
        let clone = decoded.clone();
        assert!(decoded.shares_storage_with(&clone));
        assert!(!decoded.shares_storage_with(&history()));
        assert_eq!(decoded.size_in_memory(), 5 * std::mem::size_of::<Change>());
    }
}
