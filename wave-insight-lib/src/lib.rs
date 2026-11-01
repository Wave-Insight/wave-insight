//the benchmarks need the unstable `test` crate, so keep the default build on
//stable and only enable it with `cargo +nightly bench --features nightly-bench`
#![cfg_attr(feature = "nightly-bench", feature(test))]
//rustdoc has to evaluate the auto traits of sv-parser's recursive syntax tree
//types, which blows the default limit with "overflow evaluating the requirement
//`List<..>: Sync`"
#![recursion_limit = "512"]

pub mod design;
pub mod lsp;
pub mod waveform;
