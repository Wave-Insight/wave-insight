//! Turning the store into the [`AgentSnapshot`] the MCP tools read.
//!
//! The snapshot is the model's map of the window: which dump is displayed, its
//! time range, what is visible right now, where the cursors are and which
//! signals are already on the wave. It is built from the same selectors the
//! panes use, so it can never disagree with what the user sees.
//!
//! Two dump kinds exist in the desktop build and both are reported:
//!
//! * the **native** dump the large-file mode draws over IPC (one at a time),
//! * the **wasm** dumps, one per tab, whose handle lives in the store.
//!
//! The native one wins when it is open, exactly like the wave pane.

use wave_insight_lib::agent::{AgentCursors, AgentDump, AgentSnapshot, AgentTimeRange};

use crate::state::{self, State};

/// The most rows one snapshot lists (a 2000-row wave would otherwise flood the
/// model's context; `rows_truncated` says the list was cut).
const MAX_SNAPSHOT_ROWS: usize = 200;

/// Build the snapshot of one state.
///
/// This is pure so the wasm tab branch is host-testable; the app passes the
/// native session separately because it lives in `App`, not in the store.
pub fn snapshot(
    state: &State,
    native: Option<&NativeDumpView>,
    surfaces: &Surfaces,
) -> AgentSnapshot {
    let width = if state.nav.wave_width > 0.0 {
        state.nav.wave_width
    } else {
        0.0
    };
    let fallback_width = 800.0;
    let effective_width = if width > 0.0 { width } else { fallback_width };
    let viewport = state.viewport_slice();
    let visible = window_of(viewport.view, effective_width);
    let rows = unique_rows(state);
    let rows_truncated = rows.len() > MAX_SNAPSHOT_ROWS;
    let mut rows = rows;
    rows.truncate(MAX_SNAPSHOT_ROWS);

    let mut dumps = Vec::new();
    for entry in state.dumps.items.iter() {
        let waveform = entry.handle.waveform();
        dumps.push(AgentDump {
            name: entry.id.name.clone(),
            path: surfaces.path_of(&entry.id),
            format: waveform.file_format().name().to_string(),
            timescale: waveform.timescale().map(|timescale| timescale.to_string()),
            time_range: entry.time_range.map(to_range),
            visible,
            cursors: to_cursors(viewport.cursors),
            rows: rows.clone(),
            rows_truncated,
            signal_count: Some(waveform.signal_count()),
            queryable: surfaces.path_of(&entry.id).is_some(),
            native: false,
        });
    }
    let active = match native {
        Some(native) => {
            dumps.push(AgentDump {
                name: native.name.to_string(),
                path: Some(native.path.to_string()),
                format: native.format.to_string(),
                timescale: native.timescale.map(str::to_string),
                time_range: native
                    .range
                    .map(|(start, end)| AgentTimeRange { start, end }),
                visible,
                cursors: to_cursors(viewport.cursors),
                rows: rows.clone(),
                rows_truncated,
                signal_count: Some(native.signal_count),
                queryable: true,
                native: true,
            });
            Some(dumps.len() - 1)
        }
        None if !dumps.is_empty() => Some(state.dumps.active_index()),
        None => None,
    };
    AgentSnapshot {
        active,
        dumps,
        native: native.is_some(),
        surface_width: width,
        visible,
        sources: state
            .sources
            .items
            .iter()
            .map(|source| source.name.clone())
            .collect(),
    }
}

/// What the app knows but the store does not: the native dump and the absolute
/// path behind each wasm tab.
#[derive(Default)]
pub struct Surfaces {
    /// `(DumpId, absolute path)` for the tabs the transport reported a path for.
    pub tab_paths: Vec<(crate::state::DumpId, String)>,
}

impl Surfaces {
    pub fn path_of(&self, id: &crate::state::DumpId) -> Option<String> {
        self.tab_paths
            .iter()
            .find(|(tab, _)| tab == id)
            .map(|(_, path)| path.clone())
    }
}

/// The parts of the native session the snapshot needs (kept as a plain struct so
/// this module stays testable and free of the tauri-gated remote types).
pub struct NativeDumpView<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub format: &'a str,
    pub timescale: Option<&'a str>,
    pub range: Option<(u64, u64)>,
    pub signal_count: usize,
}

fn to_range(range: wave_insight_lib::waveform::TimeRange) -> AgentTimeRange {
    AgentTimeRange {
        start: range.start,
        end: range.end,
    }
}

fn to_cursors(cursors: crate::state::Cursors) -> AgentCursors {
    AgentCursors {
        one: cursors.one,
        two: cursors.two,
    }
}

