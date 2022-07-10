use super::get_word::LastType;

#[derive(Debug, PartialEq, Clone)]
pub enum ParserType {
    ModuleDefine(String),
    EndModule,
    SignalDefine(String),
    AssignLeft(String),
    AssignRight(String),
    EndAssign,
    SubModuleDefine(String),
    SubModuleUse(String),
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum State {
    OutOfModule,
    FindingModule,
    ModuleBody,
    SignalDefine,
    Initial,
    AssignLeft,
    AssignRight,
    PrepareAlways,
    IforElse,
    AlwaysLeft,
    AlwaysRight,
    SubModuleName,
    SubModuleBody,
}
pub fn state_update(word: (String,LastType), state: (State,i32)) -> ((State,i32),Option<ParserType>) {
    match (&word.0[..],word.1,state.0,state.1) {
        ("module",_,State::OutOfModule,_) => {((State::FindingModule,state.1),None)},
        (module_name,_,State::FindingModule,_) => {((State::ModuleBody,state.1),Some(ParserType::ModuleDefine(module_name.to_string())))},
        (w,_,State::ModuleBody,_) if is_signal_define(w) => {((State::SignalDefine,state.1),None)},
        (_,LastType::Bracket,State::SignalDefine,_) => {((State::SignalDefine,state.1),None)},//Here can give out signal size
        (signal_name,_,State::SignalDefine,_) => {((State::ModuleBody,state.1),Some(ParserType::SignalDefine(signal_name.to_string())))},

        ("initial",_,State::ModuleBody,_) => {((State::Initial,state.1),None)},
        ("end",_,State::Initial,_) => {((State::ModuleBody,state.1),None)},

        ("assign",_,State::ModuleBody,_) => {((State::AssignLeft,state.1),None)},
        ("=",_,State::AssignLeft,_) => {((State::AssignRight,state.1),None)},
        ("<=",_,State::AssignLeft,_) => {((State::AssignRight,state.1),None)},//TODO:should raise error?
        (left,LastType::Normal,State::AssignLeft,_) => {((State::AssignLeft,state.1),Some(ParserType::AssignLeft(left.to_string())))},
        (right,LastType::Normal,State::AssignRight,_) => {((State::AssignRight,state.1),Some(ParserType::AssignRight(right.to_string())))},
        (w,_,State::AssignRight,_) if w.ends_with(';') => {((State::ModuleBody,state.1),Some(ParserType::EndAssign))},

        ("always",_,State::ModuleBody,_) => {((State::PrepareAlways,state.1),None)},
        ("begin",_,State::PrepareAlways,_) => {((State::AlwaysLeft,1),None)},
        ("if",_,State::AlwaysLeft,_) => {((State::IforElse,state.1),None)},
        ("else",_,State::AlwaysLeft,_) => {((State::IforElse,state.1),None)},
        ("begin",_,State::IforElse,_) => {((State::AlwaysLeft,state.1+1),None)},
        ("end",_,State::AlwaysLeft,1) => {((State::ModuleBody,0),None)},//TODO:always/if has a simple form that dont have begin/end
        ("end",_,State::AlwaysLeft,_) => {((State::AlwaysLeft,state.1-1),None)},
        (left,LastType::Normal,State::AlwaysLeft,_) => {((State::AlwaysLeft,state.1),Some(ParserType::AssignLeft(left.to_string())))},
        ("=",_,State::AlwaysLeft,_) => {((State::AlwaysRight,state.1),None)},
        ("<=",_,State::AlwaysLeft,_) => {((State::AlwaysRight,state.1),None)},
        (right,LastType::Normal,State::AlwaysRight,_) => {((State::AlwaysRight,state.1),Some(ParserType::AssignRight(right.to_string())))},
        (w,_,State::AlwaysRight,_) if w.ends_with(';') => {((State::AlwaysLeft,state.1),Some(ParserType::EndAssign))},

        ("endmodule",_,State::ModuleBody,_) => {((State::OutOfModule,0),Some(ParserType::EndModule))},

        (define_name,LastType::Normal,State::ModuleBody,_) => {((State::SubModuleName,state.1),Some(ParserType::SubModuleDefine(define_name.to_string())))},
        (module_name,_,State::SubModuleName,_) => {((State::SubModuleBody,state.1),Some(ParserType::SubModuleUse(module_name.to_string())))},
        ("(",_,State::SubModuleBody,_) => {((State::SubModuleBody,state.1+1),None)},
        (w,_,State::SubModuleBody,1) if w.starts_with(')') => {((State::ModuleBody,0),None)},
        (w,_,State::SubModuleBody,_) if w.starts_with(')') => {((State::SubModuleBody,state.1-1),None)},

        _ => {((state.0,state.1),None)},
    }
}
fn is_signal_define(w: &str) -> bool {
    w == "input"
        || w == "output"
        || w == "wire"
        || w == "reg"
}
