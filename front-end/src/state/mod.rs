//! One store for the whole front end: typed state, typed actions, slice
//! subscriptions.
//!
//! Design: `docs/state-store.md` (R2). This module is deliberately UI free — it
//! must not import the panes, the wave renderer or the editor — so the reducer
//! can be unit tested on the host target.
//!
//! R2 is being migrated in four steps; the store now owns:
//!
//! * the skeleton ([`Store`], [`Action`], [`reduce`], [`select`]) and the only
//!   way to read or change the state,
//! * [`types::Layout`] and [`types::Status`] (step 1),
//! * the wave rows, the committed viewport/cursors (step 2) and the opened dump
//!   handle with its derived time range (step 3, first increment),
//! * step 4 keeps the layout *contract* in `App`: the pixel heights are derived
//!   from the stored ratio and the measured pane, never stored. The source
//!   documents and the design database move in in the next step-3 increment and
//!   still live in `App` today.
//!
//! Wave 3 adds [`persist`]: the layout and the per-signal display settings are
//! restored on startup and written back when they change. The format, the merge
//! and the row cap are pure and host-tested; only the `localStorage` call itself
//! is wasm-only.
//!
//! Wave 4 extends the same module with the remembered *row list per dump*: the
//! reducer restores it in `DumpOpened` and records it on every row change, the
//! store persists it under a versioned key and the app mentions skipped rows in
//! the status line.
//!
//! Wave 5 (R2 step 3) moves the open **sources**, the inspector **focus** and
//! the **jump request** into the store ([`sources`], `State.focus`,
//! `State.nav.goto`), and the language server's document calls become
//! [`Effect`]s executed by `App` (`LspOpen`/`LspChange`/`LspClose`/`LspTop`/
//! `MonacoShow`). The bridge keeps a mirror of the document table so
//! `window.__waveInsightLsp.stats().documents` still counts the open documents.
//!
//! Scope decision for this step: the **design slice**, `focus.instance` and the
//! module-tree/instance selection stay in `App`/`ModuleStruct` until step 3b
//! (`module_struct/**` belongs to another agent this wave); the reducer never
//! reads the elaborated design.

pub mod action;
pub mod persist;
pub mod reduce;
pub mod select;
pub mod sources;
//the module that defines the State struct lives in state/state.rs, the name the design doc uses
#[allow(clippy::module_inception)]
pub mod state;
pub mod store;
pub mod types;

// the store's public surface grows with steps 2-4; these are re-exported now so
// the call sites do not have to move again
#[allow(unused_imports)]
pub use action::{Action, Effect};
#[allow(unused_imports)]
pub use persist::{Persister, RowPrefs, Session, SessionRow, Sessions, Storage};
#[allow(unused_imports)]
pub use reduce::reduce;
#[allow(unused_imports)]
pub use sources::{Source, Sources, SourcesSlice};
#[allow(unused_imports)]
pub use state::{Nav, Rows, State};
#[allow(unused_imports)]
pub use store::{Store, Subscription};
#[allow(unused_imports)]
pub use types::{
    Cursors, Dump, DumpHandle, DumpId, Focus, Goto, Row, RowsSlice, SessionRestore, Settings,
    StatusTone, View, Viewport,
};
pub use types::{Layout, Status};
