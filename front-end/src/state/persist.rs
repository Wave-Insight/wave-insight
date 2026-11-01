//! Persistence of the window layout and of the per-signal display settings.
//!
//! Two versioned `localStorage` entries survive a reload:
//!
//! * [`LAYOUT_KEY`] — the sidebar width, the editor ratio and the drawer flag;
//! * [`ROWS_KEY`] — the display settings (format, colour, fixed point, analog)
//!   of every signal that ever had a non-default one, keyed by its hierarchical
//!   path and capped to the [`ROW_CAP`] most recently used.
//!
//! The rules that keep this safe:
//!
//! * both keys carry the schema version and the payload repeats it, so a payload
//!   written by a future version (or by something else entirely) is *ignored*
//!   rather than migrated in place; a future version can claim a new key;
//! * every read failure (missing key, denied storage, corrupt JSON, an unknown
//!   enum variant) degrades to "nothing was stored": the app starts with its
//!   defaults instead of panicking;
//! * writes happen only when a *committed* value changed — the store compares
//!   the state against the last value it wrote — so a splitter drag writes once,
//!   on mouseup, not on every pointer move, and a selection change writes
//!   nothing;
//! * the only browser-specific code is [`BrowserStorage`]; everything above it
//!   talks to the [`Storage`] trait, which is why the format, the merge and the
//!   cap are unit tested on the host target.
//!
//! The rows are *not* persisted: which signals are on the wave is a property of
//! the session (and of the dump that happens to be open). Only the display
//! settings are remembered, by path, so adding the same signal next time gives
//! back the format the user chose for it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use super::state::State;
use super::types::{Layout, Settings};

/// `localStorage` key of the window layout.
pub const LAYOUT_KEY: &str = "wave-insight.layout.v1";
/// `localStorage` key of the per-signal display settings.
pub const ROWS_KEY: &str = "wave-insight.rows.v1";
/// Schema version written into both keys (and repeated in each payload).
pub const SCHEMA_VERSION: u32 = 1;
/// How many signals remember their settings; the oldest is dropped first.
pub const ROW_CAP: usize = 2000;

/// The two operations the app needs from a key/value store.
///
/// Implemented by [`BrowserStorage`] over `window.localStorage`; the tests use
/// an in-memory map. Both methods are best effort on purpose: a browser in
/// private mode can deny storage, and that must never take the app down.
pub trait Storage {
    /// The raw value, or `None` when the key is absent or the read failed.
    fn get(&self, key: &str) -> Option<String>;
    /// Store (or overwrite) one value. A failure is not reported: the setting
    /// simply does not survive this session.
    fn set(&self, key: &str, value: &str);
}

/// One remembered signal: its hierarchical path and the display settings the
/// user last chose for it.
#[derive(Clone, Debug, PartialEq)]
pub struct RowPref {
    pub path: String,
    pub setting: Settings,
}

/// The remembered per-signal settings, oldest first.
///
/// A path is remembered only while its setting is *not* the default, and
/// re-recording one moves it to the newest end (so the cap drops the settings
/// the user has not touched for the longest time). The list is small
/// ([ROW_CAP] entries) and only touched when a row is added or its settings
/// change, so the linear lookup costs nothing on the hot path.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowPrefs {
    entries: Vec<RowPref>,
}

