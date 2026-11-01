//Yew 0.19 expands `html!` into code that trips `unnecessary_operation` on every
//prop (`value={expr}` becomes a bare `expr;` statement in the expansion). The
//lint cannot tell that apart from a real mistake, so it is allowed for the whole
//crate rather than sprinkled over ~68 macro sites. Everything else stays on.
#![allow(clippy::unnecessary_operation)]

use app::App;

mod app;
mod code_reader;
mod file_load;
mod lsp;
mod module_struct;
mod nav;
mod state;
mod top_bar;
mod wave_show;

fn main() {
    yew::start_app::<App>();
}
