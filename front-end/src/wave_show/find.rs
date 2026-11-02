//! Find the next/previous change whose value matches what the user typed.
//!
//! The time ruler's box accepts two kinds of input: an integer jumps the cursor
//! to that time (the old behaviour), anything else is a *value* and the box
//! becomes a Verdi-style "find value": it walks the selected row's decoded
//! history and moves cursor 1 to the next (or, with Shift+Enter, previous) change
//! whose formatted value matches. Formatting goes through the same ValueFormat
//! the value column and the canvas use, so what the user sees is what the search
//! compares against (fixed point and x/z included).
//!
//! The matcher is pure and unit tested here; find_value is the thin glue that
//! reads the store, decodes the row and dispatches the cursor move.

use wave_insight_lib::waveform::{Change, DecodedSignal};

use crate::nav::{value_text, ValueFormat};
use crate::state::{select, Action, Status, Store};
use crate::wave_show::util::signal_things::derive_bit;

/// Which way a search walks the history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Towards later times (Enter).
    Next,
    /// Towards earlier times (Shift+Enter).
    Previous,
}

/// One match: its time and where it is in the row's matching changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Found {
    /// Dump time of the matching change.
    pub time: i64,
    /// 1-based ordinal of the match among all matches of that value.
    pub ordinal: usize,
    /// How many changes of the row carry that value.
    pub total: usize,
}

/// The first match strictly after (or before) from, plus its ordinal.
///
/// from is the current cursor time; a match exactly at from is skipped, so
/// pressing Enter twice walks to the next occurrence instead of standing still.
/// The comparison is case-insensitive (the display uses a lower-case radix
/// prefix, but a user typing 'H2A means the same thing) and compares the
/// formatted text, exactly as the value column shows it.
pub fn find_match(
    changes: &[Change],
    format: &ValueFormat,
    width: usize,
    query: &str,
    from: i64,
    direction: Direction,
) -> Option<Found> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let mut matches: Vec<(i64, usize)> = Vec::new();
    for (time, value) in changes {
        if let Some(text) = value_text(value, width, format) {
            if text.to_lowercase() == needle {
                matches.push((*time as i64, matches.len()));
            }
        }
    }
    let total = matches.len();
    if total == 0 {
        return None;
    }
    let picked = match direction {
        Direction::Next => matches.iter().find(|(time, _)| *time > from),
        Direction::Previous => matches.iter().rev().find(|(time, _)| *time < from),
    }?;
    Some(Found {
        time: picked.0,
        ordinal: picked.1 + 1,
        total,
    })
}

/// Run a find from the ruler's box: read the selected row, decode it, match and
/// move cursor 1 (plus bring the match into the left quarter of the window).
///
/// Every failure is an actionable status line, never a panic: nothing selected,
/// no dump open, a row that is not in the dump, a decode error.
pub fn find_value(store: &Store, query: &str, direction: Direction, width: f64) {
    let query = query.trim();
    if query.is_empty() {
        return;
    }
    let rows = store.select(select::rows);
    let Some(row) = rows.items.iter().find(|row| row.chosen).cloned() else {
        store.dispatch(Action::Status(Status::error(
            "click a wave row first, then type a value to find it",
        )));
        return;
    };
    let Some(dump) = store.select(select::dump_waveform) else {
        store.dispatch(Action::Status(Status::error(
            "no dump open - load a waveform before searching values",
        )));
        return;
    };
    let waveform = dump.waveform();
    let Some(info) = waveform.signal_by_path(&row.path) else {
        store.dispatch(Action::Status(Status::error(format!(
            "{} is not in the open dump",
            row.name
        ))));
        return;
    };
    // A value search walks the whole history; on a dense trace that history can
    // be hundreds of MB. Bound it first and refuse with an actionable message
    // instead of materialising it: the same cap drawing uses, but for the whole
    // trace rather than a window.
    const SEARCH_CHANGE_LIMIT: usize = 1_000_000;
    match waveform.window_changes_many(&[info.index()], 0, None, SEARCH_CHANGE_LIMIT) {
        Ok(probe) => {
            if probe.first().is_some_and(|window| window.capped) {
                store.dispatch(Action::Status(Status::error(format!(
                    "{}: more than {} value changes — too dense to search in the browser; narrow the dump or use the desktop build",
                    row.name, SEARCH_CHANGE_LIMIT
                ))));
                return;
            }
        }
        Err(error) => {
            store.dispatch(Action::Status(Status::error(format!(
                "{}: no values to search ({})",
                row.name,
                error.message()
            ))));
            return;
        }
    }
    let decoded = match waveform.decoded_changes(info.index()) {
        Ok(decoded) => decoded,
        Err(error) => {
            store.dispatch(Action::Status(Status::error(format!(
                "{}: no values to search ({error})",
                row.name
            ))));
            return;
        }
    };
    let decoded: DecodedSignal = match row.bit {
        Some(bit) => derive_bit(&decoded, bit as usize),
        None => decoded,
    };
    let format = ValueFormat::from_setting(&row.setting);
    let viewport = store.select(select::viewport);
    let from = viewport.cursors.one.map(i64::from).unwrap_or(-1);
    let Some(found) = find_match(
        decoded.changes(),
        &format,
        row.width,
        query,
        from,
        direction,
    ) else {
        let when = match direction {
            Direction::Next => "after cursor 1",
            Direction::Previous => "before cursor 1",
        };
        store.dispatch(Action::Status(Status::error(format!(
            "no match for \"{query}\" in {} {when}",
            row.name
        ))));
        return;
    };
    let mut cursors = viewport.cursors;
    cursors.one = Some(found.time.min(i32::MAX as i64) as i32);
    store.dispatch(Action::CursorsChanged(cursors));
    // bring the match into the left quarter of the window, like the time goto
    let mut view = viewport.view;
    let span = view.time_span(width);
    view.x_axis = (found.time as f64 - span * 0.25).max(0.0);
    store.dispatch(Action::ViewCommitted(view));
    store.dispatch(Action::Status(Status::info(format!(
        "found {query} in {} at t={} ({}/{})",
        row.name, found.time, found.ordinal, found.total
    ))));
}

