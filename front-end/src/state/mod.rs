//! One store for the whole front end: typed state, typed actions, slice
//! subscriptions.
//!
//! Design: `docs/state-store.md` (R2). This module is deliberately UI free — it
//! must not import the panes, the wave renderer or the editor — so the reducer
//! can be unit tested on the host target.
//!
//! R2 is being migrated in four steps; this is step 1:
//!
//! * the skeleton ([`Store`], [`Action`], [`reduce`], [`select`]) is in place and
//!   is the only way to read or change the state,
//! * [`types::Layout`] and [`types::Status`] live in the store (sidebar width,
//!   drawer, editor ratio, the top bar line),
//! * the panes still take their props exactly as before; steps 2-4 move the
//!   rows, the viewport, the documents and the design in.

pub mod action;
pub mod reduce;
pub mod select;
pub mod state;
pub mod store;
pub mod types;

// the store's public surface grows with steps 2-4; these are re-exported now so
// the call sites do not have to move again
#[allow(unused_imports)]
pub use action::{Action, Effect};
#[allow(unused_imports)]
pub use reduce::reduce;
#[allow(unused_imports)]
pub use state::{Nav, Rows, State};
#[allow(unused_imports)]
pub use store::{Store, Subscription};
#[allow(unused_imports)]
pub use types::{Row, RowsSlice, Settings, StatusTone};
pub use types::{Layout, Status};
