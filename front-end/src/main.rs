use app::App;

// `tauri` and `web-remote` select different transports for the same native
// remote UI path (Tauri IPC vs. the loopback HTTP JS shim). Enabling both would
// pick two `invoke` implementations at once, so refuse it at compile time.
#[cfg(all(feature = "tauri", feature = "web-remote"))]
compile_error!(
    "features `tauri` and `web-remote` are mutually exclusive: build with either --features tauri or --features web-remote"
);

#[cfg(any(feature = "tauri", feature = "web-remote"))]
mod agent;
mod app;
mod code_reader;
mod file_load;
mod lsp;
mod module_struct;
mod nav;
// the desktop-only native dump path: the IPC half is tauri gated, the types are
// always compiled so the wave pane can carry an optional native session
#[allow(dead_code)]
mod remote;
mod state;
mod top_bar;
mod wave_show;

fn main() {
    // Yew 0.20 removed `start_app` in favour of an explicit renderer; `render()`
    // mounts to the document body, exactly like `start_app` did, and keeps the
    // panic hook. `csr` (Cargo.toml) is what makes the renderer available.
    yew::Renderer::<App>::new().render();
}