#[cfg(test)]
mod tests {
    use super::*;
    use wave_insight_lib::waveform::ShowType;
    use wave_insight_lib::waveform::{BitValue, Bits, WaveValue};

    fn bits(raw: u64, width: usize) -> WaveValue {
        WaveValue::Bits(Bits::new_msb_first(
            (0..width)
                .rev()
                .map(|bit| {
                    if raw & (1u64 << bit) != 0 {
                        BitValue::One
                    } else {
                        BitValue::Zero
                    }
                })
                .collect(),
        ))
    }

    fn hex_changes() -> Vec<Change> {
        vec![
            (0u64, bits(0, 8)),
            (10, bits(0x01, 8)),
            (20, bits(0x2a, 8)),
            (30, bits(0x01, 8)),
            (40, bits(0x2a, 8)),
        ]
    }

    fn format() -> ValueFormat {
        ValueFormat::base(ShowType::Hex)
    }

    #[test]
    fn the_next_match_is_strictly_after_the_cursor() {
        let changes = hex_changes();
        let found = find_match(&changes, &format(), 8, "'h01", 0, Direction::Next).expect("match");
        assert_eq!(
            found,
            Found {
                time: 10,
                ordinal: 1,
                total: 2
            }
        );
        let found = find_match(&changes, &format(), 8, "'h01", 10, Direction::Next).expect("match");
        assert_eq!(
            found,
            Found {
                time: 30,
                ordinal: 2,
                total: 2
            }
        );
    }

    #[test]
    fn the_previous_match_is_strictly_before_the_cursor() {
        let changes = hex_changes();
        let found =
            find_match(&changes, &format(), 8, "'h2a", 40, Direction::Previous).expect("match");
        assert_eq!(
            found,
            Found {
                time: 20,
                ordinal: 1,
                total: 2
            }
        );
        assert!(find_match(&changes, &format(), 8, "'h2a", 20, Direction::Previous).is_none());
    }

    #[test]
    fn matching_is_case_insensitive_and_trimmed() {
        let changes = hex_changes();
        let found =
            find_match(&changes, &format(), 8, "  'H2A \t", 0, Direction::Next).expect("match");
        assert_eq!(found.time, 20);
        assert!(find_match(&changes, &format(), 8, "   ", 0, Direction::Next).is_none());
    }

    #[test]
    fn unknown_values_are_reported_as_no_match() {
        assert!(find_match(&hex_changes(), &format(), 8, "'hff", 0, Direction::Next).is_none());
    }

    #[test]
    fn unknown_bits_match_the_x_text_the_value_column_shows() {
        let changes: Vec<Change> = vec![
            (
                0u64,
                WaveValue::Bits(Bits::new_msb_first(vec![BitValue::X])),
            ),
            (5, bits(0, 1)),
        ];
        let mut one_bit = format();
        one_bit.show = ShowType::Bin;
        // no cursor yet is -1 (the same default find_value uses), so t=0 matches
        let found = find_match(&changes, &one_bit, 1, "x", -1, Direction::Next);
        // x is rendered as the bare x of the value column for a 1 bit row
        assert_eq!(found.map(|found| found.time), Some(0));
    }
}
