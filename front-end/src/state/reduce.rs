//! The reducer: `(State, Action) -> Vec<Effect>`.
//!
//! Pure on purpose — no `web_sys`, no waveform handle, no LSP — so it can be
//! unit tested on the host target (`cargo test -p front-end --bins`) and so a
//! change is always "this fact changes these fields". Anything that needs the
//! outside world is returned as an [`Effect`] for the app to execute.
//!
//! Row semantics mirror the old `SignalThings` exactly (dedupe by path, insert
//! after the last chosen row, index-shifting removal, the three selection
//! modes) so moving them into the store does not move behaviour.

use super::action::{Action, Effect};
use super::state::State;
use super::types::{clamp_ratio, clamp_sidebar, Dump, Row};

pub fn reduce(state: &mut State, action: Action) -> Vec<Effect> {
    match action {
        Action::Layout(layout) => {
            state.layout = layout.clamped();
        }
        Action::ResizeSidebar { width, max } => {
            state.layout.sidebar_w = clamp_sidebar(width, max);
        }
        Action::ToggleDrawer => {
            state.layout.drawer_open = !state.layout.drawer_open;
        }
        Action::SetEditorRatio(ratio) => {
            state.layout.editor_ratio = clamp_ratio(ratio);
        }
        Action::Status(status) => {
            state.nav.status = Some(status);
        }
        Action::ClearStatus => {
            state.nav.status = None;
        }
        Action::RowAdded { path, name, width, load, drive } => {
            if !path.is_empty() {
                let mut items = (*state.rows.items).clone();
                if !items.iter().any(|row| row.path == path) {
                    // insert after the last chosen row, like the old push()
                    let insert_at = items
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.chosen)
                        .map(|(index, _)| index)
                        .last()
                        .map(|index| index + 1)
                        .unwrap_or(items.len());
                    items.insert(
                        insert_at,
                        Row {
                            path,
                            name,
                            width: width.max(1),
                            setting: State::default_row_setting(),
                            chosen: false,
                            load,
                            drive,
                        },
                    );
                    state.set_rows(items);
                }
            }
        }
        Action::RowsRemoved { indices } => {
            if !indices.is_empty() {
                let mut sorted = indices;
                sorted.sort_unstable();
                sorted.dedup();
                let mut items = (*state.rows.items).clone();
                // remove from the back so the earlier indices stay valid
                for index in sorted.into_iter().rev() {
                    if index < items.len() {
                        items.remove(index);
                    }
                }
                state.set_rows(items);
            }
        }
        Action::RowSelected { index, ctrl, shift } => {
            let mut items = (*state.rows.items).clone();
            if index < items.len() && choose(&mut items, index, ctrl, shift) {
                state.set_rows(items);
            }
        }
        Action::RowsAllSelected => {
            let mut items = (*state.rows.items).clone();
            if items.iter().any(|row| !row.chosen) {
                for row in items.iter_mut() {
                    row.chosen = true;
                }
                state.set_rows(items);
            }
        }
        Action::RowsCleared => {
            let mut items = (*state.rows.items).clone();
            if items.iter().any(|row| row.chosen) {
                for row in items.iter_mut() {
                    row.chosen = false;
                }
                state.set_rows(items);
            }
        }
        Action::RowsReordered { from, to } => {
            let mut items = (*state.rows.items).clone();
            if from < items.len() && to < items.len() && from != to {
                let row = items.remove(from);
                items.insert(to, row);
                state.set_rows(items);
            }
        }
        Action::DumpOpened { waveform } => {
            // the range is derived from the handle, never hand-written into the
            // store: the reducer is the one place that may compute it
            let time_range = waveform.waveform().time_range_hint().ok().flatten();
            state.dump = Dump { waveform: Some(waveform), time_range };
        }
        Action::ViewCommitted(view) => {
            state.viewport.view = view;
        }
        Action::CursorsChanged(cursors) => {
            state.viewport.cursors = cursors;
        }
        Action::RowSettingChanged { indices, setting } => {
            let mut items = (*state.rows.items).clone();
            let mut changed = false;
            for index in indices {
                if let Some(row) = items.get_mut(index) {
                    row.setting = setting.clone();
                    changed = true;
                }
            }
            if changed {
                state.set_rows(items);
            }
        }
    }
    Vec::new()
}