impl RowPrefs {
    /// The settings a row at `path` starts with: the remembered one, or the
    /// default for a signal that was never configured.
    pub fn setting_for(&self, path: &str) -> Settings {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.path == path)
            .map(|entry| entry.setting.clone())
            .unwrap_or_default()
    }

    /// Remember `setting` for `path`.
    ///
    /// The default setting is *forgotten* instead of remembered (it would be
    /// reapplied anyway), which also keeps the cap honest: resetting a row frees
    /// its slot.
    pub fn record(&mut self, path: &str, setting: &Settings) {
        if path.is_empty() {
            return;
        }
        if *setting == Settings::new() {
            self.forget(path);
            return;
        }
        if let Some(index) = self.entries.iter().position(|entry| entry.path == path) {
            self.entries.remove(index);
        }
        self.entries.push(RowPref {
            path: path.to_string(),
            setting: setting.clone(),
        });
        if self.entries.len() > ROW_CAP {
            let excess = self.entries.len() - ROW_CAP;
            self.entries.drain(0..excess);
        }
    }

    /// Drop the memory of `path` (a reset to the defaults).
    pub fn forget(&mut self, path: &str) {
        self.entries.retain(|entry| entry.path != path);
    }

    /// The remembered signals, oldest first.
    pub fn entries(&self) -> &[RowPref] {
        &self.entries
    }

    /// Merge a decoded payload: later entries win over earlier ones for the same
    /// path, defaults are dropped and the cap is enforced, so a hand-edited or
    /// older payload cannot break the invariants above.
    fn merge(rows: Vec<RowPref>) -> Self {
        let mut prefs = Self::default();
        for row in rows {
            prefs.record(&row.path, &row.setting);
        }
        prefs
    }
}

// --- the on-disk format ----------------------------------------------------
//
// The payload is a small JSON object per key. It is written by this module and
// read only by this module; anything that does not match the schema exactly
// (wrong version, wrong type, an unknown format name) is discarded as a whole.

#[derive(Deserialize, Serialize)]
struct LayoutFile {
    version: u32,
    sidebar_w: f64,
    editor_ratio: f32,
    drawer_open: bool,
}

#[derive(Deserialize, Serialize)]
struct RowsFile {
    version: u32,
    rows: Vec<RowFile>,
}

#[derive(Deserialize, Serialize)]
struct RowFile {
    path: String,
    setting: Settings,
}

/// Serialize the layout; the values are clamped first so a bug elsewhere can
/// never write a non-finite number (which JSON cannot represent either).
pub fn encode_layout(layout: &Layout) -> String {
    let layout = layout.clamped();
    serde_json::to_string(&LayoutFile {
        version: SCHEMA_VERSION,
        sidebar_w: layout.sidebar_w,
        editor_ratio: layout.editor_ratio,
        drawer_open: layout.drawer_open,
    })
    .unwrap_or_default()
}

/// Parse a stored layout. `None` for anything this version does not understand
/// (including a newer schema); the caller falls back to the defaults.
pub fn decode_layout(raw: &str) -> Option<Layout> {
    let file: LayoutFile = serde_json::from_str(raw).ok()?;
    if file.version != SCHEMA_VERSION {
        return None;
    }
    Some(
        Layout {
            sidebar_w: file.sidebar_w,
            editor_ratio: file.editor_ratio,
            drawer_open: file.drawer_open,
        }
        .clamped(),
    )
}

/// Serialize the remembered row settings (already capped by [`RowPrefs`]).
pub fn encode_rows(prefs: &RowPrefs) -> String {
    let rows: Vec<RowFile> = prefs
        .entries()
        .iter()
        .map(|entry| RowFile {
            path: entry.path.clone(),
            setting: entry.setting.clone(),
        })
        .collect();
    serde_json::to_string(&RowsFile {
        version: SCHEMA_VERSION,
        rows,
    })
    .unwrap_or_default()
}

/// Parse stored row settings; `None` when the payload is not ours.
pub fn decode_rows(raw: &str) -> Option<RowPrefs> {
    let file: RowsFile = serde_json::from_str(raw).ok()?;
    if file.version != SCHEMA_VERSION {
        return None;
    }
    Some(RowPrefs::merge(
        file.rows
            .into_iter()
            .map(|row| RowPref {
                path: row.path,
                setting: row.setting,
            })
            .collect(),
    ))
}

// --- the store's side ------------------------------------------------------

/// Reads the persisted values once and writes them back when they change.
///
/// The store owns one of these (or none, when the browser denies storage) and
/// calls [`Persister::sync`] after every dispatch; comparing against the last
/// written value is the throttle: a drag that is committed once writes once.
pub struct Persister {
    storage: Box<dyn Storage>,
    saved_layout: Cell<Layout>,
    saved_rows: RefCell<RowPrefs>,
}

