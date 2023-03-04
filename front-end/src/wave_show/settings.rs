use serde::{Deserialize, Serialize};
use wave_insight_lib::data_struct::ShowType;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Settings {
    pub show_type: ShowType,
    pub color: (u8,u8,u8),

    pub fixed_active: bool,
    pub fixed_point: u32,

    pub analog_active: bool,
    pub analog_initerpolate: u32,

    pub load_all: bool,
    pub load_one: i32,
    pub drive_all: bool,
    pub drive_one: i32,

}

impl Settings {
    pub fn new() -> Self {
        Self {
            show_type: ShowType::Hex,
            color: (0,255,0),

            fixed_active: false,
            fixed_point: 0,

            analog_active: false,
            analog_initerpolate: 1,

            load_all: false,
            load_one: -1,
            drive_all: false,
            drive_one: -1,
        }
    }
}
