use serde::{Deserialize, Serialize};

/// How a bit vector is rendered in the wave view and the value table.
///
/// This is a display preference, not part of the waveform backend: the value is
/// the same, only its text changes. It lives with the waveform types (R3) because
/// the library's render API ([\`WaveValue::render\`](super::WaveValue::render))
/// needs it; the viewer re-exports it through the same module.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, Default)]
pub enum ShowType {
    #[default]
    Hex,
    UInt,
    SInt,
    Oct,
    Bin,
    Ascii,
}