impl Persister {
    /// Read what the last session stored. Anything unreadable is ignored.
    pub fn load(storage: Box<dyn Storage>) -> Self {
        let layout = storage
            .get(LAYOUT_KEY)
            .as_deref()
            .and_then(decode_layout)
            .unwrap_or_default();
        let rows = storage
            .get(ROWS_KEY)
            .as_deref()
            .and_then(decode_rows)
            .unwrap_or_default();
        Self {
            storage,
            saved_layout: Cell::new(layout),
            saved_rows: RefCell::new(rows),
        }
    }

    /// The layout the last session ended with (already clamped by the decoder).
    pub fn restored_layout(&self) -> Layout {
        self.saved_layout.get()
    }

    /// The per-signal settings the last session ended with.
    pub fn restored_rows(&self) -> RowPrefs {
        self.saved_rows.borrow().clone()
    }

    /// Write the slices that changed since the last write.
    pub fn sync(&self, state: &State) {
        if state.layout != self.saved_layout.get() {
            self.storage.set(LAYOUT_KEY, &encode_layout(&state.layout));
            self.saved_layout.set(state.layout);
        }
        let rows_changed = state.row_prefs != *self.saved_rows.borrow();
        if rows_changed {
            self.storage.set(ROWS_KEY, &encode_rows(&state.row_prefs));
            *self.saved_rows.borrow_mut() = state.row_prefs.clone();
        }
    }
}

// --- the browser side ------------------------------------------------------

/// The real `localStorage` behind the [`Storage`] trait.
struct BrowserStorage(web_sys::Storage);

impl Storage for BrowserStorage {
    fn get(&self, key: &str) -> Option<String> {
        // a browser that revoked access returns an error here, not a value
        self.0.get_item(key).ok().flatten()
    }

    fn set(&self, key: &str, value: &str) {
        // a full quota (or a denied store) is not fatal: the value just does not
        // survive this session
        let _ = self.0.set_item(key, value);
    }
}

/// The app's persister over the browser's `localStorage`.
///
/// `None` when the window has no usable storage (private mode denies it, and
/// the host target has no window at all): the app then runs with the defaults
/// and never panics.
pub fn browser_persister() -> Option<Rc<Persister>> {
    let storage = web_sys::window()?.local_storage().ok()??;
    Some(Rc::new(Persister::load(Box::new(BrowserStorage(storage)))))
}

// --- the fake used by the host tests ---------------------------------------

/// In-memory [`Storage`] for the host tests; `Clone` so the test can keep one
/// handle while the persister owns another.
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct MemoryStorage {
    values: Rc<RefCell<std::collections::HashMap<String, String>>>,
    writes: Rc<Cell<u32>>,
}

#[cfg(test)]
impl MemoryStorage {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn preload(&self, key: &str, value: &str) {
        self.values
            .borrow_mut()
            .insert(key.to_string(), value.to_string());
    }

    pub(crate) fn raw(&self, key: &str) -> Option<String> {
        self.values.borrow().get(key).cloned()
    }

    /// How many values were written; the throttle assertions count this.
    pub(crate) fn writes(&self) -> u32 {
        self.writes.get()
    }
}

#[cfg(test)]
impl Storage for MemoryStorage {
    fn get(&self, key: &str) -> Option<String> {
        self.raw(key)
    }

