use app::App;

mod app;
mod top_bar;
mod file_load;
mod code_reader;
mod module_struct;
mod wave_show;

fn main() {
    yew::start_app::<App>();
}
