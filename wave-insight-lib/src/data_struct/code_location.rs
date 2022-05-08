use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
pub struct CodeLocation {
    pub file_name: String,//TODO: choose another type
    pub line: u32,
}

impl CodeLocation {
    pub fn new() -> Self {
        Self {
            file_name: "".to_string(),
            line: 0,
        }
    }
}

impl Default for CodeLocation {
    fn default() -> Self {
        Self::new()
    }
}

