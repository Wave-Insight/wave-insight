
use crate::{data_struct::{Module, Signal}, parser::{get_word::{LastType, get_word}, word_parser::{State, state_update, ParserType}, module_verilog::ModuleVerilog}};

pub fn verilog_parser(input: &str, raw_module: Module) -> Module {
    let chars = input.chars();
    let mut word = "".to_string();
    let mut last_type = LastType::Space;
    let mut line_idx = 1;
    let mut state = (State::OutOfModule,0);
    let mut modules: Vec<ModuleVerilog> = Vec::new();
    let mut module = ModuleVerilog::new();
    let mut assignment: (Vec<String>,Vec<String>) = (Vec::new(),Vec::new());
    let mut submodule_define = "".to_string();
    chars.map(|c| {
        let ret = get_word(c,word.to_string(),last_type,line_idx);
        word = ret.1;
        last_type = ret.2;
        line_idx = ret.3;
        ret.0
    })
    .map(|ret| {
        if let Some(x) = ret {
            let (new_state,ret) = state_update(x, state);
            //println!("stage1:{:?}",new_state.0);
            state = new_state;
            ret
        }else {
            None
        }
    })
    .for_each(|ret| {
        if let Some(x) = ret {
            //println!("stage2:{:?}", x)
            match x.0 {
                ParserType::ModuleDefine => {module=ModuleVerilog::new();module.name=x.1;},
                ParserType::EndModule => {modules.push(module.clone());},
                ParserType::SignalDefine => {module.signal.push(x.1)},
                ParserType::AssignLeft => {assignment.0.push(x.1)},
                ParserType::AssignRight => {assignment.1.push(x.1)},
                ParserType::EndAssign => {module.assignment.push(assignment.clone());assignment=(Vec::new(),Vec::new())},
                ParserType::SubModuleDefine => {submodule_define = x.1},
                ParserType::SubModuleUse => {module.sub_module.insert(x.1,submodule_define.clone());},
            }
        }
    });
    combine_module(raw_module, modules)
}

fn which_is_top(modules: &[ModuleVerilog]) -> usize {
    let module_number = modules.len();
    let mut has_father = vec![0;module_number];
    for m in modules {
        let sub_idx = (&m.sub_module).iter()
            .map(|s| {modules
                .iter()
                .enumerate()
                .filter(|(_idx,mo)| mo.name == s.1.clone())
                .map(|(idx,_mo)| idx)});
        sub_idx.for_each(|x| x.for_each(|idx| has_father[idx] = 1));
    }
    has_father.into_iter().enumerate()
        .filter(|(_idx,has)| *has==0)
        .map(|(idx,_x)| idx)
        .next().unwrap()//TODO:only get the first top
}

fn combine_module(raw_module: Module, modules: Vec<ModuleVerilog>) -> Module {
    let top_idx = which_is_top(&modules);
    let raw_top = (&raw_module.sub_module).iter().next().unwrap().1;
    if (&raw_top.sub_module).iter()
        .map(|(name,_module)| (modules[top_idx].sub_module.contains_key(name)))
        .reduce(|a,b| a && b).unwrap_or(false)
    {
        let mut ret = raw_module;
        (&modules[top_idx].signal).iter().for_each(|s| {
            ret.signal.entry(s.to_string()).or_insert(Signal::new());
        });
        //TODO:assignment, and all sub module
        ret
    }else if (raw_top.sub_module.len()==1) &&
        (&raw_top.sub_module.iter().next().unwrap().1.sub_module).iter()//TODO:dangerous to unwrap here
        .map(|(name,_module)| (modules[top_idx].sub_module.contains_key(name)))
        .reduce(|a,b| a && b).unwrap_or(false)
    {
        let mut ret = raw_module;
        let top = (&mut ret.sub_module).iter_mut().next().unwrap().1
                .sub_module.iter_mut().next().unwrap().1;
        insert_signal(&modules[top_idx],top);
        insert_load(&modules[top_idx],top);
        insert_drive(&modules[top_idx],top);
        ret
    }else {
        //raw_module//TODO:

        let mut ret = Module::new();
        (&modules[top_idx].sub_module).iter()
        //(&(&(&raw_module.sub_module).iter().next().unwrap().1.sub_module).iter().next().unwrap().1.sub_module).into_iter()
            .for_each(|s| {ret.sub_module.insert(s.0.to_string(),Module::new());});
        ret
    }

}

fn insert_signal(from: &ModuleVerilog, to: &mut Module) {
    from.signal.iter().for_each(|s| {to.signal.entry(s.to_string()).or_insert(Signal::new());})
}
fn insert_load(from: &ModuleVerilog, to: &mut Module) {
    (from.assignment).iter().for_each(|s| {
        s.0.iter()
            .for_each(|left| {
                if let Some(sig) = to.signal.get_mut(left)
                    { sig.load.extend(s.1.clone()) }
            });
    });
}
fn insert_drive(from: &ModuleVerilog, to: &mut Module) {
    (from.assignment).iter().for_each(|s| {
        s.1.iter()
            .for_each(|left| {
                if let Some(sig) = to.signal.get_mut(left)
                    { sig.drive.extend(s.0.clone()) }
            });
    });
}
