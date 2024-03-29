use wave_insight_lib::{data_struct::{Module, ModuleValue}, parser::{verilog_parser::verilog_parser, vcd_parser::vcd_parser}};
use yew::prelude::*;
use web_sys::{Event, HtmlInputElement};
use gloo_file::File;

#[cfg(feature = "backend")]
use crate::file_load::FileList;

use gloo_file::callbacks::FileReader;
use web_sys::console;//TODO:for debug

#[cfg(feature = "wasm")]
use std::rc::Rc;

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "tauri"])]
    async fn invoke(cmd: &str, args: JsValue) -> JsValue;
}

pub enum FileType {
    IsVcd,
    IsVerilog,
}

pub enum Msg {
    #[cfg(feature = "wasm")]
    Loaded(FileType,String,String),
    #[cfg(feature = "wasm")]
    Files(Vec<File>),
    #[cfg(feature = "backend")]
    Get,
    #[cfg(feature = "backend")]
    ExitList,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct FileLoadProps {
    #[cfg(feature = "wasm")]
    pub module: Rc<Module>,
    pub ongetmodule: Callback<Module>,
    #[cfg(feature = "wasm")]
    pub ongetvalue: Callback<ModuleValue>,
    pub ongetverilog: Callback<(String, String)>,
}

pub struct FileLoad {
    #[cfg(feature = "wasm")]
    task: Vec<FileReader>,
    #[cfg(feature = "backend")]
    filelist_show: bool,
}

impl Component for FileLoad {
    type Message = Msg;
    type Properties = FileLoadProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            #[cfg(feature = "wasm")]
            task: vec![],
            #[cfg(feature = "backend")]
            filelist_show: false,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            #[cfg(feature = "wasm")]
            Msg::Loaded(file_type, file_name, text) => {
                console::log_1(&format!("parsering {}",(match file_type {FileType::IsVcd=>{"vcd"},FileType::IsVerilog=>{"verilog"},})).into());

                match file_type {
                    FileType::IsVcd => {
                        let (module, value) = vcd_parser(text,&mut Module::new());
                        ctx.props().ongetmodule.emit(module);
                        ctx.props().ongetvalue.emit(value);
                    },//TODO:module::new()
                    FileType::IsVerilog => {
                        ctx.props().ongetmodule.emit(verilog_parser(&text,Rc::clone(&ctx.props().module)));
                        ctx.props().ongetverilog.emit((file_name,text));
                    },
                }
                console::log_1(&format!("finish parser {}",(match file_type {FileType::IsVcd=>{"vcd"},FileType::IsVerilog=>{"verilog"},})).into());

                true
            }
            #[cfg(feature = "wasm")]
            Msg::Files(result) => {
                console::log_1(&format!("input {} file",result.len()).into());
                for file in result.into_iter() {
                    let link = ctx.link().clone();
                    let file_name = file.name();
                    console::log_1(&format!("file name:{file_name}").into());
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
            #[cfg(feature = "backend")]
            Msg::Get => {
                self.filelist_show = true;
                true
            }
            #[cfg(feature = "backend")]
            Msg::ExitList => {
                self.filelist_show = false;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        let link = ctx.link();
        #[cfg(feature = "wasm")]
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
        #[cfg(feature = "backend")]
        let button_style = "
            background-color:#ffffff;
            border-radius:6px;
            border:1px solid #dcdcdc;
            display:inline-block;
            cursor:pointer;
            color:#666666;font-size:15px;font-weight:bold;
            margin:2% 6.5%;height:20px;width:20%;";
        #[cfg(feature = "wasm")]
        html! {
            <div style="height:5%">
                <input type="file" multiple=true onchange={onchangefunc}/>
            </div>
        }
        #[cfg(feature = "backend")]
        html! {
            <div>
                if self.filelist_show {
                    <FileList
                        onexit={link.callback(|_| Msg::ExitList)}
                        ongetmodule={ctx.props().ongetmodule.clone()}
                        ongetverilog={ctx.props().ongetverilog.clone()}
                    />
                }
                <div style="height:5%">
                    <button type="button" style={button_style} onclick={link.callback(|_| Msg::Get)}>{"File"}</button>
                </div>
            </div>
        }
    }
}
