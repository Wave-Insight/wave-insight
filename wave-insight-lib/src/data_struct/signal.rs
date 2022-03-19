use super::CodeLocation;

//no need of name because name is key
#[derive(Debug, PartialEq, Clone)]
pub struct Signal {
    pub size: usize,
    pub value_change: Vec<(i32,i32)>,
    pub same_value_signal: Option<(Vec<String>,String)>,//(module_name,signal_name)
    pub location_define: CodeLocation,//TODO:need file name
    pub location_drive: Vec<CodeLocation>,
    pub location_load: Vec<CodeLocation>,
}