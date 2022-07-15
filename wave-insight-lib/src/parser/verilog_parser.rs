use std::rc::Rc;

use crate::{data_struct::{Module, Signal, CodeLocation}, parser::{get_word::{LastType, get_word}, word_parser::{State, state_update, ParserType}, module_verilog::ModuleVerilog}};

pub fn verilog_parser(input: &str, raw_module: Rc<Module>) -> Module {
    let chars = input.chars();
    //init some states
    let mut state = (State::OutOfModule,0);
    let mut modules: Vec<ModuleVerilog> = Vec::new();
    let mut module = ModuleVerilog::new();
    let mut assignment: (Vec<String>,Vec<String>,u32) = (Vec::new(),Vec::new(),0);
    let mut submodule_define = "".to_string();
    chars.scan((' ',false), |(last_char, jump),c| {
        if *jump {
            if c == '\n' {
                *jump = false;
                Some('\n')
            }else {
                Some(' ')
            }
        }else if *last_char == '/' && c == '/' {
            *jump = true;
            Some(' ')
        }else {
            let temp = *last_char;
            *last_char = c;
            Some(temp)
        }
    })//skip the comment
    .scan(("".to_string(),LastType::Space,1), |(word, last_type, line_idx),c| {
        let ret = get_word(c,word.to_string(),*last_type,*line_idx);
        *word = ret.1;
        *last_type = ret.2;
        *line_idx = ret.3;
        Some((ret.0,ret.3))
    })//split the verilog to words
    .map(|word| {
        word.0.and_then(|x| {
            let (new_state,ret) = state_update(x, word.1, state);
            state = new_state;
            ret
        })
    })//get what these words mean
    .for_each(|ret| {
        if let Some(x) = ret {
            match x {
                ParserType::ModuleDefine(name, _line_idx) => {module=ModuleVerilog::new();module.name=name;},
                ParserType::EndModule => {modules.push(module.clone());},
                ParserType::SignalDefine(name, line_idx) => {module.signal.push((name, line_idx))},
                ParserType::AssignLeft(name, line_idx) => {assignment.0.push(name);assignment.2 = line_idx},
                ParserType::AssignRight(name, _line_idx) => {assignment.1.push(name)},
                ParserType::EndAssign => {module.assignment.push(assignment.clone());assignment=(Vec::new(),Vec::new(),0)},
                ParserType::SubModuleDefine(name, _line_idx) => {submodule_define = name},
                ParserType::SubModuleUse(name, _line_idx) => {module.sub_module.insert(name,submodule_define.clone());},
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

fn combine_module(raw_module: Rc<Module>, modules: Vec<ModuleVerilog>) -> Module {
    let top_idx = which_is_top(&modules);
    let raw_top = (&raw_module.sub_module).iter().next().unwrap().1;
    if (&raw_top.sub_module).iter()// top of raw_module and modules is the same
        .map(|(name,_module)| (modules[top_idx].sub_module.contains_key(name)))
        .reduce(|a,b| a && b).unwrap_or(false)
    {
        let mut ret = (*raw_module).clone();
        recursive_combine_module(&mut ret, &modules, top_idx);
        ret
    }else if (raw_top.sub_module.len()==1) &&//top of modules is 1 level higher than raw_module
        (&raw_top.sub_module.iter().next().unwrap().1.sub_module).iter()//TODO:dangerous to unwrap here
        .map(|(name,_module)| (modules[top_idx].sub_module.contains_key(name)))
        .reduce(|a,b| a && b).unwrap_or(false)
    {
        let mut ret = (*raw_module).clone();
        let top = (&mut ret.sub_module).iter_mut().next().unwrap().1
                .sub_module.iter_mut().next().unwrap().1;
        recursive_combine_module(top, &modules, top_idx);
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

fn recursive_combine_module(raw: &mut Module, verilog: &[ModuleVerilog], now_idx: usize) {
    insert_signal(&verilog[now_idx],raw);
    insert_load(&verilog[now_idx],raw);
    insert_drive(&verilog[now_idx],raw);
    raw.sub_module.iter_mut().for_each(|s| {
        let define_name = verilog[now_idx].sub_module
                                          .iter()
                                          .filter(|(name,_)| **name == *s.0)
                                          .map(|(_,def)| def)
                                          .next();
        let this_idx = define_name.and_then(|def| {verilog.iter()
                              .enumerate()
                              .filter(|(_idx,module)| module.name == *def)
                              .map(|(idx, _module)| idx)
                              .next()
        });//TODO:what if not found
        if let Some(idx) = this_idx {
            recursive_combine_module(s.1, verilog, idx);//TODO:clone?
        }
    });
}

fn insert_signal(from: &ModuleVerilog, to: &mut Module) {
    from.signal.iter().for_each(|s| {to.signal.entry(s.0.to_string()).or_insert(Signal::new()).location_define.line = s.1;})
}
fn insert_load(from: &ModuleVerilog, to: &mut Module) {
    (from.assignment).iter().for_each(|s| {
        s.0.iter()
            .for_each(|left| {
                if let Some(sig) = to.signal.get_mut(left){
                    sig.load.extend(s.1.clone());
                    sig.location_load.push(CodeLocation{ file_name: "".to_string(), line: s.2 });
                }
            });
    });
}
fn insert_drive(from: &ModuleVerilog, to: &mut Module) {
    (from.assignment).iter().for_each(|s| {
        s.1.iter()
            .for_each(|left| {
                if let Some(sig) = to.signal.get_mut(left){
                    sig.drive.extend(s.0.clone());
                    sig.location_drive.push(CodeLocation { file_name: "".to_string(), line: s.2 });
                }
            });
    });
}