fn window_of(view: state::View, width: f64) -> Option<AgentTimeRange> {
    if !view.x_axis.is_finite() || !view.size.is_finite() || view.size <= 0.0 {
        return None;
    }
    let (start, end) = view.window(width);
    Some(AgentTimeRange { start, end })
}

/// The wave rows, deduplicated by path and in row order.
fn unique_rows(state: &State) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for row in state.rows_items() {
        if row.path.is_empty() || rows.iter().any(|existing| existing == &row.path) {
            continue;
        }
        rows.push(row.path);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{reduce, Action, DumpHandle, DumpId, View};

    fn wave() -> DumpHandle {
        let bytes = include_bytes!("../../../qa/fixtures/wave.vcd").to_vec();
        DumpHandle::new(
            wave_insight_lib::waveform::Waveform::open_bytes(bytes).expect("fixture opens"),
        )
    }

    fn opened() -> State {
        let mut state = State::default();
        reduce(
            &mut state,
            Action::DumpAdded {
                id: DumpId::new("wave.vcd", Some(4096)),
                handle: wave(),
                fingerprint: None,
            },
        );
        state
    }

    #[test]
    fn the_snapshot_reports_the_visible_window_from_the_view() {
        let mut state = opened();
        reduce(&mut state, Action::WaveSurfaceWidth(500.0));
        reduce(
            &mut state,
            Action::ViewCommitted(View::from_time_range(100, 200, 500.0)),
        );
        let surfaces = Surfaces::default();
        let snapshot = snapshot(&state, None, &surfaces);
        assert_eq!(snapshot.active, Some(0));
        assert_eq!(snapshot.surface_width, 500.0);
        assert_eq!(
            snapshot.visible,
            Some(AgentTimeRange {
                start: 100,
                end: 200
            })
        );
        assert_eq!(snapshot.dumps.len(), 1);
        assert_eq!(snapshot.dumps[0].visible, snapshot.visible);
        // the fixture has no absolute path: the data tools need one
        assert!(!snapshot.dumps[0].queryable);
        assert_eq!(
            snapshot.dumps[0].signal_count,
            Some(wave().waveform().signal_count())
        );
    }

    #[test]
    fn an_unmeasured_surface_falls_back_to_a_usable_width() {
        let state = opened();
        let snapshot = snapshot(&state, None, &Surfaces::default());
        assert_eq!(snapshot.surface_width, 0.0);
        // the default view (x_axis 0, size 1) over 800 px
        assert_eq!(
            snapshot.visible,
            Some(AgentTimeRange { start: 0, end: 800 })
        );
    }

    #[test]
    fn a_native_dump_is_reported_as_the_active_one() {
        let state = opened();
        let native = NativeDumpView {
            name: "big.fst",
            path: "D:\\sim\\big.fst",
            format: "FST",
            timescale: Some("1 ns"),
            range: Some((0, 1_000)),
            signal_count: 42,
        };
        let snapshot = snapshot(&state, Some(&native), &Surfaces::default());
        assert!(snapshot.native);
        let active = snapshot.active.expect("the native dump is active");
        let dump = &snapshot.dumps[active];
        assert_eq!(dump.name, "big.fst");
        assert_eq!(dump.path.as_deref(), Some("D:\\sim\\big.fst"));
        assert!(dump.queryable);
        assert!(dump.native);
        // the wasm tab is still listed, at index 0
        assert_eq!(snapshot.dumps[0].name, "wave.vcd");
    }

    #[test]
    fn rows_are_deduplicated_in_row_order() {
        let mut state = opened();
        for path in ["top.clk", "top.clk", "top.data"] {
            reduce(
                &mut state,
                Action::RowAdded {
                    path: path.into(),
                    name: path.into(),
                    width: 1,
                    load: vec![],
                    drive: vec![],
                    bit: None,
                },
            );
        }
        let snapshot = snapshot(&state, None, &Surfaces::default());
        assert_eq!(snapshot.dumps[0].rows, vec!["top.clk", "top.data"]);
        assert!(!snapshot.dumps[0].rows_truncated);
    }

    #[test]
    fn the_tab_path_makes_a_wasm_dump_queryable() {
        let state = opened();
        let id = state.dumps.items[0].id.clone();
        let surfaces = Surfaces {
            tab_paths: vec![(id, "D:\\sim\\wave.vcd".into())],
        };
        let snapshot = snapshot(&state, None, &surfaces);
        assert!(snapshot.dumps[0].queryable);
        assert_eq!(snapshot.dumps[0].path.as_deref(), Some("D:\\sim\\wave.vcd"));
    }

    #[test]
    fn no_dump_at_all_is_an_empty_snapshot() {
        let snapshot = snapshot(&State::default(), None, &Surfaces::default());
        assert_eq!(snapshot.active, None);
        assert!(snapshot.dumps.is_empty());
        assert!(!snapshot.native);
    }
}
