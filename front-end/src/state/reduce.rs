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

use std::rc::Rc;

use super::action::{Action, Effect};
use super::dumps::DumpEntry;
use super::persist::SessionRow;
use super::sources::Source;
use super::state::State;
use super::types::{
    clamp_ratio, clamp_sidebar, Dialog, DumpHandle, DumpId, Goto, Picker, Row, SessionRestore,
    Status,
};
use crate::nav::SidebarTree;

/// How many bytes of dump files may be open at once (docs/design/multi-dump.md
/// §4.3): every open dump keeps its file bytes in wasm memory. 1.5 GiB of file
/// bytes is already several GiB of wasm memory once the renderer cache is
/// added; the guard refuses the next open with an actionable message instead of
/// letting the tab kill the page.
pub const DUMP_BUDGET_BYTES: u64 = 1536 * 1024 * 1024;

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
                let mut items = state.rows_items();
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
            let mut items = state.rows_items();
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
            let mut items = state.rows_items();
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
                let mut items = state.rows_items();
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
            let mut items = state.rows_items();
            if index < items.len() && choose(&mut items, index, ctrl, shift) {
                state.set_rows(items);
            }
        }
        Action::RowsAllSelected => {
            let mut items = state.rows_items();
            if items.iter().any(|row| !row.chosen) {
                for row in items.iter_mut() {
                    row.chosen = true;
                }
                state.set_rows(items);
            }
        }
        Action::RowsCleared => {
            let mut items = state.rows_items();
            if items.iter().any(|row| row.chosen) {
                for row in items.iter_mut() {
                    row.chosen = false;
                }
                state.set_rows(items);
            }
        }
        Action::RowsReordered { from, to } => {
            let mut items = state.rows_items();
            if from < items.len() && to < items.len() && from != to {
                let row = items.remove(from);
                items.insert(to, row);
                state.set_rows(items);
            }
        }
        Action::DumpAdded {
            id,
            handle,
            fingerprint,
        } => {
            // the same file is already open: focus its tab, never a second copy.
            // But the identity is only name + byte size, and a re-simulated dump
            // can keep both while every value changes: when the content
            // fingerprint differs, that tab reloads the new bytes in place
            // instead of keeping the old waveform on screen.
            if let Some(index) = state.dumps.index_of(&id) {
                let same_content = match (state.dumps.items[index].fingerprint, fingerprint) {
                    (Some(open), Some(incoming)) => open == incoming,
                    // a transport that reports no fingerprint keeps the old
                    // dedupe rule instead of reparsing blind
                    _ => true,
                };
                if same_content {
                    activate_dump(state, index);
                } else {
                    reload_dump(state, index, handle, fingerprint);
                }
                return vec![Effect::LspTop {
                    top: state.design.top().map(str::to_owned),
                }];
            }
            // memory budget: every open dump keeps its file bytes in wasm memory
            let total = state.dumps.total_bytes();
            let incoming = id.bytes.unwrap_or(0);
            if total.saturating_add(incoming) > DUMP_BUDGET_BYTES {
                state.nav.status = Some(Status::error(format!(
                    "cannot open {}: the open dumps already use {} MB (limit {} MB) — close a dump tab first",
                    id.name,
                    total / (1024 * 1024),
                    DUMP_BUDGET_BYTES / (1024 * 1024)
                )));
                return Vec::new();
            }
            // the range is derived from the handle, never hand-written into the
            // store: the reducer is the one place that may compute it, and the
            // handle knows the GHW fallback (one body scan) behind one helper
            let (time_range, range_from_body) = handle.time_range();
            let (remembered, session_restore) = remembered_rows(state, &id, &handle);
            let rows = match remembered {
                Some(rows) => rows,
                // the first dump of a sources-only session keeps the rows that
                // are already on the wave; a later tab starts empty
                None if state.dumps.is_empty() => take_scratch_rows(state),
                None => Vec::new(),
            };
            let healed = session_restore.is_some();
            let rows_version = state.dumps.next_rows_version();
            let mut entry =
                DumpEntry::new(id.clone(), handle, time_range, range_from_body, fingerprint);
            entry.rows = super::state::Rows {
                items: Rc::new(rows),
                version: rows_version,
            };
            entry.session_restore = session_restore;
            let remembered = (*entry.rows.items)
                .iter()
                .map(SessionRow::from_row)
                .collect();
            let mut items = (*state.dumps.items).clone();
            items.push(entry);
            state.dumps.items = Rc::new(items);
            let index = state.dumps.len() - 1;
            state.dumps.touch();
            state.dumps.touch_tabs();
            // a restored list is written back as it came back: the rows the dump
            // no longer has are forgotten, not retried on every open
            if healed {
                state.sessions.record(id, remembered);
            }
            activate_dump(state, index);
            return vec![Effect::LspTop {
                top: state.design.top().map(str::to_owned),
            }];
        }
        Action::DumpTabActivated { index } => {
            if index < state.dumps.len() && index != state.dumps.active_index() {
                activate_dump(state, index);
                return vec![Effect::LspTop {
                    top: state.design.top().map(str::to_owned),
                }];
            }
        }
        Action::DumpClosed { index } => {
            if index >= state.dumps.len() {
                return Vec::new();
            }
            let mut items = (*state.dumps.items).clone();
            let closed = items.remove(index);
            let mut active = state.dumps.active_index();
            if items.is_empty() {
                active = 0;
            } else {
                if index < active {
                    active = active.saturating_sub(1);
                }
                active = active.min(items.len() - 1);
            }
            state.dumps.items = Rc::new(items);
            state.dumps.active = active;
            state.dumps.touch();
            state.dumps.touch_tabs();
            // keep the split consistent with the new tab list: the closed dump
            // cannot be the right pane, and the right pane must not be the
            // (possibly new) active dump
            if let Some(split) = state.split {
                if state.dumps.len() < 2 {
                    state.split = None;
                } else {
                    let mut right = split.right;
                    if right == index {
                        right = other_dump(state, index);
                    } else if right > index {
                        right = right.saturating_sub(1);
                    }
                    state.split = Some(super::split::Split {
                        right: other_dump(state, right),
                        linked: split.linked,
                    });
                }
            }
            sync_top_hint(state);
            refresh_tree(state);
            state.nav.status = Some(Status::info(format!("closed {}", closed.id.name)));
            return vec![Effect::LspTop {
                top: state.design.top().map(str::to_owned),
            }];
        }
        Action::SourceOpened { file, text } | Action::SourceReloaded { file, text } => {
            if file.is_empty() {
                return Vec::new();
            }
            // an editor edit re-dispatches the buffer it already holds; only a
            // real change re-parses and re-elaborates
            let unchanged = state
                .sources
                .index_of(&file)
                .is_some_and(|index| state.sources.items[index].text == text);
            if unchanged {
                return Vec::new();
            }
            let text_for_design = text.clone();
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
            // the design database lives in the store now: the reducer is the
            // one place a source is parsed, so every pane reads the same design
            state.design.add_source(&file, &text_for_design);
            refresh_tree(state);
            state.nav.status = Some(Status::info(format!("parsed {file}")));
            let mut effects = effects;
            effects.push(Effect::LspTop {
                top: state.design.top().map(str::to_owned),
            });
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
            // the selection belongs to the active dump (or to the no-dump
            // scratch slice before the first dump is opened)
            state.set_focus(focus);
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
        Action::PickerOpened { title, paths } => {
            if paths.is_empty() {
                return Vec::new();
            }
            state.nav.picker = Some(Picker { title, paths });
        }
        Action::PickerChosen { index } => {
            // the choice itself is the app's work (it resolves the path); the
            // store only owns whether the dialog is open
            if let Some(picker) = &state.nav.picker {
                if index < picker.paths.len() {
                    state.nav.picker = None;
                }
            }
        }
        Action::PickerCancelled => {
            state.nav.picker = None;
        }
        Action::DialogOpened { indices } => {
            if indices.is_empty() {
                return Vec::new();
            }
            state.nav.dialog = Some(Dialog { indices });
        }
        Action::DialogClosed => {
            state.nav.dialog = None;
        }
        Action::SplitOpened { right } => {
            // a split needs a second dump to show on the right
            if state.dumps.len() < 2 {
                return Vec::new();
            }
            state.split = Some(super::split::Split {
                right: other_dump(state, right),
                linked: true,
            });
        }
        Action::SplitClosed => {
            state.split = None;
        }
        Action::SplitRightChanged { right } => {
            if state.dumps.len() < 2 {
                state.split = None;
                return Vec::new();
            }
            let linked = state.split.map(|split| split.linked).unwrap_or(true);
            state.split = Some(super::split::Split {
                right: other_dump(state, right),
                linked,
            });
        }
        Action::SplitLinkedToggled => {
            if let Some(split) = state.split.as_mut() {
                split.linked = !split.linked;
            }
        }
        Action::SetSplitRatio(ratio) => {
            state.layout.split_ratio = crate::state::types::clamp_split_ratio(ratio);
        }
        Action::ViewCommittedFor { dump, view } => {
            // one dump's own committed viewport (the split's unlinked pane);
            // no shell subscription reads it, so a gesture commit here cannot
            // re-render the shell
            if let Some(entry) = state.dumps.items_mut().get_mut(dump) {
                entry.viewport.view = view;
                state.dumps.touch();
            }
        }
        Action::ViewCommitted(view) => {
            let mut viewport = state.viewport_slice();
            viewport.view = view;
            state.set_viewport(viewport);
        }
        Action::CursorsChanged(cursors) => {
            let mut viewport = state.viewport_slice();
            viewport.cursors = cursors;
            state.set_viewport(viewport);
        }
        Action::TreeSelectionToggled { path } => {
            let selection = &mut state.nav.tree_selection;
            if let Some(index) = selection
                .paths
                .iter()
                .position(|selected| selected == &path)
            {
                selection.paths.remove(index);
            } else {
                selection.paths.push(path.clone());
            }
            selection.anchor = Some(path);
        }
        Action::TreeSelectionReset { path } => {
            state.nav.tree_selection = crate::state::types::TreeSelection {
                paths: vec![path.clone()],
                anchor: Some(path),
            };
        }
        Action::TreeSelectionRanged { paths } => {
            let mut paths = paths;
            paths.dedup();
            let anchor = state
                .nav
                .tree_selection
                .anchor
                .clone()
                .or_else(|| paths.first().cloned());
            state.nav.tree_selection = crate::state::types::TreeSelection { paths, anchor };
        }
        Action::TreeSelectionCleared => {
            state.nav.tree_selection = crate::state::types::TreeSelection::default();
        }
        Action::RowsAdded { rows } => {
            // bulk add: one dispatch, one rows-version bump, one session write
            if rows.is_empty() {
                return Vec::new();
            }
            if rows.len() > crate::state::bulk::BULK_ADD_CAP {
                state.nav.status = Some(Status::error(format!(
                    "too many signals to add at once: {} (limit {}) — select fewer modules",
                    rows.len(),
                    crate::state::bulk::BULK_ADD_CAP
                )));
                return Vec::new();
            }
            let mut items = state.rows_items();
            // insert after the last chosen row, like a single add
            let mut insert_at = items
                .iter()
                .enumerate()
                .filter(|(_, row)| row.chosen)
                .map(|(index, _)| index)
                .next_back()
                .map(|index| index + 1)
                .unwrap_or(items.len());
            let mut added = 0usize;
            let mut skipped = 0usize;
            for mut row in rows {
                if row.path.is_empty()
                    || items
                        .iter()
                        .any(|existing| existing.path == row.path && existing.bit == row.bit)
                {
                    skipped += 1;
                    continue;
                }
                // the remembered display settings win, exactly like a single add
                row.setting = state.row_prefs.setting_for(&row.path);
                row.chosen = false;
                row.width = row.width.max(1);
                items.insert(insert_at, row);
                insert_at += 1;
                added += 1;
            }
            if added == 0 {
                state.nav.status = Some(Status::info(format!(
                    "no new signals: {skipped} already in the wave"
                )));
                return Vec::new();
            }
            state.set_rows(items);
            state.nav.status = Some(Status::info(format!(
                "added {added} signals ({skipped} already in the wave)"
            )));
        }
        Action::RowSettingChanged { indices, setting } => {
            let mut items = state.rows_items();
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

/// A dump index that is *not* the active one (the right pane must show another
/// dump): `wanted` when it is valid and different, otherwise the next one.
fn other_dump(state: &State, wanted: usize) -> usize {
    let len = state.dumps.len();
    let active = state.dumps.active_index();
    if wanted < len && wanted != active {
        wanted
    } else if len > 1 {
        (active + 1) % len
    } else {
        0
    }
}

/// Make `index` the active dump: the tree, the design's top hint and the LSP top
/// follow it, and the module selection (which belonged to the old tree) is
/// cleared. The split's right pane must keep showing *another* dump, so the
/// indices are swapped when the left pane takes the right dump's place.
fn activate_dump(state: &mut State, index: usize) {
    if state.dumps.is_empty() {
        return;
    }
    let previous = state.dumps.active_index();
    state.dumps.active = index.min(state.dumps.len() - 1);
    // a fresh row version: the panes' `rows` subscription compares versions, and
    // two tabs can hold lists at the same per-entry version
    let version = state.dumps.next_rows_version();
    if let Some(entry) = state.dumps.active_mut() {
        entry.rows.version = version;
    }
    state.dumps.touch();
    // the active index is part of the shell's tab strip snapshot
    state.dumps.touch_tabs();
    // the right pane follows the active dump: when the left pane took the right
    // dump's place, the previous left dump becomes the right pane
    if let Some(split) = state.split.as_mut() {
        if split.right == state.dumps.active {
            split.right = previous;
        }
    }
    sync_top_hint(state);
    refresh_tree(state);
}

/// Replace the handle of an already-open tab in place.
///
/// The same dump identity (file name + byte size) was opened again but the
/// content fingerprint differs: the testbench was re-simulated, so the tab has
/// to show the new waveform. The tab, its row order/settings, the committed
/// viewport and the cursors stay; the rows are re-resolved against the new dump
/// and the ones whose signal is gone (or whose bit no longer fits the bus) are
/// dropped and reported through [`SessionRestore`], exactly like a remembered
/// session that no longer matches.
fn reload_dump(state: &mut State, index: usize, handle: DumpHandle, fingerprint: Option<u64>) {
    let (time_range, range_from_body) = handle.time_range();
    let waveform = handle.waveform();
    let mut rows = (*state.dumps.items[index].rows.items).clone();
    let mut dropped = 0usize;
    rows.retain_mut(|row| {
        let Some(signal) = waveform.signal_by_path(&row.path) else {
            dropped += 1;
            return false;
        };
        match row.bit {
            // an expanded bit row survives while the bus still has that bit
            Some(bit) if signal.width() > u32::from(bit) => {
                row.width = 1;
                true
            }
            Some(_) => {
                dropped += 1;
                false
            }
            None => {
                row.width = signal.width().max(1) as usize;
                true
            }
        }
    });
    // a cursor past the new end of the trace would sit outside the surface:
    // keep it on the last time step instead of losing it off screen
    let mut viewport = state.dumps.items[index].viewport;
    if let Some(range) = time_range {
        let end = i32::try_from(range.end).unwrap_or(i32::MAX);
        viewport.cursors.one = viewport.cursors.one.map(|one| one.min(end));
        viewport.cursors.two = viewport.cursors.two.map(|two| two.min(end));
    }
    let restored = rows.len();
    let entry = &mut state.dumps.items_mut()[index];
    entry.handle = handle;
    entry.time_range = time_range;
    entry.range_from_body = range_from_body;
    entry.fingerprint = fingerprint;
    entry.viewport = viewport;
    entry.session_restore = Some(SessionRestore {
        restored,
        skipped: dropped,
    });
    state.dumps.touch();
    // activate first: `set_rows` works on the active entry, and the tree/top
    // hint have to follow the reloaded hierarchy
    activate_dump(state, index);
    // the row funnel bumps the rows version and records the remembered list, so
    // the session keeps exactly the rows the reloaded dump has
    state.set_rows(rows);
}

/// Tell the design which module the active dump was made from: the best guess
/// for the elaborated top while no source declares one.
fn sync_top_hint(state: &mut State) {
    let top_hint = state
        .dumps
        .active()
        .and_then(|entry| {
            entry
                .handle
                .waveform()
                .hierarchy()
                .root()
                .children()
                .first()
        })
        .map(|scope| scope.name().to_string());
    state.design.set_top_hint(top_hint);
}

/// Rebuild the sidebar tree the store hands to the sidebar.
///
/// The active dump's scope hierarchy wins; the elaborated design is the fallback
/// so the module list is usable before a dump is loaded. The module selection
/// belongs to the old tree, so it is cleared (the sidebar then lists the top
/// scope's signals, exactly as before this slice moved into the store).
fn refresh_tree(state: &mut State) {
    state.tree = match state.dumps.active() {
        Some(entry) => Some(SidebarTree::from_hierarchy(
            entry.handle.waveform().hierarchy(),
        )),
        None => state.design.sidebar_tree(),
    };
    match state.dumps.active_mut() {
        Some(entry) => entry.focus.instance = None,
        None => state.focus.instance = None,
    }
    // the bulk selection belongs to the old tree
    state.nav.tree_selection = crate::state::types::TreeSelection::default();
}

/// The rows a sources-only session already had on the wave, moved into the first
/// dump's entry (the old single-dump behaviour: opening the first dump kept the
/// row list).
fn take_scratch_rows(state: &mut State) -> Vec<Row> {
    let rows = (*state.rows.items).clone();
    state.rows.items = Rc::new(Vec::new());
    state.rows.version = state.rows.version.wrapping_add(1);
    rows
}

/// The remembered row list of `id`, if the session store has one.
///
/// A remembered row whose signal is not in the dump any more (or whose bit does
/// not fit the bus it belongs to) is skipped and counted in [`SessionRestore`];
/// the open itself still succeeds. Returns `None` when this dump has no session.
fn remembered_rows(
    state: &State,
    id: &DumpId,
    handle: &DumpHandle,
) -> (Option<Vec<Row>>, Option<SessionRestore>) {
    let Some(session) = state.sessions.get(id).cloned() else {
        return (None, None);
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
            setting: state.row_prefs.setting_for(&stored.path),
            chosen: false,
            load: stored.load.clone(),
            drive: stored.drive.clone(),
            bit: stored.bit,
        });
    }
    let restored = items.len();
    (Some(items), Some(SessionRestore { restored, skipped }))
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
        Cursors, DumpHandle, DumpId, Layout, Settings, Status, StatusTone, View, EDITOR_RATIO_MIN,
        SIDEBAR_MIN,
    };
    use wave_insight_lib::waveform::ShowType;

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

    /// A handle for raw dump bytes.
    fn handle_of(bytes: Vec<u8>) -> DumpHandle {
        DumpHandle::new(
            wave_insight_lib::waveform::Waveform::open_bytes(bytes).expect("the dump opens"),
        )
    }

    /// `wave.vcd` with one `pc` value changed: the re-simulated dump the reload
    /// rule exists for. Same file name, same byte size, different waveform.
    fn wave_vcd_changed() -> (DumpHandle, u64) {
        let mut bytes = include_bytes!("../../../qa/fixtures/wave.vcd").to_vec();
        let marker = b"b00000001 $";
        let at = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("the fixture has a pc value");
        bytes[at..at + marker.len()].copy_from_slice(b"b10101010 $");
        let size = bytes.len() as u64;
        (handle_of(bytes), size)
    }

    /// A minimal synthetic VCD padded with blank lines to `size` bytes, so two
    /// variants (different end time, with/without `pc`) can share the identity
    /// (name + size) that the reload rule is keyed by.
    fn padded_vcd(end: u32, with_pc: bool, size: usize) -> (DumpHandle, u64) {
        let mut text =
            String::from("$timescale 1ns $end\n$scope module top $end\n  $var wire 1 ! clk $end\n");
        if with_pc {
            text.push_str("  $var wire 8 # pc [7:0] $end\n");
        }
        text.push_str("$upscope $end\n$enddefinitions $end\n$dumpvars\n0!\n");
        if with_pc {
            text.push_str("b0 #\n");
        }
        text.push_str("$end\n");
        text.push_str(&format!("#{end}\n1!\n"));
        let mut bytes = text.into_bytes();
        assert!(bytes.len() <= size, "the padding size is large enough");
        bytes.resize(size, b'\n');
        let size = bytes.len() as u64;
        (handle_of(bytes), size)
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
            .rows_items()
            .iter()
            .map(|row| row.path.clone())
            .collect()
    }

    /// Add one dump as a tab (the loader path in the app).
    fn open_dump(
        state: &mut State,
        handle: DumpHandle,
        name: Option<&str>,
        bytes: Option<u64>,
    ) -> Vec<Effect> {
        open_dump_fp(state, handle, name, bytes, None)
    }

    /// [`open_dump`] with the content fingerprint the loader reports: this is
    /// what tells a re-open of the same bytes from a re-written file with the
    /// same name and size.
    fn open_dump_fp(
        state: &mut State,
        handle: DumpHandle,
        name: Option<&str>,
        bytes: Option<u64>,
        fingerprint: Option<u64>,
    ) -> Vec<Effect> {
        reduce(
            state,
            Action::DumpAdded {
                id: DumpId::new(name.unwrap_or_default(), bytes),
                handle,
                fingerprint,
            },
        )
    }

    /// The active dump entry.
    fn dump(state: &State) -> &crate::state::dumps::DumpEntry {
        state.dumps.active().expect("a dump is open")
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
            vec![
                Effect::LspOpen {
                    file: "top.sv".to_string(),
                    text: "// top.sv\n".to_string()
                },
                // the source was parsed: the server is told which module to
                // elaborate (none yet, the buffer is a comment)
                Effect::LspTop { top: None },
            ]
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
        assert!(matches!(effects.first(), Some(Effect::LspOpen { .. })));
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
            vec![
                Effect::LspChange {
                    file: "top.sv".to_string(),
                    text: "// edited\n".to_string()
                },
                Effect::LspTop { top: None },
            ]
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
                    instance: None,
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

    // --- R2 step 3b: the design slice, focus.instance, editor reloads ------

    #[test]
    fn a_parsed_source_lands_in_the_design_slice_and_rebuilds_the_tree() {
        let mut state = state();
        let effects = reduce(
            &mut state,
            Action::SourceOpened {
                file: "top.sv".to_string(),
                text: "module top; wire clk; endmodule\n".to_string(),
            },
        );
        // the store parsed the source itself: the design knows it and the tree
        // was derived from it in the same dispatch
        assert_eq!(state.design.version(), 1);
        assert!(!state.design.is_empty());
        assert!(state.tree.is_some());
        assert!(state.tree.as_ref().is_some_and(|tree| {
            !tree.root().children.is_empty() || !tree.root().signals.is_empty()
        }));
        // and the language server is told (open + the elaborated top)
        assert!(effects
            .iter()
            .any(|effect| matches!(effect, Effect::LspOpen { .. })));
        assert!(effects
            .iter()
            .any(|effect| matches!(effect, Effect::LspTop { .. })));
    }

    #[test]
    fn an_editor_edit_re_syncs_the_document_without_a_duplicate_open() {
        let mut state = state();
        reduce(
            &mut state,
            Action::SourceOpened {
                file: "top.sv".to_string(),
                text: "module top; endmodule\n".to_string(),
            },
        );
        // the same text again (the editor re-attaching a model) is a no-op
        let effects = reduce(
            &mut state,
            Action::SourceReloaded {
                file: "top.sv".to_string(),
                text: "module top; endmodule\n".to_string(),
            },
        );
        assert!(effects.is_empty());
        assert_eq!(state.sources.items[0].version, 1);
        // a real edit bumps the document and asks for a didChange
        let effects = reduce(
            &mut state,
            Action::SourceReloaded {
                file: "top.sv".to_string(),
                text: "module top; wire a; endmodule\n".to_string(),
            },
        );
        assert_eq!(
            effects
                .iter()
                .filter(|effect| matches!(effect, Effect::LspChange { .. }))
                .count(),
            1
        );
        assert_eq!(state.sources.items[0].version, 2);
        assert_eq!(
            state.sources.items[0].text,
            "module top; wire a; endmodule\n"
        );
        // the design was re-elaborated from the new buffer (the store owns it)
        assert_eq!(state.design.version(), 2);
    }

    #[test]
    fn the_module_selection_lives_in_focus_and_a_new_tree_clears_it() {
        let mut state = state();
        reduce(
            &mut state,
            Action::FocusChanged {
                focus: crate::state::types::Focus {
                    signal: Some("top.clk".to_string()),
                    instance: Some("top.u_cpu".to_string()),
                },
            },
        );
        assert_eq!(state.focus.instance.as_deref(), Some("top.u_cpu"));
        // the wave selection is a separate field and is untouched
        assert_eq!(state.focus.signal.as_deref(), Some("top.clk"));
        // a source parse rebuilds the tree: the old module selection is gone
        reduce(
            &mut state,
            Action::SourceOpened {
                file: "top.sv".to_string(),
                text: "module top; endmodule\n".to_string(),
            },
        );
        assert_eq!(state.focus.instance, None);
        assert_eq!(state.focus.signal.as_deref(), Some("top.clk"));
    }
    #[test]
    fn the_picker_is_store_state_and_the_choice_closes_it() {
        let mut state = state();
        reduce(
            &mut state,
            Action::PickerOpened {
                title: "2 signals match `pc`".to_string(),
                paths: vec!["top.pc".to_string(), "top.u_cpu.pc".to_string()],
            },
        );
        let picker = state.nav.picker.clone().expect("the picker is open");
        assert_eq!(picker.paths.len(), 2);
        // an out-of-range choice is ignored, the picker stays open
        reduce(&mut state, Action::PickerChosen { index: 9 });
        assert!(state.nav.picker.is_some());
        // a real choice closes it (the app resolves the path itself)
        reduce(&mut state, Action::PickerChosen { index: 0 });
        assert!(state.nav.picker.is_none());
        // and an empty picker is not opened at all
        reduce(
            &mut state,
            Action::PickerOpened {
                title: "none".to_string(),
                paths: Vec::new(),
            },
        );
        assert!(state.nav.picker.is_none());
    }

    #[test]
    fn the_settings_dialog_is_store_state() {
        let mut state = state();
        assert!(state.nav.dialog.is_none());
        reduce(
            &mut state,
            Action::DialogOpened {
                indices: vec![0, 2],
            },
        );
        assert_eq!(
            state
                .nav
                .dialog
                .as_ref()
                .map(|dialog| dialog.indices.clone()),
            Some(vec![0, 2])
        );
        // an empty target set is not a dialog
        reduce(
            &mut state,
            Action::DialogOpened {
                indices: Vec::new(),
            },
        );
        assert_eq!(
            state
                .nav
                .dialog
                .as_ref()
                .map(|dialog| dialog.indices.clone()),
            Some(vec![0, 2])
        );
        reduce(&mut state, Action::DialogClosed);
        assert!(state.nav.dialog.is_none());
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
                split_ratio: 0.9,
            }),
        );
        assert_eq!(state.layout.sidebar_w, 4000.0_f64.max(SIDEBAR_MIN));
        assert_eq!(state.layout.editor_ratio, EDITOR_RATIO_MIN);
        assert!(!state.layout.drawer_open);
        // the split divider is clamped to its own documented range
        assert_eq!(
            state.layout.split_ratio,
            crate::state::types::SPLIT_RATIO_MAX
        );
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
        setting.show_type = wave_insight_lib::waveform::ShowType::Bin;
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
        assert!(state.dumps.is_empty());
        open_dump(
            &mut state,
            handle.clone(),
            Some("io_mode_wave.fst"),
            Some(4096),
        );
        assert_eq!(state.dumps.len(), 1);
        assert_eq!(dump(&state).time_range, expected);
        // the file the user picked is part of the dump entry (exports use it)
        assert_eq!(dump(&state).id.name, "io_mode_wave.fst");
        // the handle is the identity: same file, same handle
        assert_eq!(dump(&state).handle, handle);
        // another handle for another file is *not* equal (pointer identity)
        let other = wave_insight_lib::waveform::Waveform::open_bytes(
            include_bytes!("../../../wave-insight-lib/tests/data/io_mode_wave.fst").to_vec(),
        )
        .expect("the fst fixture opens");
        assert_ne!(dump(&state).handle, DumpHandle::new(other));
    }

    #[test]
    fn reopening_the_same_dump_restores_its_rows_in_order() {
        let (handle, size) = wave_vcd();
        let mut state = state();
        open_dump(&mut state, handle, Some("wave.vcd"), Some(size));
        // first open: nothing is remembered yet, so nothing is restored
        assert!(state.rows_items().is_empty());
        assert_eq!(dump(&state).session_restore, None);

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
        open_dump(&mut reopened, handle, Some("wave.vcd"), Some(size));
        assert_eq!(paths(&reopened), vec!["top.pc", "top.clk"]);
        // the width comes from the dump, the format from the remembered settings
        assert_eq!(reopened.rows_items()[0].width, 8);
        assert_eq!(reopened.rows_items()[0].setting, setting);
        assert!(!reopened.rows_items()[0].chosen);
        assert_eq!(
            dump(&reopened).session_restore,
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
        open_dump(&mut state, handle, Some("wave.vcd"), Some(size));
        let bits: Vec<Option<u8>> = state.rows_items().iter().map(|row| row.bit).collect();
        assert_eq!(bits, vec![None, Some(7)]);
        assert_eq!(
            dump(&state).session_restore,
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
        // same name, different size: a different file, so it gets its own tab
        // and the remembered list is not this dump's
        open_dump(&mut state, handle, Some("wave.vcd"), Some(size + 1));
        assert!(state.rows_items().is_empty());
        assert_eq!(dump(&state).session_restore, None);

        // a dump that arrived as raw bytes has no name, so it has no identity:
        // nothing is restored and nothing is remembered for it
        let (handle, _) = wave_vcd();
        open_dump(&mut state, handle, None, None);
        assert!(state.rows_items().is_empty());
        assert_eq!(state.dumps.len(), 2);
        add(&mut state, "top.pc");
        assert_eq!(state.sessions.entries().len(), 1);
    }

    // --- Wave 8: multiple dumps -------------------------------------------

    #[test]
    fn a_second_dump_gets_its_own_tab_and_rows() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("wave.vcd"), Some(size));
        add(&mut state, "top.pc");
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("wave2.vcd"), Some(size));
        // the new tab is active and starts with no rows (nothing remembered)
        assert_eq!(state.dumps.len(), 2);
        assert_eq!(state.dumps.active_index(), 1);
        assert!(state.rows_items().is_empty());
        add(&mut state, "top.clk");
        assert_eq!(paths(&state), vec!["top.clk"]);
        // switching back restores the first tab's own rows
        reduce(&mut state, Action::DumpTabActivated { index: 0 });
        assert_eq!(state.dumps.active_index(), 0);
        assert_eq!(paths(&state), vec!["top.pc"]);
        // and the selectors read the active entry
        assert_eq!(crate::state::select::rows(&state).items[0].path, "top.pc");
        assert_eq!(
            crate::state::select::dump_name(&state).as_deref(),
            Some("wave.vcd")
        );
    }

    #[test]
    fn opening_the_same_identity_focuses_the_existing_tab() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        let first = handle.clone();
        open_dump(&mut state, handle, Some("wave.vcd"), Some(size));
        add(&mut state, "top.pc");
        let (handle, size) = wave_vcd();
        let second = handle.clone();
        open_dump(&mut state, handle, Some("wave.vcd"), Some(size));
        // dedupe by identity: one tab, still active
        assert_eq!(state.dumps.len(), 1);
        assert_eq!(state.dumps.active_index(), 0);
        // no fingerprints (a transport that reports none): the old dedupe rule,
        // the open handle and the rows are untouched
        assert_eq!(dump(&state).handle, first);
        assert_ne!(dump(&state).handle, second);
        assert_eq!(paths(&state), vec!["top.pc"]);
    }

    #[test]
    fn reopening_the_same_bytes_keeps_the_handle_and_the_rows() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        let first = handle.clone();
        open_dump_fp(&mut state, handle, Some("wave.vcd"), Some(size), Some(7));
        add(&mut state, "top.pc");
        let (handle, _) = wave_vcd();
        let second = handle.clone();
        open_dump_fp(&mut state, handle, Some("wave.vcd"), Some(size), Some(7));
        // the same content is the same dump: focus the tab, keep the handle
        assert_eq!(state.dumps.len(), 1);
        assert_eq!(dump(&state).handle, first);
        assert_ne!(dump(&state).handle, second);
        // and the rows are not restored a second time on top of themselves
        assert_eq!(paths(&state), vec!["top.pc"]);
    }

    #[test]
    fn a_rewritten_dump_with_the_same_identity_reloads_in_place() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        open_dump_fp(&mut state, handle, Some("wave.vcd"), Some(size), Some(1));
        add(&mut state, "top.pc");
        add(&mut state, "top.clk");
        // the user zoomed/panned and set both cursors before re-simulating
        reduce(&mut state, Action::ViewCommitted(View::new(120.0, 2.0)));
        reduce(
            &mut state,
            Action::CursorsChanged(Cursors {
                one: Some(200),
                two: Some(300),
            }),
        );

        let (changed, changed_size) = wave_vcd_changed();
        assert_eq!(changed_size, size, "same name and same byte size");
        open_dump_fp(
            &mut state,
            changed.clone(),
            Some("wave.vcd"),
            Some(size),
            Some(2),
        );

        // one tab, now showing the new bytes, with the rows/settings/viewport kept
        assert_eq!(state.dumps.len(), 1);
        assert_eq!(state.dumps.active_index(), 0);
        assert_eq!(dump(&state).handle, changed);
        assert_eq!(dump(&state).fingerprint, Some(2));
        assert_eq!(paths(&state), vec!["top.pc", "top.clk"]);
        assert_eq!(dump(&state).viewport.view, View::new(120.0, 2.0));
        assert_eq!(dump(&state).viewport.cursors.one, Some(200));
        assert_eq!(dump(&state).viewport.cursors.two, Some(300));
        // the rows were re-resolved against the new dump (same widths: no drop)
        assert_eq!(
            dump(&state).session_restore,
            Some(SessionRestore {
                restored: 2,
                skipped: 0
            })
        );
        // the remembered list is the reloaded dump's, and it is what a later
        // re-open restores
        let remembered = state
            .sessions
            .get(&DumpId::new("wave.vcd", Some(size)))
            .expect("the reload recorded the session");
        assert_eq!(remembered.rows.len(), 2);
    }

    #[test]
    fn a_reload_drops_rows_the_new_dump_no_longer_has_and_clamps_cursors() {
        let mut state = state();
        let (handle, size) = padded_vcd(1000, true, 512);
        open_dump_fp(&mut state, handle, Some("s.vcd"), Some(size), Some(1));
        add(&mut state, "top.pc");
        add(&mut state, "top.clk");
        reduce(
            &mut state,
            Action::CursorsChanged(Cursors {
                one: Some(900),
                two: None,
            }),
        );
        // the re-simulated dump is shorter and no longer has pc
        let (changed, changed_size) = padded_vcd(10, false, 512);
        assert_eq!(changed_size, size);
        open_dump_fp(&mut state, changed, Some("s.vcd"), Some(size), Some(2));
        assert_eq!(state.dumps.len(), 1);
        // the row whose signal is gone is dropped and reported; the survivor
        // keeps its place
        assert_eq!(paths(&state), vec!["top.clk"]);
        assert_eq!(
            dump(&state).session_restore,
            Some(SessionRestore {
                restored: 1,
                skipped: 1
            })
        );
        // a cursor past the new end of the trace is kept on the last step
        assert_eq!(dump(&state).viewport.cursors.one, Some(10));
    }

    #[test]
    fn closing_a_tab_activates_its_neighbour_and_the_last_one_empties_the_app() {
        let mut state = state();
        for name in ["a.vcd", "b.vcd", "c.vcd"] {
            let (handle, size) = wave_vcd();
            open_dump(&mut state, handle, Some(name), Some(size));
        }
        assert_eq!(state.dumps.active_index(), 2);
        // closing the active last tab activates the one before it
        reduce(&mut state, Action::DumpClosed { index: 2 });
        assert_eq!(state.dumps.len(), 2);
        assert_eq!(state.dumps.active_index(), 1);
        // closing a tab before the active one shifts the active index left
        reduce(&mut state, Action::DumpTabActivated { index: 0 });
        reduce(&mut state, Action::DumpClosed { index: 1 });
        assert_eq!(state.dumps.len(), 1);
        assert_eq!(state.dumps.active_index(), 0);
        // the last close returns to the empty state
        reduce(&mut state, Action::DumpClosed { index: 0 });
        assert!(state.dumps.is_empty());
        assert!(crate::state::select::dump_waveform(&state).is_none());
        assert!(crate::state::select::time_range(&state).is_none());
        // an out-of-range close is a no-op
        assert!(reduce(&mut state, Action::DumpClosed { index: 3 }).is_empty());
    }

    #[test]
    fn the_memory_budget_refuses_a_dump_that_does_not_fit() {
        let mut state = state();
        let (handle, _) = wave_vcd();
        open_dump(
            &mut state,
            handle,
            Some("big.vcd"),
            Some(super::DUMP_BUDGET_BYTES),
        );
        assert_eq!(state.dumps.len(), 1);
        let (handle, _) = wave_vcd();
        let effects = open_dump(&mut state, handle, Some("second.vcd"), Some(1));
        // refused: no tab, no effect, an actionable error
        assert!(effects.is_empty());
        assert_eq!(state.dumps.len(), 1);
        let status = state.nav.status.as_ref().expect("an error is reported");
        assert!(status.is_error());
        assert!(status.text.contains("close a dump tab"), "{}", status.text);
    }

    #[test]
    fn the_remembered_rows_are_restored_per_tab() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        // a previous session remembered rows for the second file only
        state.sessions.record(
            DumpId::new("b.vcd", Some(size)),
            vec![session_row("top.pc", None)],
        );
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        assert!(state.rows_items().is_empty());
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("b.vcd"), Some(size));
        // the restored tab is active with its remembered row
        assert_eq!(state.dumps.active_index(), 1);
        assert_eq!(paths(&state), vec!["top.pc"]);
        assert_eq!(
            dump(&state).session_restore,
            Some(SessionRestore {
                restored: 1,
                skipped: 0
            })
        );
        // and its row changes are recorded under its own identity
        add(&mut state, "top.clk");
        let remembered = state
            .sessions
            .get(&DumpId::new("b.vcd", Some(size)))
            .expect("the session is recorded");
        assert_eq!(remembered.rows.len(), 2);
        // the first tab still has no remembered rows
        assert!(state
            .sessions
            .get(&DumpId::new("a.vcd", Some(size)))
            .is_none());
    }

    #[test]
    fn switching_dumps_changes_the_rows_slice_version() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        add(&mut state, "top.pc");
        let first = crate::state::select::rows(&state);
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("b.vcd"), Some(size));
        add(&mut state, "top.clk");
        let second = crate::state::select::rows(&state);
        assert_ne!(first, second);
        // the subscription compares only the version, so a tab switch must look
        // different even when both tabs happen to hold one row
        reduce(&mut state, Action::DumpTabActivated { index: 0 });
        let switched = crate::state::select::rows(&state);
        assert_ne!(switched, second);
        assert_eq!(switched.items[0].path, "top.pc");
    }
    #[test]
    fn a_viewport_or_cursor_commit_does_not_change_the_dump_tabs_slice() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("b.vcd"), Some(size));
        let tabs = crate::state::select::dump_tabs(&state);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active_index(), 1);
        // the shell subscribes to the tab slice only: a gesture commit changes the
        // active entry, never the tab list (the tier-A render-count gate)
        reduce(
            &mut state,
            Action::ViewCommitted(crate::state::types::View::new(120.0, 4.0)),
        );
        reduce(
            &mut state,
            Action::CursorsChanged(crate::state::types::Cursors {
                one: Some(10),
                two: Some(20),
            }),
        );
        add(&mut state, "top.pc");
        reduce(
            &mut state,
            Action::FocusChanged {
                focus: crate::state::types::Focus {
                    signal: Some("top.pc".to_string()),
                    instance: None,
                },
            },
        );
        assert_eq!(tabs, crate::state::select::dump_tabs(&state));
        // tab changes do move it
        reduce(&mut state, Action::DumpTabActivated { index: 0 });
        let switched = crate::state::select::dump_tabs(&state);
        assert_ne!(tabs, switched);
        assert_eq!(switched.active_index(), 0);
        assert_eq!(switched.items[0].name, "a.vcd");
        reduce(&mut state, Action::DumpClosed { index: 0 });
        assert_ne!(switched, crate::state::select::dump_tabs(&state));
        // and the single remaining tab keeps its close path available (the strip
        // renders for one dump too)
        let single = crate::state::select::dump_tabs(&state);
        assert_eq!(single.len(), 1);
    }
    // --- Wave 10: the side-by-side split ----------------------------------

    #[test]
    fn the_split_needs_two_dumps_and_maps_its_right_index() {
        let mut state = state();
        // no dumps: opening is a no-op
        reduce(&mut state, Action::SplitOpened { right: 0 });
        assert!(state.split.is_none());
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        // one dump: still no split
        reduce(&mut state, Action::SplitOpened { right: 0 });
        assert!(state.split.is_none());
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("b.vcd"), Some(size));
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("c.vcd"), Some(size));
        // the right pane must be a dump other than the active one
        reduce(&mut state, Action::SplitOpened { right: 2 });
        let split = state.split.expect("the split opened");
        assert_ne!(split.right, state.dumps.active_index());
        assert!(split.linked);
        // an explicit choice is honoured when it is another dump
        reduce(&mut state, Action::SplitRightChanged { right: 0 });
        assert_eq!(state.split.map(|split| split.right), Some(0));
        // ...and refused (a different dump is picked) when it is the active one
        reduce(&mut state, Action::SplitRightChanged { right: 2 });
        let split = state.split.expect("the split stays open");
        assert_ne!(split.right, state.dumps.active_index());
        // the linked toggle flips the flag and keeps the right dump
        let right = split.right;
        reduce(&mut state, Action::SplitLinkedToggled);
        assert_eq!(
            state.split,
            Some(crate::state::Split {
                right,
                linked: false
            })
        );
        reduce(&mut state, Action::SplitClosed);
        assert!(state.split.is_none());
    }

    #[test]
    fn the_split_survives_tab_switches_and_closes_with_its_dump() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("b.vcd"), Some(size));
        reduce(&mut state, Action::SplitOpened { right: 0 });
        assert_eq!(state.split.map(|split| split.right), Some(0));
        // activating the dump shown on the right swaps the panes instead of
        // letting both sides show the same dump
        reduce(&mut state, Action::DumpTabActivated { index: 0 });
        assert_eq!(state.dumps.active_index(), 0);
        assert_eq!(state.split.map(|split| split.right), Some(1));
        // closing the right dump remaps the split to the other dump
        reduce(&mut state, Action::DumpClosed { index: 1 });
        assert_eq!(state.dumps.len(), 1);
        assert!(state.split.is_none(), "one dump cannot be split");
        // and a split opened before a close is dropped when only one dump is left
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("c.vcd"), Some(size));
        reduce(&mut state, Action::SplitOpened { right: 0 });
        assert!(state.split.is_some());
        reduce(&mut state, Action::DumpClosed { index: 1 });
        assert!(state.split.is_none());
    }
    #[test]
    fn toggling_linked_keeps_each_side_s_own_viewport() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        reduce(
            &mut state,
            Action::ViewCommitted(crate::state::types::View::new(50.0, 2.0)),
        );
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("b.vcd"), Some(size));
        reduce(&mut state, Action::SplitOpened { right: 0 });
        reduce(
            &mut state,
            Action::ViewCommittedFor {
                dump: 0,
                view: crate::state::types::View::new(120.0, 4.0),
            },
        );
        // linking is a display decision: neither side's committed window changes
        reduce(&mut state, Action::SplitLinkedToggled);
        reduce(&mut state, Action::SplitLinkedToggled);
        assert_eq!(state.dumps.items[0].viewport.view.x_axis, 120.0);
        assert_eq!(state.dumps.items[1].viewport.view.x_axis, 0.0);
        assert_eq!(
            state.layout.split_ratio,
            crate::state::types::SPLIT_RATIO_DEFAULT
        );
    }
    #[test]
    fn a_viewport_commit_for_one_dump_leaves_the_active_one_alone() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("b.vcd"), Some(size));
        assert_eq!(state.dumps.active_index(), 1);
        // the split pane commits its own dump's viewport by index
        reduce(
            &mut state,
            Action::ViewCommittedFor {
                dump: 0,
                view: crate::state::types::View::new(120.0, 4.0),
            },
        );
        assert_eq!(state.dumps.items[0].viewport.view.x_axis, 120.0);
        // the active dump is untouched, and so is the selector the panes read
        assert_eq!(state.dumps.items[1].viewport.view.x_axis, 0.0);
        assert_eq!(crate::state::select::viewport(&state).view.x_axis, 0.0);
        // an out-of-range dump index is a no-op
        reduce(
            &mut state,
            Action::ViewCommittedFor {
                dump: 9,
                view: crate::state::types::View::new(1.0, 1.0),
            },
        );
        assert_eq!(state.dumps.items[0].viewport.view.x_axis, 120.0);
    }
    #[test]
    fn the_viewport_and_focus_are_per_dump() {
        let mut state = state();
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        reduce(
            &mut state,
            Action::ViewCommitted(crate::state::types::View::new(120.0, 4.0)),
        );
        reduce(
            &mut state,
            Action::FocusChanged {
                focus: crate::state::types::Focus {
                    signal: Some("top.pc".to_string()),
                    instance: None,
                },
            },
        );
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("b.vcd"), Some(size));
        // the new tab has the default viewport and no selection
        assert_eq!(crate::state::select::viewport(&state).view.x_axis, 0.0);
        assert_eq!(crate::state::select::focus(&state).signal, None);
        // switching back restores the first tab's viewport and selection
        reduce(&mut state, Action::DumpTabActivated { index: 0 });
        assert_eq!(crate::state::select::viewport(&state).view.x_axis, 120.0);
        assert_eq!(
            crate::state::select::focus(&state).signal.as_deref(),
            Some("top.pc")
        );
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

    #[test]
    fn tree_selection_toggles_ranges_and_resets() {
        let mut state = state();
        reduce(
            &mut state,
            Action::TreeSelectionToggled {
                path: "top".to_string(),
            },
        );
        reduce(
            &mut state,
            Action::TreeSelectionToggled {
                path: "top.u_cpu".to_string(),
            },
        );
        assert_eq!(
            state.nav.tree_selection.paths,
            vec!["top".to_string(), "top.u_cpu".to_string()]
        );
        assert_eq!(
            state.nav.tree_selection.anchor.as_deref(),
            Some("top.u_cpu")
        );
        // toggling again removes it; the anchor is the row just clicked
        reduce(
            &mut state,
            Action::TreeSelectionToggled {
                path: "top".to_string(),
            },
        );
        assert_eq!(
            state.nav.tree_selection.paths,
            vec!["top.u_cpu".to_string()]
        );
        assert_eq!(state.nav.tree_selection.anchor.as_deref(), Some("top"));
        // Shift+click: the sidebar hands the visible range over
        reduce(
            &mut state,
            Action::TreeSelectionRanged {
                paths: vec!["top".to_string(), "top.u_cpu".to_string()],
            },
        );
        assert_eq!(
            state.nav.tree_selection.paths,
            vec!["top".to_string(), "top.u_cpu".to_string()]
        );
        // a plain click resets to one row and becomes the new anchor
        reduce(
            &mut state,
            Action::TreeSelectionReset {
                path: "top.g[0]".to_string(),
            },
        );
        assert_eq!(state.nav.tree_selection.paths, vec!["top.g[0]".to_string()]);
        assert_eq!(state.nav.tree_selection.anchor.as_deref(), Some("top.g[0]"));
        // and the selection can be cleared
        reduce(&mut state, Action::TreeSelectionCleared);
        assert!(state.nav.tree_selection.is_empty());
        assert_eq!(state.nav.tree_selection.anchor, None);
    }

    #[test]
    fn bulk_add_dedupes_applies_settings_and_bumps_the_rows_version_once() {
        let mut state = state();
        add(&mut state, "top.clk");
        // a remembered display setting must come back on the batch rows
        reduce(
            &mut state,
            Action::RowSettingChanged {
                indices: vec![0],
                setting: Settings { ..Settings::new() },
            },
        );
        let version = state.rows.version;
        let rows = vec![
            Row {
                path: "top.clk".to_string(),
                name: "top.clk".to_string(),
                width: 8,
                setting: Settings::new(),
                chosen: false,
                load: vec![],
                drive: vec![],
                bit: None,
            },
            Row {
                path: "top.rst".to_string(),
                name: "top.rst".to_string(),
                width: 1,
                setting: Settings::new(),
                chosen: false,
                load: vec![],
                drive: vec![],
                bit: None,
            },
            Row {
                path: "top.pc".to_string(),
                name: "top.pc".to_string(),
                width: 8,
                setting: Settings::new(),
                chosen: false,
                load: vec![],
                drive: vec![],
                bit: None,
            },
        ];
        reduce(&mut state, Action::RowsAdded { rows });
        assert_eq!(paths(&state), vec!["top.clk", "top.rst", "top.pc"]);
        // one rows-version bump for the whole batch
        assert_eq!(state.rows.version, version + 1);
        // the duplicate is reported in the status line, not added twice
        let status = state.nav.status.as_ref().expect("a status line");
        assert!(status.text.contains("added 2 signals"), "{}", status.text);
        assert!(
            status.text.contains("1 already in the wave"),
            "{}",
            status.text
        );
    }

    #[test]
    fn bulk_add_refuses_more_than_the_cap_and_keeps_the_rows() {
        let mut state = state();
        let rows: Vec<Row> = (0..crate::state::bulk::BULK_ADD_CAP + 1)
            .map(|index| Row {
                path: format!("top.signal{index}"),
                name: format!("top.signal{index}"),
                width: 1,
                setting: Settings::new(),
                chosen: false,
                load: vec![],
                drive: vec![],
                bit: None,
            })
            .collect();
        let version = state.rows.version;
        reduce(&mut state, Action::RowsAdded { rows });
        assert!(state.rows_items().is_empty());
        assert_eq!(state.rows.version, version);
        let status = state.nav.status.as_ref().expect("a status line");
        assert_eq!(status.tone, StatusTone::Error);
        assert!(status.text.contains("too many signals"), "{}", status.text);
    }

    #[test]
    fn bulk_add_remembers_the_session_once() {
        let mut state = state();
        let rows = vec![
            Row {
                path: "top.clk".to_string(),
                name: "top.clk".to_string(),
                width: 1,
                setting: Settings::new(),
                chosen: false,
                load: vec![],
                drive: vec![],
                bit: None,
            },
            Row {
                path: "top.rst".to_string(),
                name: "top.rst".to_string(),
                width: 1,
                setting: Settings::new(),
                chosen: false,
                load: vec![],
                drive: vec![],
                bit: None,
            },
        ];
        reduce(&mut state, Action::RowsAdded { rows });
        // the session remembers the whole batch (it is the rows memory the
        // persister writes), not just the last signal
        let remembered = state.rows_items();
        assert_eq!(remembered.len(), 2);
        assert_eq!(remembered[0].path, "top.clk");
        assert_eq!(remembered[1].path, "top.rst");
    }

    #[test]
    fn a_new_tree_drops_the_bulk_selection() {
        let mut state = state();
        reduce(
            &mut state,
            Action::TreeSelectionToggled {
                path: "top".to_string(),
            },
        );
        assert!(!state.nav.tree_selection.is_empty());
        let (handle, size) = wave_vcd();
        open_dump(&mut state, handle, Some("a.vcd"), Some(size));
        // the tree was rebuilt for the dump: the old selection is gone
        assert!(state.nav.tree_selection.is_empty());
    }
}
