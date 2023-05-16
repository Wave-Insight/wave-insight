use crate::data_struct::{Module, Signal};


pub enum ParseAction {
    Module(String,Module),
    EndModule,
    Signal(String,Signal),
    Value(String,Vec<(u8,u8)>),
    ValueSingle(String,(u8,u8)),
    Clk(i32),
}
