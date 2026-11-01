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
use super::sources::Source;
use super::state::State;
use super::types::{clamp_ratio, clamp_sidebar, Dump, Goto, Row, SessionRestore, Status};

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
        Action::Problems(count) => {
            state.nav.problems = count;
        }
        Action::RowAdded {
            path,
            name,
            width,
            load,
            drive,
            bit,
        } => {
            if !path.is_empty() {
                let mut items = (*state.rows.items).clone();
                // a row is identified by its path *and* the bit it shows, so the
                // bus and its expanded bits can coexist
                if !items.iter().any(|row| row.path == path && row.bit == bit) {
                    // insert after the last chosen row, like the old push()
                    let insert_at = items
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.chosen)
                        .map(|(index, _)| index)
                        .next_back()
                        .map(|index| index + 1)
                        .unwrap_or(items.len());
                    // the display settings are remembered by signal path, so
                    // a signal that was configured in an earlier session (or
                    // before it was removed) comes back the way it looked
                    let setting = state.row_prefs.setting_for(&path);
                    items.insert(
                        insert_at,
                        Row {
                            path,
                            name,
                            width: width.max(1),
                            setting,
                            chosen: false,
                            load,
                            drive,
                            bit,
                        },
                    );
                    state.set_rows(items);
                }
            }
        }
        Action::RowsBitExpanded { index, width } => {
            let mut items = (*state.rows.items).clone();
            let Some(row) = items.get(index) else {
                return Vec::new();
            };
            // only a bus, and only once
            if row.bit.is_some() || !(2..=64).contains(&width) {
                return Vec::new();
            }
            if items
                .iter()
                .any(|candidate| candidate.path == row.path && candidate.bit.is_some())
            {
                return Vec::new();
            }
            let (path, name) = (row.path.clone(), row.name.clone());
            // the bits of a bus share the bus's remembered display settings
            let setting = state.row_prefs.setting_for(&path);
            // most significant bit on top, like Verdi's expanded bus
            let bits: Vec<Row> = (0..width)
                .rev()
                .map(|bit| Row {
                    path: path.clone(),
                    name: name.clone(),
                    width: 1,
                    setting: setting.clone(),
                    chosen: false,
                    load: Vec::new(),
                    drive: Vec::new(),
                    bit: Some(bit as u8),
                })
                .collect();
            items.splice(index + 1..index + 1, bits);
            state.set_rows(items);
        }
        Action::RowsBitCollapsed { index } => {
            let mut items = (*state.rows.items).clone();
            let Some(row) = items.get(index) else {
                return Vec::new();
            };
            let path = row.path.clone();
            // the bit rows are the contiguous block right below the bus
            let mut end = index + 1;
            while end < items.len() && items[end].path == path && items[end].bit.is_some() {
                end += 1;
            }
            if end > index + 1 {
                items.drain(index + 1..end);
                state.set_rows(items);
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
        Action::DumpOpened {
            waveform,
            name,
            bytes,
        } => {
            // the range is derived from the handle, never hand-written into the
            // store: the reducer is the one place that may compute it, and the
            // handle knows the GHW fallback (one body scan) behind one helper
            let (time_range, range_from_body) = waveform.time_range();
            state.dump = Dump {
                waveform: Some(waveform),
                name,
                time_range,
                range_from_body,
                bytes,
                session_restore: None,
            };
            // reopening a dump puts its remembered row list back (a different
            // dump leaves the current list alone)
            restore_session_rows(state);
        }
        Action::SourceOpened { file, text } | Action::SourceReloaded { file, text } => {
            if file.is_empty() {
                return Vec::new();
            }
            let mut items = (*state.sources.items).clone();
            let effects = match items.iter().position(|source| source.name == file) {
                Some(index) => {
                    // the same file was read again: replace the buffer and keep
                    // the tab where it is (a reload must not steal the active tab)
                    let source = &mut items[index];
                    source.text = text.clone();
                    source.version = source.version.wrapping_add(1);
                    state.set_sources(items, index);
                    vec![Effect::LspChange {
                        file: file.clone(),
                        text,
                    }]
                }
                None => {
                    items.push(Source::new(file.clone(), text.clone()));
                    let index = items.len() - 1;
                    state.set_sources(items, index);
                    vec![Effect::LspOpen {
                        file: file.clone(),
                        text,
                    }]
                }
            };
            state.nav.status = Some(Status::info(format!("parsed {file}")));
            return effects;
        }
        Action::SourceClosed { index } => {
            let mut items = (*state.sources.items).clone();
            if index >= items.len() {
                return Vec::new();
            }
            let closed = items.remove(index);
            let mut active = state.sources.active;
            if items.is_empty() {
                active = 0;
            } else {
                if index < active {
                    active = active.saturating_sub(1);
                }
                active = active.min(items.len() - 1);
            }
            state.set_sources(items, active);
            state.nav.status = Some(Status::info(format!("closed {}", closed.name)));
            return vec![Effect::LspClose { file: closed.name }];
        }
        Action::TabActivated { index } => {
            if index < state.sources.items.len() && index != state.sources.active {
                let items = (*state.sources.items).clone();
                state.set_sources(items, index);
            }
        }
        Action::FocusChanged { focus } => {
            state.focus = focus;
        }
        Action::JumpRequested { file, line, col } => {
            if line == 0 {
                return Vec::new();
            }
            let Some(index) = state.sources.index_of(&file) else {
                state.nav.status = Some(Status::error(format!(
                    "{file} is not open in the source pane — open it from the file picker again"
                )));
                return Vec::new();
            };
            state.nav.jump_token = state.nav.jump_token.wrapping_add(1);
            let token = state.nav.jump_token;
            state.nav.goto = Some(Goto {
                file: file.clone(),
                line,
                col,
                token,
            });
            state.nav.status = Some(Status::info(format!("{file}:{line}:{col}")));
            // the tab that contains the position must be the visible one
            if state.sources.active != index {
                let items = (*state.sources.items).clone();
                state.set_sources(items, index);
            }
            return vec![Effect::MonacoShow { file, line, col }];
        }
        Action::JumpHandled { token } => {
            if state.nav.goto.as_ref().map(|goto| goto.token) == Some(token) {
                state.nav.goto = None;
            }
        }
        Action::TopChanged { top } => {
            state.nav.top = top.clone();
            return vec![Effect::LspTop { top }];
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
            // the paths are collected before the rows are handed back to the
            // state (the row list and the memory are two fields of it)
            let mut paths: Vec<String> = Vec::new();
            for index in indices {
                if let Some(row) = items.get_mut(index) {
                    row.setting = setting.clone();
                    changed = true;
                    if !row.path.is_empty() && !paths.contains(&row.path) {
                        paths.push(row.path.clone());
                    }
                }
            }
            if changed {
                state.set_rows(items);
                // remember the choice by signal path: the next time this signal
                // is added (this session or after a reload) it starts like this
                for path in paths {
                    state.row_prefs.record(&path, &setting);
                }
            }
        }
    }
    Vec::new()
}