    fn set(&self, key: &str, value: &str) {
        self.writes.set(self.writes.get() + 1);
        self.preload(key, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::types::{EDITOR_RATIO_MAX, SIDEBAR_MIN};
    use wave_insight_lib::data_struct::ShowType;

    /// A non-default setting whose colour/format depends on `seed`.
    fn setting(seed: u8) -> Settings {
        let mut setting = Settings::new();
        setting.show_type = if seed.is_multiple_of(2) {
            ShowType::Bin
        } else {
            ShowType::SInt
        };
        setting.color = (seed, seed.wrapping_add(1), seed.wrapping_add(2));
        setting.fixed_active = seed.is_multiple_of(3);
        setting.fixed_point = u32::from(seed % 16);
        setting.analog_active = seed.is_multiple_of(5);
        setting
    }

    #[test]
    fn layout_round_trips_and_is_clamped_on_the_way_in() {
        let layout = Layout {
            sidebar_w: 321.0,
            editor_ratio: 0.6,
            drawer_open: false,
        };
        assert_eq!(decode_layout(&encode_layout(&layout)), Some(layout));

        // every decoded value is brought back into the documented range
        let raw = r#"{"version":1,"sidebar_w":10.0,"editor_ratio":9.0,"drawer_open":true}"#;
        assert_eq!(
            decode_layout(raw),
            Some(Layout {
                sidebar_w: SIDEBAR_MIN,
                editor_ratio: EDITOR_RATIO_MAX,
                drawer_open: true,
            })
        );
    }

    #[test]
    fn corrupt_or_foreign_layout_is_ignored() {
        let cases = [
            "",
            "not json",
            "{",
            "null",
            "[]",
            // a version we do not know: a newer app may write a different shape
            r#"{"version":2,"sidebar_w":300.0,"editor_ratio":0.5,"drawer_open":true}"#,
            // the version is required, a partial payload is not guessed at
            r#"{"version":1,"sidebar_w":300.0}"#,
            r#"{"sidebar_w":300.0,"editor_ratio":0.5,"drawer_open":true}"#,
            // wrong types, including JSON null where a number is required
            r#"{"version":1,"sidebar_w":"wide","editor_ratio":0.5,"drawer_open":true}"#,
            r#"{"version":1,"sidebar_w":null,"editor_ratio":0.5,"drawer_open":true}"#,
        ];
        for raw in cases {
            assert_eq!(decode_layout(raw), None, "should ignore {raw:?}");
        }
    }

    #[test]
    fn unknown_layout_keys_are_ignored() {
        // a future minor version may add fields; they must not break this one
        let raw = r#"{"version":1,"sidebar_w":300.0,"editor_ratio":0.5,"drawer_open":true,"dock":"left","nested":{"x":1}}"#;
        assert_eq!(
            decode_layout(raw),
            Some(Layout {
                sidebar_w: 300.0,
                editor_ratio: 0.5,
                drawer_open: true,
            })
        );
    }

    #[test]
    fn row_settings_round_trip() {
        let mut prefs = RowPrefs::default();
        for (index, path) in ["top.clk", "top.pc", "top.data"].iter().enumerate() {
            prefs.record(path, &setting(index as u8));
        }
        let encoded = encode_rows(&prefs);
        assert_eq!(decode_rows(&encoded), Some(prefs));
        // the encoded payload names the schema version, so a future format can
        // be recognised instead of guessed at
        assert!(encoded.contains(r#""version":1"#));
    }

    #[test]
    fn corrupt_or_foreign_rows_are_ignored() {
        let cases = [
            "",
            "not json",
            "{}",
            // a newer schema
            r#"{"version":2,"rows":[]}"#,
            // a format name this build does not know
            r#"{"version":1,"rows":[{"path":"a","setting":{"show_type":"Nope","color":[1,2,3],"fixed_active":false,"fixed_point":0,"analog_active":false}}]}"#,
            // a missing setting field
            r#"{"version":1,"rows":[{"path":"a","setting":{"show_type":"Hex","color":[1,2,3]}}]}"#,
        ];
        for raw in cases {
            assert_eq!(decode_rows(raw), None, "should ignore {raw:?}");
        }
    }

    #[test]
    fn a_default_setting_is_not_remembered() {
        let mut prefs = RowPrefs::default();
        let setting = setting(2);
        prefs.record("top.pc", &setting);
        assert_eq!(prefs.setting_for("top.pc"), setting);
        // resetting a row to the defaults forgets it (the default would be
        // applied anyway) and frees its slot in the cap
        prefs.record("top.pc", &Settings::new());
        assert!(prefs.entries().is_empty());
        assert_eq!(prefs.setting_for("top.pc"), Settings::new());
        // an empty path is not a signal
        prefs.record("", &setting);
        assert!(prefs.entries().is_empty());
    }

    #[test]
    fn the_row_cap_drops_the_oldest_and_keeps_the_newest() {
        let mut prefs = RowPrefs::default();
        for index in 0..ROW_CAP + 10 {
            prefs.record(&format!("top.s{index}"), &setting((index % 32) as u8));
        }
        let newest = format!("top.s{}", ROW_CAP + 9);
        let entries = prefs.entries();
        assert_eq!(entries.len(), ROW_CAP);
        assert_eq!(entries[0].path, "top.s10");
        assert_eq!(
            entries.last().map(|entry| entry.path.as_str()),
            Some(newest.as_str())
        );

        // touching an old signal makes it the newest instead of duplicating it
        prefs.record("top.s10", &setting(1));
        let entries = prefs.entries();
        assert_eq!(entries.len(), ROW_CAP);
        assert_eq!(entries[0].path, "top.s11");
        assert_eq!(
            entries.last().map(|entry| entry.path.as_str()),
            Some("top.s10")
        );

        // a hand-edited/older payload is merged through the same rules
        let rows: Vec<RowPref> = (0..ROW_CAP + 5)
            .map(|index| RowPref {
                path: format!("top.t{index}"),
                setting: setting(3),
            })
            .collect();
        let merged = RowPrefs::merge(rows);
        assert_eq!(merged.entries().len(), ROW_CAP);
        assert_eq!(merged.entries()[0].path, "top.t5");
    }

    #[test]
    fn the_persister_restores_and_writes_only_changes() {
        let storage = MemoryStorage::new();
        let persister = Persister::load(Box::new(storage.clone()));
        assert_eq!(persister.restored_layout(), Layout::default());
        assert!(persister.restored_rows().entries().is_empty());

        let mut state = State {
            layout: Layout {
                sidebar_w: 333.0,
                editor_ratio: 0.55,
                drawer_open: false,
            },
            ..State::default()
        };
        persister.sync(&state);
        assert_eq!(storage.writes(), 1, "only the layout changed");
        // nothing changed since the last sync: no write at all (the throttle)
        persister.sync(&state);
        assert_eq!(storage.writes(), 1);

        let setting = setting(7);
        state.row_prefs.record("top.pc", &setting);
        persister.sync(&state);
        assert_eq!(storage.writes(), 2);

        // a reload reads exactly what the last session ended with
        let reloaded = Persister::load(Box::new(storage.clone()));
        assert_eq!(reloaded.restored_layout(), state.layout);
        assert_eq!(reloaded.restored_rows(), state.row_prefs);
        assert_eq!(reloaded.restored_rows().setting_for("top.pc"), setting);
    }

    #[test]
    fn denied_or_corrupt_storage_degrades_to_the_defaults() {
        struct Denied;
        impl Storage for Denied {
            fn get(&self, _key: &str) -> Option<String> {
                None
            }
            fn set(&self, _key: &str, _value: &str) {}
        }

        let persister = Persister::load(Box::new(Denied));
        assert_eq!(persister.restored_layout(), Layout::default());
        assert!(persister.restored_rows().entries().is_empty());
        // writing through a store that silently drops everything must not panic
        let mut state = State::default();
        state.layout.sidebar_w = 400.0;
        state.row_prefs.record("top.pc", &setting(1));
        persister.sync(&state);

        // the same for values that are there but unusable
        let storage = MemoryStorage::new();
        storage.preload(LAYOUT_KEY, "{broken");
        storage.preload(ROWS_KEY, r#"{"version":1,"rows":"nope"}"#);
        let persister = Persister::load(Box::new(storage));
        assert_eq!(persister.restored_layout(), Layout::default());
        assert!(persister.restored_rows().entries().is_empty());
    }
}
