//! The sidebar: the module tree and the signal list of the selected scope.
//!
//! Both lists are virtualized for big dumps: [`virtual_list`] holds the windowing
//! math (host tested), [`seam`] publishes the logical lists for the QA matrix,
//! and [`ModuleStruct`] / [`SignalStruct`] draw the visible window.
//!
//! Lists up to [`virtual_list::VIRTUALIZE_ABOVE`] rows are drawn whole, so the
//! small fixtures the QA matrix drives keep exactly the DOM they had before.
pub use module::{ModuleStruct, RowContext, RowKind, TreeSelect};
pub use signal_struct::SignalStruct;

mod module;
pub mod seam;
mod signal_struct;
pub mod virtual_list;