/// Put the remembered row list of the dump that just opened back on the wave.
///
/// Only runs for a dump with an identity (a named file): a dump that arrived as
/// raw bytes is never remembered. When there is no remembered session the
/// current list is left alone, exactly as before this feature existed.
///
/// A remembered row whose signal is not in the dump any more (or whose bit does
/// not fit the bus it belongs to) is skipped and counted in
/// [`SessionRestore`]; the open itself still succeeds, and the app mentions the
/// skipped rows once in the status line instead of turning the load into an
/// error. The stored list is rewritten with what actually came back, so the
/// skipped rows are forgotten on the next write.
fn restore_session_rows(state: &mut State) {
    let Some(id) = state.dump.identity() else {
        return;
    };
    let Some(session) = state.sessions.get(&id).cloned() else {
        return;
    };
    let Some(handle) = state.dump.waveform.as_ref() else {
        return;
    };
    let waveform = handle.waveform();
    let mut items = Vec::with_capacity(session.rows.len());
    let mut skipped = 0usize;
    for stored in &session.rows {
        let Some(signal) = waveform.signal_by_path(&stored.path) else {
            skipped += 1;
            continue;
        };
        let width = match stored.bit {
            Some(bit) if signal.width() > u32::from(bit) => 1,
            Some(_) => {
                // the bus is narrower than the bit this row used to show
                skipped += 1;
                continue;
            }
            None => signal.width().max(1) as usize,
        };
        items.push(Row {
            path: stored.path.clone(),
            name: if stored.name.is_empty() {
                stored.path.clone()
            } else {
                stored.name.clone()
            },
            width,
            // the display settings are remembered by path, so a restored row
            // comes back with the format the user chose for that signal
            setting: state.row_prefs.setting_for(&stored.path),
            chosen: false,
            load: stored.load.clone(),
            drive: stored.drive.clone(),
            bit: stored.bit,
        });
    }
    let restored = items.len();
    state.dump.session_restore = Some(SessionRestore { restored, skipped });
    state.set_rows(items);
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
    use crate::state::persist::SessionRow;
    use crate::state::types::{
        DumpHandle, DumpId, Layout, Settings, Status, StatusTone, EDITOR_RATIO_MIN, SIDEBAR_MIN,
    };
    use wave_insight_lib::data_struct::ShowType;

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
                bit: None,
            },
        );
    }

    /// The VCD fixture the QA matrix uses: real signal paths, so the restore
    /// filter (a signal that is not in the dump) is exercised for real.
    fn wave_vcd() -> (DumpHandle, u64) {
        let bytes = include_bytes!("../../../qa/fixtures/wave.vcd").to_vec();
        let size = bytes.len() as u64;
        let waveform =
            wave_insight_lib::waveform::Waveform::open_bytes(bytes).expect("the vcd fixture opens");
        (DumpHandle::new(waveform), size)
    }

    fn session_row(path: &str, bit: Option<u8>) -> SessionRow {
        SessionRow {
            path: path.to_string(),
            name: path.to_string(),
            width: if bit.is_some() { 1 } else { 8 },
            load: Vec::new(),
            drive: Vec::new(),
            bit,
        }
    }

    fn paths(state: &State) -> Vec<String> {
        state
            .rows
            .items
            .iter()
            .map(|row| row.path.clone())
            .collect()
    }

    #[test]
    fn expanding_a_bus_adds_one_row_per_bit_most_significant_first() {
        let mut state = state();
        add(&mut state, "top.pc");
        reduce(&mut state, Action::RowsBitExpanded { index: 0, width: 4 });
        assert_eq!(
            state
                .rows
                .items
                .iter()
                .map(|row| row.bit)
                .collect::<Vec<_>>(),
            vec![None, Some(3), Some(2), Some(1), Some(0)]
        );
        assert!(state.rows.items.iter().all(|row| row.path == "top.pc"));
        assert!(state.rows.items[1..].iter().all(|row| row.width == 1));
        // expanding twice does not add a second block
        reduce(&mut state, Action::RowsBitExpanded { index: 0, width: 4 });
        assert_eq!(state.rows.items.len(), 5);
        // collapsing removes exactly the block below the bus
        reduce(&mut state, Action::RowsBitCollapsed { index: 0 });
        assert_eq!(
            state
                .rows
                .items
                .iter()
                .map(|row| row.bit)
                .collect::<Vec<_>>(),
            vec![None]
        );
        // and a bit row is not the bus, so it is not a duplicate either
        reduce(
            &mut state,
            Action::RowAdded {
                path: "top.pc".into(),
                name: "top.pc".into(),
                width: 1,
                load: vec![],
                drive: vec![],
                bit: Some(1),
            },
        );
        assert_eq!(state.rows.items.len(), 2);
        reduce(
            &mut state,
            Action::RowAdded {
                path: "top.pc".into(),
                name: "top.pc".into(),
                width: 1,
                load: vec![],
                drive: vec![],
                bit: Some(1),
            },
        );
        assert_eq!(state.rows.items.len(), 2);
    }

    #[test]
    fn resize_sidebar_clamps_into_the_window() {
        let mut state = state();
        reduce(
            &mut state,
            Action::ResizeSidebar {
                width: 340.0,
                max: 640.0,
            },
        );
        assert_eq!(state.layout.sidebar_w, 340.0);
        reduce(
            &mut state,
            Action::ResizeSidebar {
                width: 5000.0,
                max: 640.0,
            },
        );
        assert_eq!(state.layout.sidebar_w, 640.0);
        reduce(
            &mut state,
            Action::ResizeSidebar {
                width: 10.0,
                max: 640.0,
            },
        );
        assert_eq!(state.layout.sidebar_w, SIDEBAR_MIN);
    }

    // --- R2 step 3: sources, focus and the jump request ---------------------

    fn open(state: &mut State, file: &str) -> Vec<Effect> {
        reduce(
            state,
            Action::SourceOpened {
                file: file.to_string(),
                text: format!("// {file}\n"),
            },
        )
    }

    #[test]
    fn opening_a_source_adds_a_document_and_emits_did_open() {
        let mut state = state();
        let effects = open(&mut state, "top.sv");
        assert_eq!(
            effects,
            vec![Effect::LspOpen {
                file: "top.sv".to_string(),
                text: "// top.sv\n".to_string()
            }]
        );
        assert_eq!(state.sources.len(), 1);
        assert_eq!(state.sources.active, 0);
        let source = &state.sources.items[0];
        assert_eq!(source.name, "top.sv");
        assert_eq!(source.language, crate::state::sources::SOURCE_LANGUAGE);
        assert_eq!(source.version, 1);
        // the status line reports it (the shell no longer does)
        assert!(state
            .nav
            .status
            .as_ref()
            .is_some_and(|status| status.text.contains("top.sv")));
        // a second document becomes the active tab
        let effects = open(&mut state, "cpu.sv");
        assert!(matches!(effects.as_slice(), [Effect::LspOpen { .. }]));
        assert_eq!(state.sources.len(), 2);
        assert_eq!(state.sources.active, 1);
    }

    #[test]
    fn opening_the_same_file_again_replaces_its_text_and_emits_did_change() {
        let mut state = state();
        open(&mut state, "top.sv");
        let effects = reduce(
            &mut state,
            Action::SourceReloaded {
                file: "top.sv".to_string(),
                text: "// edited\n".to_string(),
            },
        );
        assert_eq!(
            effects,
            vec![Effect::LspChange {
                file: "top.sv".to_string(),
                text: "// edited\n".to_string()
            }]
        );
        // one tab, new text, bumped version: the editor keeps the same document
        assert_eq!(state.sources.len(), 1);
        assert_eq!(state.sources.items[0].text, "// edited\n");
        assert_eq!(state.sources.items[0].version, 2);
    }

    #[test]
    fn closing_a_tab_drops_the_document_and_emits_did_close() {
        let mut state = state();
        open(&mut state, "a.sv");
        open(&mut state, "b.sv");
        let effects = reduce(&mut state, Action::SourceClosed { index: 0 });
        assert_eq!(
            effects,
            vec![Effect::LspClose {
                file: "a.sv".to_string()
            }]
        );
        assert_eq!(state.sources.len(), 1);
        assert_eq!(state.sources.items[0].name, "b.sv");
        // the active tab shifted left with the closed one
        assert_eq!(state.sources.active, 0);
        // an out-of-range close is a no-op (no effect, no version bump)
        let version = state.sources.version;
        assert!(reduce(&mut state, Action::SourceClosed { index: 9 }).is_empty());
        assert_eq!(state.sources.version, version);
        // closing the last tab empties the slice
        reduce(&mut state, Action::SourceClosed { index: 0 });
        assert!(state.sources.is_empty());
    }

    #[test]
    fn activating_a_tab_is_clamped_and_versioned() {
        let mut state = state();
        open(&mut state, "a.sv");
        open(&mut state, "b.sv");
        assert_eq!(state.sources.active, 1);
        reduce(&mut state, Action::TabActivated { index: 0 });
        assert_eq!(state.sources.active, 0);
        // out of range is ignored
        let version = state.sources.version;
        reduce(&mut state, Action::TabActivated { index: 9 });
        assert_eq!(state.sources.active, 0);
        assert_eq!(state.sources.version, version);
    }

    #[test]
    fn the_focus_slice_holds_the_inspector_selection() {
        let mut state = state();
        assert_eq!(state.focus.signal, None);
        reduce(
            &mut state,
            Action::FocusChanged {
                focus: crate::state::types::Focus {
                    signal: Some("top.pc".to_string()),
                },
            },
        );
        assert_eq!(state.focus.signal.as_deref(), Some("top.pc"));
    }

    #[test]
    fn a_jump_request_needs_an_open_file_and_emits_monaco_show() {
        let mut state = state();
        // nothing open: the request is refused with an error, no effect
        let effects = reduce(
            &mut state,
            Action::JumpRequested {
                file: "top.sv".to_string(),
                line: 3,
                col: 1,
            },
        );
        assert!(effects.is_empty());
        assert!(state.nav.goto.is_none());
        assert!(state
            .nav
            .status
            .as_ref()
            .is_some_and(|status| status.is_error()));

        open(&mut state, "top.sv");
        open(&mut state, "cpu.sv");
        let effects = reduce(
            &mut state,
            Action::JumpRequested {
                file: "top.sv".to_string(),
                line: 12,
                col: 4,
            },
        );
        assert_eq!(
            effects,
            vec![Effect::MonacoShow {
                file: "top.sv".to_string(),
                line: 12,
                col: 4
            }]
        );
        let goto = state.nav.goto.clone().expect("the request is pending");
        assert_eq!((goto.file.as_str(), goto.line, goto.col), ("top.sv", 12, 4));
        assert_eq!(goto.token, 1);
        // the tab with the position is the active one
        assert_eq!(state.sources.active, 0);
        // a second request gets a new token, so a repeat still scrolls
        reduce(
            &mut state,
            Action::JumpRequested {
                file: "top.sv".to_string(),
                line: 12,
                col: 4,
            },
        );
        assert_eq!(state.nav.goto.as_ref().map(|goto| goto.token), Some(2));
        // line 0 is not a position
        let effects = reduce(
            &mut state,
            Action::JumpRequested {
                file: "top.sv".to_string(),
                line: 0,
                col: 0,
            },
        );
        assert!(effects.is_empty());
    }

    #[test]
    fn a_handled_jump_clears_only_the_matching_request() {
        let mut state = state();
        open(&mut state, "top.sv");
        reduce(
            &mut state,
            Action::JumpRequested {
                file: "top.sv".to_string(),
                line: 2,
                col: 1,
            },
        );
        let token = state.nav.goto.as_ref().map(|goto| goto.token).unwrap();
        // a stale token (the request was replaced) does not clear the new one
        reduce(&mut state, Action::JumpHandled { token: token + 7 });
        assert!(state.nav.goto.is_some());
        reduce(&mut state, Action::JumpHandled { token });
        assert!(state.nav.goto.is_none());
    }

    #[test]
    fn a_top_change_is_mirrored_to_the_language_server() {
        let mut state = state();
        let effects = reduce(
            &mut state,
            Action::TopChanged {
                top: Some("top".to_string()),
            },
        );
        assert_eq!(
            effects,
            vec![Effect::LspTop {
                top: Some("top".to_string())
            }]
        );
        assert_eq!(state.nav.top.as_deref(), Some("top"));
        let effects = reduce(&mut state, Action::TopChanged { top: None });
        assert_eq!(effects, vec![Effect::LspTop { top: None }]);
    }

    #[test]
    fn the_sources_slice_compares_by_version() {
        let mut state = state();
        open(&mut state, "a.sv");
        let before = state.sources.slice();
        // an action that changes nothing must not move the version
        reduce(&mut state, Action::TabActivated { index: 0 });
        assert_eq!(before, state.sources.slice());
        open(&mut state, "b.sv");
        assert_ne!(before, state.sources.slice());
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
            Action::Layout(Layout {
                sidebar_w: 4000.0,
                editor_ratio: 0.02,
                drawer_open: false,
            }),
        );
        assert_eq!(state.layout.sidebar_w, 4000.0_f64.max(SIDEBAR_MIN));
        assert_eq!(state.layout.editor_ratio, EDITOR_RATIO_MIN);
        assert!(!state.layout.drawer_open);
    }

    #[test]
    fn status_is_set_and_cleared() {
        let mut state = state();
        assert_eq!(state.nav.status, None);
        reduce(
            &mut state,
            Action::Status(Status::error("top.sv: not utf-8")),
        );
        let status = state.nav.status.clone().expect("status");
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(status.text, "top.sv: not utf-8");
        reduce(&mut state, Action::ClearStatus);
        assert_eq!(state.nav.status, None);
    }

    #[test]
    fn the_problem_count_is_stored_in_nav() {
        let mut state = state();
        assert_eq!(state.nav.problems, 0);
        assert!(reduce(&mut state, Action::Problems(3)).is_empty());
        assert_eq!(state.nav.problems, 3);
        reduce(&mut state, Action::Problems(0));
        assert_eq!(state.nav.problems, 0);
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
        reduce(
            &mut state,
            Action::RowSelected {
                index: 0,
                ctrl: false,
                shift: false,
            },
        );
        add(&mut state, "c");
        assert_eq!(paths(&state), vec!["a", "c", "b"]);
    }

    #[test]
    fn removing_rows_shifts_the_rest_and_ignores_bad_indices() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        add(&mut state, "c");
        reduce(
            &mut state,
            Action::RowsRemoved {
                indices: vec![2, 0, 9],
            },
        );
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
        reduce(
            &mut state,
            Action::RowSettingChanged {
                indices: vec![0, 1],
                setting,
            },
        );
        assert!(state.rows.items[0].setting.fixed_active);
        assert!(state.rows.items[1].setting.fixed_active);
        assert!(!state.rows.items[0].chosen);
    }

    #[test]
    fn a_remembered_setting_comes_back_when_the_signal_is_added_again() {
        let mut state = state();
        add(&mut state, "top.pc");
        let mut setting = Settings::new();
        setting.show_type = wave_insight_lib::data_struct::ShowType::Bin;
        setting.analog_active = true;
        reduce(
            &mut state,
            Action::RowSettingChanged {
                indices: vec![0],
                setting: setting.clone(),
            },
        );
        // the memory is by signal path, not by row: removing the row and adding
        // the same signal again restores the setting
        reduce(&mut state, Action::RowsRemoved { indices: vec![0] });
        add(&mut state, "top.pc");
        assert_eq!(state.rows.items[0].setting, setting);

        // a reset to the default is forgotten, so the next add starts at the
        // default instead of the old choice
        reduce(
            &mut state,
            Action::RowSettingChanged {
                indices: vec![0],
                setting: Settings::new(),
            },
        );
        reduce(&mut state, Action::RowsRemoved { indices: vec![0] });
        add(&mut state, "top.pc");
        assert_eq!(state.rows.items[0].setting, Settings::new());

        // a signal that was never configured starts at the default too
        add(&mut state, "top.other");
        assert_eq!(state.rows.items[1].setting, Settings::new());
    }

    #[test]
    fn plain_click_selects_exactly_one_row() {
        let mut state = state();
        add(&mut state, "a");
        add(&mut state, "b");
        reduce(
            &mut state,
            Action::RowSelected {
                index: 1,
                ctrl: false,
                shift: false,
            },
        );
        assert!(!state.rows.items[0].chosen);
        assert!(state.rows.items[1].chosen);
        // clicking the same row again clears it
        reduce(
            &mut state,
            Action::RowSelected {
                index: 1,
                ctrl: false,
                shift: false,
            },
        );
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
        reduce(
            &mut state,
            Action::RowSelected {
                index: 0,
                ctrl: false,
                shift: false,
            },
        );
        reduce(
            &mut state,
            Action::RowSelected {
                index: 1,
                ctrl: true,
                shift: false,
            },
        );
        assert!(state.rows.items[0].chosen);
        assert!(state.rows.items[1].chosen);
        reduce(
            &mut state,
            Action::RowSelected {
                index: 0,
                ctrl: true,
                shift: false,
            },
        );
        assert!(!state.rows.items[0].chosen);
        assert!(state.rows.items[1].chosen);
    }

    #[test]
    fn the_viewport_and_cursors_are_committed_values() {
        let mut state = state();
        reduce(
            &mut state,
            Action::ViewCommitted(crate::state::types::View::new(120.0, 4.0)),
        );
        assert_eq!(state.viewport.view.x_axis, 120.0);
        assert_eq!(state.viewport.view.size, 4.0);
        reduce(
            &mut state,
            Action::CursorsChanged(crate::state::types::Cursors {
                one: Some(10),
                two: None,
            }),
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
        reduce(
            &mut state,
            Action::DumpOpened {
                waveform: handle.clone(),
                name: Some("io_mode_wave.fst".to_string()),
                bytes: Some(4096),
            },
        );
        assert_eq!(state.dump.time_range, expected);
        // the file the user picked is part of the dump slice (exports use it)
        assert_eq!(state.dump.name.as_deref(), Some("io_mode_wave.fst"));
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
    fn reopening_the_same_dump_restores_its_rows_in_order() {
        let (handle, size) = wave_vcd();
        let mut state = state();
        reduce(
            &mut state,
            Action::DumpOpened {
                waveform: handle,
                name: Some("wave.vcd".to_string()),
                bytes: Some(size),
            },
        );
        // first open: nothing is remembered yet, so nothing is restored
        assert!(state.rows.items.is_empty());
        assert_eq!(state.dump.session_restore, None);

        add(&mut state, "top.pc");
        add(&mut state, "top.clk");
        let mut setting = Settings::new();
        setting.show_type = ShowType::Bin;
        reduce(
            &mut state,
            Action::RowSettingChanged {
                indices: vec![0],
                setting: setting.clone(),
            },
        );
        // every row change records the list under the dump identity
        assert_eq!(state.sessions.entries().len(), 1);
        assert_eq!(
            state.sessions.entries()[0].id,
            DumpId::new("wave.vcd", Some(size))
        );
        assert_eq!(state.sessions.entries()[0].rows.len(), 2);

        // a reload: a fresh store has no rows but the remembered sessions and
        // the per-signal settings
        let mut reopened = State {
            sessions: state.sessions.clone(),
            row_prefs: state.row_prefs.clone(),
            ..State::default()
        };
        let (handle, size) = wave_vcd();
        reduce(
            &mut reopened,
            Action::DumpOpened {
                waveform: handle,
                name: Some("wave.vcd".to_string()),
                bytes: Some(size),
            },
        );
        assert_eq!(paths(&reopened), vec!["top.pc", "top.clk"]);
        // the width comes from the dump, the format from the remembered settings
        assert_eq!(reopened.rows.items[0].width, 8);
        assert_eq!(reopened.rows.items[0].setting, setting);
        assert!(!reopened.rows.items[0].chosen);
        assert_eq!(
            reopened.dump.session_restore,
            Some(SessionRestore {
                restored: 2,
                skipped: 0
            })
        );
    }

    #[test]
    fn remembered_rows_whose_signal_is_gone_are_skipped_and_counted() {
        let (handle, size) = wave_vcd();
        let mut state = state();
        state.sessions.record(
            DumpId::new("wave.vcd", Some(size)),
            vec![
                session_row("top.pc", None),
                session_row("top.gone", None),
                session_row("top.pc", Some(7)),
                // pc is eight bits wide, so bit 8 does not fit
                session_row("top.pc", Some(8)),
            ],
        );
        reduce(
            &mut state,
            Action::DumpOpened {
                waveform: handle,
                name: Some("wave.vcd".to_string()),
                bytes: Some(size),
            },
        );
        let bits: Vec<Option<u8>> = state.rows.items.iter().map(|row| row.bit).collect();
        assert_eq!(bits, vec![None, Some(7)]);
        assert_eq!(
            state.dump.session_restore,
            Some(SessionRestore {
                restored: 2,
                skipped: 2
            })
        );
        // what actually came back is recorded again; the skipped rows are gone
        let remembered = state
            .sessions
            .get(&DumpId::new("wave.vcd", Some(size)))
            .expect("the session is recorded");
        assert_eq!(remembered.rows.len(), 2);
    }

    #[test]
    fn a_different_or_nameless_dump_does_not_restore_rows() {
        let (handle, size) = wave_vcd();
        let mut state = state();
        state.sessions.record(
            DumpId::new("wave.vcd", Some(size)),
            vec![session_row("top.pc", None)],
        );
        // same name, different size: a different file, so the list is left alone
        reduce(
            &mut state,
            Action::DumpOpened {
                waveform: handle,
                name: Some("wave.vcd".to_string()),
                bytes: Some(size + 1),
            },
        );
        assert!(state.rows.items.is_empty());
        assert_eq!(state.dump.session_restore, None);

        // a dump that arrived as raw bytes has no name, so it has no identity:
        // nothing is restored and nothing is remembered for it
        let (handle, _) = wave_vcd();
        reduce(
            &mut state,
            Action::DumpOpened {
                waveform: handle,
                name: None,
                bytes: None,
            },
        );
        assert!(state.rows.items.is_empty());
        add(&mut state, "top.pc");
        assert_eq!(state.sessions.entries().len(), 1);
    }

    #[test]
    fn shift_click_selects_the_range() {
        let mut state = state();
        for path in ["a", "b", "c", "d"] {
            add(&mut state, path);
        }
        reduce(
            &mut state,
            Action::RowSelected {
                index: 0,
                ctrl: false,
                shift: false,
            },
        );
        reduce(
            &mut state,
            Action::RowSelected {
                index: 3,
                ctrl: false,
                shift: true,
            },
        );
        let chosen: Vec<bool> = state.rows.items.iter().map(|row| row.chosen).collect();
        assert_eq!(chosen, vec![true, true, true, true]);
        // shift inside the range clears it
        reduce(
            &mut state,
            Action::RowSelected {
                index: 2,
                ctrl: false,
                shift: true,
            },
        );
        assert!(state.rows.items.iter().all(|row| !row.chosen));
    }
}
