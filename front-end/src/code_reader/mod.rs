pub use code::CodeReader;

mod code;
mod code_block;
/// The editor wrapper and the document/model plumbing that the language
/// providers in `crate::lsp::provider` share.
pub(crate) mod monaco_glue;
