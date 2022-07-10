use crate::data_struct::{Module, Signal, ValueType};


pub enum ParseAction {
    Module(String,Module),
    EndModule,
    Signal(String,Signal),
    Value(String,ValueType),
    Clk(i32),
}
