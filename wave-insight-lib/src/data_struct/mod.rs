//! The few value types the viewer shares between the backend and the UI.
//!
//! Waveform *values* live in [`crate::waveform`]; what is left here is the
//! description of a signal row ([`Signal`], [`CodeLocation`]) and the display
//! preference ([`ShowType`]).

pub use code_location::CodeLocation;
pub use show_type::ShowType;
pub use signal::Signal;

mod code_location;
mod show_type;
mod signal;
