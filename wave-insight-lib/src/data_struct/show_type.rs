use serde::{Deserialize, Serialize};

/// How a bit vector is rendered in the wave view and the value table.
///
/// This is a display preference, not part of the waveform backend: the value is
/// the same, only its text changes.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum ShowType {
    Hex,
    UInt,
    SInt,
    Oct,
    Bin,
    Ascii,
}

impl Default for ShowType {
    fn default() -> Self {
        ShowType::Hex
    }
}
