use super::CodeLocation;

//no need of name because name is key
#[derive(Debug, PartialEq, Clone)]
pub struct Signal {
    pub size: usize,
    pub value_change: Vec<(i32,i32)>,
    pub same_value_signal: Option<(Vec<String>,String)>,//(module_name,signal_name)
    pub module_path: Vec<String>,//TODO:try to find something instead of String,which looks like hash key
    pub location_define: CodeLocation,//TODO:need file name
    pub location_drive: Vec<CodeLocation>,
    pub location_load: Vec<CodeLocation>,
}

impl Signal {
    pub fn new() -> Self {
        Self {
            size: 0,
            value_change: vec![],
            same_value_signal: None,
            module_path: vec![],
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