/// The three selection modes of the wave rows, exactly as the view had them.
///
/// Returns `true` when the selection changed.
fn choose(items: &mut [Row], index: usize, ctrl: bool, shift: bool) -> bool {
    if index >= items.len() {
        return false;
    }
    if ctrl {
        items[index].chosen = !items[index].chosen;
        return true;
    }
    let chosen: Vec<usize> = items
        .iter()
        .enumerate()
        .filter(|(_, row)| row.chosen)
        .map(|(index, _)| index)
        .collect();
    if shift && !chosen.is_empty() {
        let head = chosen[0];
        let tail = *chosen.last().unwrap_or(&head);
        for row in items.iter_mut() {
            row.chosen = false;
        }
        if chosen.contains(&index) {
            // clicking inside the selection clears it
            return true;
        }
        let (start, end) = if tail < index {
            (head, index)
        } else if index < head {
            (index, tail)
        } else if (head as isize - index as isize).abs() <= (index as isize - tail as isize).abs() {
            (head, index)
        } else {
            (index, tail)
        };
        if let Some(range) = items.get_mut(start..=end) {
            for row in range {
                row.chosen = true;
            }
        }
        return true;
    }
    // a plain click toggles the clicked row and clears everything else
    let was_chosen = items[index].chosen;
    for row in items.iter_mut() {
        row.chosen = false;
    }
    items[index].chosen = !was_chosen;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::types::{DumpHandle, Layout, Settings, Status, StatusTone, EDITOR_RATIO_MIN, SIDEBAR_MIN};

    fn state() -> State {
        State::default()
    }

    fn add(state: &mut State, path: &str) {
        reduce(
            state,
            Action::RowAdded {
                path: path.to_string(),
                name: path.to_string(),
                width: 8,
                load: vec![],
                drive: vec![],
            },
        );
    }

    fn paths(state: &State) -> Vec<String> {
        state.rows.items.iter().map(|row| row.path.clone()).collect()
    }

    #[test]
    fn resize_sidebar_clamps_into_the_window() {
        let mut state = state();
        reduce(&mut state, Action::ResizeSidebar { width: 340.0, max: 640.0 });
        assert_eq!(state.layout.sidebar_w, 340.0);
        reduce(&mut state, Action::ResizeSidebar { width: 5000.0, max: 640.0 });
        assert_eq!(state.layout.sidebar_w, 640.0);
        reduce(&mut state, Action::ResizeSidebar { width: 10.0, max: 640.0 });
        assert_eq!(state.layout.sidebar_w, SIDEBAR_MIN);
    }

    #[test]
    fn toggle_drawer_flips_the_flag() {
        let mut state = state();
        assert!(state.layout.drawer_open);
        reduce(&mut state, Action::ToggleDrawer);
        assert!(!state.layout.drawer_open);
        reduce(&mut state, Action::ToggleDrawer);
        assert!(state.layout.drawer_open);
    }

    #[test]
    fn editor_ratio_is_clamped_on_write() {
        let mut state = state();
        reduce(&mut state, Action::SetEditorRatio(0.9));
        assert_eq!(state.layout.editor_ratio, 0.75);
        reduce(&mut state, Action::SetEditorRatio(0.01));
        assert_eq!(state.layout.editor_ratio, EDITOR_RATIO_MIN);
    }

    #[test]
    fn replacing_the_layout_clamps_every_field() {
        let mut state = state();
        reduce(
            &mut state,
            Action::Layout(Layout { sidebar_w: 4000.0, editor_ratio: 0.02, drawer_open: false }),
        );
        assert_eq!(state.layout.sidebar_w, 4000.0_f64.max(SIDEBAR_MIN));
        assert_eq!(state.layout.editor_ratio, EDITOR_RATIO_MIN);
        assert!(!state.layout.drawer_open);
    }

    #[test]
    fn status_is_set_and_cleared() {
        let mut state = state();
        assert_eq!(state.nav.status, None);
        reduce(&mut state, Action::Status(Status::error("top.sv: not utf-8")));
        let status = state.nav.status.clone().expect("status");
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(status.text, "top.sv: not utf-8");
        reduce(&mut state, Action::ClearStatus);
        assert_eq!(state.nav.status, None);
    }

    #[test]
    fn step_one_produces_no_effects() {
        let mut state = state();
        assert!(reduce(&mut state, Action::ToggleDrawer).is_empty());
        assert!(reduce(&mut state, Action::Status(Status::info("ok"))).is_empty());
    }

    #[test]
    fn adding_a_row_appends_it_once() {
        let mut state = state();
        add(&mut state, "top.clk");
        add(&mut state, "top.pc");
        // the same signal can be added from several places; it is deduped by path
        add(&mut state, "top.clk");
        assert_eq!(paths(&state), vec!["top.clk", "top.pc"]);
    }

    #[test]
    fn adding_a_row_puts_it_after_the_last_chosen_one() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        reduce(&mut state, Action::RowSelected { index: 0, ctrl: false, shift: false });
        add(&mut state, "c");
        assert_eq!(paths(&state), vec!["a", "c", "b"]);
    }

    #[test]
    fn removing_rows_shifts_the_rest_and_ignores_bad_indices() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        add(&mut state, "c");
        reduce(&mut state, Action::RowsRemoved { indices: vec![2, 0, 9] });
        assert_eq!(paths(&state), vec!["b"]);
    }

    #[test]
    fn reordering_moves_a_row_and_keeps_the_others() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        add(&mut state, "c");
        reduce(&mut state, Action::RowsReordered { from: 2, to: 0 });
        assert_eq!(paths(&state), vec!["c", "a", "b"]);
        // out of range is a no-op
        reduce(&mut state, Action::RowsReordered { from: 9, to: 0 });
        assert_eq!(paths(&state), vec!["c", "a", "b"]);
    }

    #[test]
    fn rows_are_versioned_so_unchanged_actions_do_not_bump() {
        let mut state = state();
        add(&mut state, "a");
        let version = state.rows.version;
        // a duplicate add changes nothing, so the version must not move
        add(&mut state, "a");
        assert_eq!(state.rows.version, version);
        // and a real change makes the slice compare different
        let before = state.rows.slice();
        add(&mut state, "b");
        assert_ne!(before, state.rows.slice());
    }

    #[test]
    fn settings_are_applied_to_every_given_row() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        let mut setting = Settings::new();
        setting.fixed_active = true;
        reduce(&mut state, Action::RowSettingChanged { indices: vec![0, 1], setting });
        assert!(state.rows.items[0].setting.fixed_active);
        assert!(state.rows.items[1].setting.fixed_active);
        assert!(!state.rows.items[0].chosen);
    }

    #[test]
    fn plain_click_selects_exactly_one_row() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        reduce(&mut state, Action::RowSelected { index: 1, ctrl: false, shift: false });
        assert!(!state.rows.items[0].chosen);
        assert!(state.rows.items[1].chosen);
        // clicking the same row again clears it
        reduce(&mut state, Action::RowSelected { index: 1, ctrl: false, shift: false });
        assert!(!state.rows.items[1].chosen);
    }

    #[test]
    fn select_all_and_clear_are_one_action_each() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        add(&mut state, "c");
        // Ctrl+A on an empty selection selects every row
        reduce(&mut state, Action::RowsAllSelected);
        assert!(state.rows.items.iter().all(|row| row.chosen));
        // running it again is a no-op (the version must not move, or every
        // subscriber would re-render for nothing)
        let version = state.rows.version;
        reduce(&mut state, Action::RowsAllSelected);
        assert_eq!(state.rows.version, version);
        // Esc clears the selection
        reduce(&mut state, Action::RowsCleared);
        assert!(state.rows.items.iter().all(|row| !row.chosen));
        let version = state.rows.version;
        reduce(&mut state, Action::RowsCleared);
        assert_eq!(state.rows.version, version);
    }

    #[test]
    fn ctrl_click_toggles_one_row_and_keeps_the_others() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        reduce(&mut state, Action::RowSelected { index: 0, ctrl: false, shift: false });
        reduce(&mut state, Action::RowSelected { index: 1, ctrl: true, shift: false });
        assert!(state.rows.items[0].chosen);
        assert!(state.rows.items[1].chosen);
        reduce(&mut state, Action::RowSelected { index: 0, ctrl: true, shift: false });
        assert!(!state.rows.items[0].chosen);
        assert!(state.rows.items[1].chosen);
    }

    #[test]
    fn the_viewport_and_cursors_are_committed_values() {
        let mut state = state();
        reduce(&mut state, Action::ViewCommitted(crate::state::types::View::new(120.0, 4.0)));
        assert_eq!(state.viewport.view.x_axis, 120.0);
        assert_eq!(state.viewport.view.size, 4.0);
        reduce(
            &mut state,
            Action::CursorsChanged(crate::state::types::Cursors { one: Some(10), two: None }),
        );
        assert_eq!(state.viewport.cursors.one, Some(10));
        assert_eq!(state.viewport.cursors.two, None);
    }

    #[test]
    fn opening_a_dump_stores_the_handle_and_derives_the_range() {
        // A real FST fixture: the range is derived from the handle by the
        // reducer (that is what `trace_end` and the fit zoom read), so this also
        // covers the `time_range_hint` path.
        let waveform = wave_insight_lib::waveform::Waveform::open_bytes(
            include_bytes!("../../../wave-insight-lib/tests/data/io_mode_wave.fst").to_vec(),
        )
        .expect("the fst fixture opens");
        let expected = waveform.time_range_hint().ok().flatten();
        let handle = DumpHandle::new(waveform);
        let mut state = state();
        assert_eq!(state.dump.waveform, None);
        assert_eq!(state.dump.time_range, None);
        reduce(&mut state, Action::DumpOpened { waveform: handle.clone() });
        assert_eq!(state.dump.time_range, expected);
        // the handle is the identity: same file, same handle
        assert_eq!(state.dump.waveform, Some(handle));
        // another handle for another file is *not* equal (pointer identity)
        let other = wave_insight_lib::waveform::Waveform::open_bytes(
            include_bytes!("../../../wave-insight-lib/tests/data/io_mode_wave.fst").to_vec(),
        )
        .expect("the fst fixture opens");
        assert_ne!(state.dump.waveform, Some(DumpHandle::new(other)));
    }

    #[test]
    fn shift_click_selects_the_range() {
        let mut state = state();
        for path in ["a", "b", "c", "d"] {
            add(&mut state, path);
        }
        reduce(&mut state, Action::RowSelected { index: 0, ctrl: false, shift: false });
        reduce(&mut state, Action::RowSelected { index: 3, ctrl: false, shift: true });
        let chosen: Vec<bool> = state.rows.items.iter().map(|row| row.chosen).collect();
        assert_eq!(chosen, vec![true, true, true, true]);
        // shift inside the range clears it
        reduce(&mut state, Action::RowSelected { index: 2, ctrl: false, shift: true });
        assert!(state.rows.items.iter().all(|row| !row.chosen));
    }
}
