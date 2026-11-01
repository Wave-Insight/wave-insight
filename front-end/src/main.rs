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
    // Yew 0.20 removed `start_app` in favour of an explicit renderer; `render()`
    // mounts to the document body, exactly like `start_app` did, and keeps the
    // panic hook. `csr` (Cargo.toml) is what makes the renderer available.
    yew::Renderer::<App>::new().render();
}
