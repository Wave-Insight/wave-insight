use num::BigUint;

use crate::data_struct::{Module, Signal};


pub enum ParseAction {
    Module(String,Module),
    EndModule,
    Signal(String,Signal),
    Value(String,BigUint),
    Clk(i32),
}
