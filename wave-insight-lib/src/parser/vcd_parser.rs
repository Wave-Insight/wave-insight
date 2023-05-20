use std::io::BufRead;

use crate::data_struct::{Module, ModuleValue};

use super::vcd::parse_state::ParseState;
use super::vcd::parsing_line::parsing_line;

pub fn vcd_parser(input: String, raw_module: &mut Module) -> (Module, ModuleValue) {
    let lines = input.lines();
    let mut state = ParseState{clk: 0, module: raw_module.clone(), value: ModuleValue::new(), stack: vec![]};
    lines.for_each(|l| parsing_line(&mut state, l));
    state.module.end_clock = state.clk;
    (state.module, state.value)
}

pub fn vcd_parser_iter(lines: impl Iterator<Item = String>, raw_module: &mut Module) -> (Module, ModuleValue) {
    let mut state = ParseState{clk: 0, module: raw_module.clone(), value: ModuleValue::new(), stack: vec![]};
    lines.for_each(|l| parsing_line(&mut state, &l));
    state.module.end_clock = state.clk;
    (state.module, state.value)
}

pub fn vcd_parser_path(path: std::path::PathBuf, raw_module: &mut Module) -> (Module, ModuleValue) {
    let file = std::fs::File::open(path).unwrap();
    let mut read_line = std::io::BufReader::new(file);
    let mut buf = String::new();
    let mut state = ParseState{clk: 0, module: raw_module.clone(), value: ModuleValue::new(), stack: vec![]};
    while read_line.read_line(&mut buf).unwrap() > 0 {
        parsing_line(&mut state, &buf);
        buf.clear();
    }
    state.module.end_clock = state.clk;
    (state.module, state.value)
}
/*
extern crate test;

#[cfg(test)]
mod bench {
    use std::{str::FromStr, io::{BufRead, Read}};

    use super::*;
    use test::Bencher;

    #[bench]
    fn bench_no_iter(b: &mut Bencher) {
        b.iter(|| {
            let dest_path = std::path::PathBuf::from_str("../test.vcd").unwrap();//TODO:do not unwrap
            let mut file = std::fs::File::open(dest_path).unwrap();
            let mut contents = String::new();
            file.read_to_string(&mut contents).unwrap();
            vcd_parser(contents, &mut Module::new())
        })
    }

    #[bench]
    fn bench_iter(b: &mut Bencher) {
        b.iter(|| {
            let dest_path = std::path::PathBuf::from_str("../test.vcd").unwrap();//TODO:do not unwrap
            let file = std::fs::File::open(dest_path).unwrap();
            let read_line = std::io::BufReader::new(file);
            vcd_parser_iter(read_line.lines().map(|s| s.unwrap()), &mut Module::new())
        })
    }

    #[bench]
    fn bench_iter_2(b: &mut Bencher) {
        b.iter(|| {
            let dest_path = std::path::PathBuf::from_str("../test.vcd").unwrap();//TODO:do not unwrap
            vcd_parser_path(dest_path, &mut Module::new())
        })
    }
}*/
