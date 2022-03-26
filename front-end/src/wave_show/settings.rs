use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Settings {
    pub size: i32,
    pub x_axis: i32,
}
