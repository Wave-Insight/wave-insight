use yew::prelude::*;
use web_sys::{Event, HtmlInputElement};
use gloo_file::File;

use gloo_file::callbacks::FileReader;
use web_sys::console;//TODO:for debug

pub enum FileType {
    IsVcd,
    IsVerilog,
}

pub enum Msg {
    Loaded(FileType,String,String),
    Files(Vec<File>),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct FileLoadProps {
    #[prop_or_default]
    pub ongetfile: Callback<(FileType,String,String)>,
}

pub struct FileLoad {
    task: Vec<FileReader>,
}

impl Component for FileLoad {
    type Message = Msg;
    type Properties = FileLoadProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            task: vec![],
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Loaded(file_type, file_name, text) => {
                console::log_1(&format!("parsering {}",(match file_type {FileType::IsVcd=>{"vcd"},FileType::IsVerilog=>{"verilog"},})).into());
                let ongetfile = ctx.props().ongetfile.clone();
                ongetfile.emit((file_type,file_name,text));
                true
            }
            Msg::Files(result) => {
                console::log_1(&format!("input {} file",result.len()).into());
                for file in result.into_iter() {
                    let link = ctx.link().clone();
                    let file_name = file.name();
                    console::log_1(&format!("file name:{}",file_name).into());
                    let file_type = if file_name.ends_with(".vcd") {FileType::IsVcd}
                                            else {FileType::IsVerilog};
                    let task = gloo_file::callbacks::read_as_text(&file, move |res| {
                        link.send_message(Msg::Loaded(
                            file_type,
                            file_name,
                            res.unwrap_or_else(|e| e.to_string()),
                        ))
                    });
                    self.task.push(task);
                }
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        let link = ctx.link();
        let onchangefunc = link.callback(move |e: Event| {
                        let mut result = Vec::new();
                        let input: HtmlInputElement = e.target_unchecked_into();

                        if let Some(files) = input.files() {
                            let files = js_sys::try_iter(&files)
                                .unwrap()
                                .unwrap()
                                .map(|v| web_sys::File::from(v.unwrap()))
                                .map(File::from);
                            result.extend(files);
                        }
                        Msg::Files(result)
                    });
        html! {
            <div style="height:5%">
                <input type="file" multiple=true onchange={onchangefunc}/>
            </div>
        }
    }
}
