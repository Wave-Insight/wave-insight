
use serde::{Deserialize, Serialize};
use super::CodeLocation;

//no need of name because name is key
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Signal {
    pub size: usize,
    pub value_key: String,
    pub drive: Vec<String>,
    pub load: Vec<String>,
    pub location_define: CodeLocation,//TODO:need file name
    pub location_drive: Vec<CodeLocation>,
    pub location_load: Vec<CodeLocation>,
}

impl Signal {
    pub fn new() -> Self {
        Self {
            size: 0,
            value_key: "".to_string(),
            load: vec![],
            drive: vec![],
            location_define: CodeLocation::new(),
            location_drive: vec![],
            location_load: vec![],
        }
    }
}

impl Default for Signal {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for Signal {
    fn eq(&self, other: &Self) -> bool {
        self.value_key == other.value_key
    }
}
