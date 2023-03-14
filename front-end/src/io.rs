
pub enum FileMsg {
    Get,
    Back,
    Into(String),
    Choose(String),
}

pub enum IoMsg {
    File(FileMsg),
    
}
