//the benchmarks need the unstable `test` crate, so keep the default build on
//stable and only enable it with `cargo +nightly bench --features nightly-bench`
#![cfg_attr(feature = "nightly-bench", feature(test))]

pub mod data_struct;
pub mod design;
pub mod lsp;
pub mod parser;
pub mod waveform;

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}

pub fn hello_string() -> String {
    ("hello from lib").to_string()
}
