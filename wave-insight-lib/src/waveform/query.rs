//! Describes "give me these value changes" without materialising the trace.
//!
//! A query is always bounded: by signals and (optionally) by a time window and
//! a change count. That is the whole point of the lazy path - the UI asks for
//! the time range that is currently on screen for the signals that are
//! currently shown, instead of the whole dump.

use wellen::Time;

/// Which value changes to report.
#[derive(Clone, Debug)]
pub struct ValueQuery {
    /// Variables to report, `None` means every variable of the file.
    ///
    /// Indices are [`super::SignalInfo::index`] values.
    pub signals: Option<Vec<usize>>,
    /// First time stamp to report (inclusive).
    pub start: Time,
    /// Last time stamp to report (inclusive), `None` means until the end.
    pub end: Option<Time>,
    /// Stop after this many changes, `None` means no limit.
    pub max_changes: Option<usize>,
}

impl ValueQuery {
    /// Every change of every variable.
    pub fn all() -> Self {
        Self {
            signals: None,
            start: 0,
            end: None,
            max_changes: None,
        }
    }

    /// Every change of one variable.
    pub fn for_signal(signal: usize) -> Self {
        Self {
            signals: Some(vec![signal]),
            ..Self::all()
        }
    }

    /// Every change of the given variables.
    pub fn for_signals(signals: impl IntoIterator<Item = usize>) -> Self {
        Self {
            signals: Some(signals.into_iter().collect()),
            ..Self::all()
        }
    }

    /// One variable, one time window.
    pub fn window(signal: usize, start: Time, end: Option<Time>) -> Self {
        Self::for_signal(signal).between(start, end)
    }

    /// Restrict to `start ..= end`.
    pub fn between(mut self, start: Time, end: Option<Time>) -> Self {
        self.start = start;
        self.end = end;
        self
    }

    /// Stop after `max_changes` reported changes.
    pub fn with_max_changes(mut self, max_changes: usize) -> Self {
        self.max_changes = Some(max_changes);
        self
    }

    /// `true` if the query would report `signal`.
    pub fn includes_signal(&self, signal: usize) -> bool {
        self.signals
            .as_ref()
            .map(|signals| signals.contains(&signal))
            .unwrap_or(true)
    }

    /// `true` if `time` is inside the requested window.
    pub fn includes_time(&self, time: Time) -> bool {
        time >= self.start && self.end.map(|end| time <= end).unwrap_or(true)
    }
}

impl Default for ValueQuery {
    fn default() -> Self {
        Self::all()
    }
}
