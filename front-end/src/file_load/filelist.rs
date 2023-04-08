use wave_insight_lib::data_struct::Module;
use yew::prelude::*;
use serde::{Deserialize, Serialize};
use serde_wasm_bindgen::{to_value, from_value};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

//use web_sys::console;//TODO:for debug

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "tauri"])]
    pub async fn invoke(cmd: &str, args: JsValue) -> JsValue;
    #[wasm_bindgen(js_namespace = ["window", "__TAURI__", "event"])]
    pub async fn listen(event: &str, handler: &Closure<dyn FnMut(JsValue)>);
}

pub enum Msg {
    Exit(MouseEvent),
    Back,
    Into(String),
    GetModule(Module),

    SetList(Vec<String>),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct FileListProps {
    pub onexit: Callback<()>,
    pub ongetmodule: Callback<Module>,
    pub ongetverilog: Callback<(String, String)>,
}

pub struct FileList {
    list: Vec<String>,
    choose_list: Vec<String>,
}

impl Component for FileList {
    type Message = Msg;
    type Properties = FileListProps;

    fn create(ctx: &Context<Self>) -> Self {

        ctx.link().callback(Msg::Into).emit("".to_string());

        Self {
            list: vec![],
            choose_list: vec![],
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Exit(_) => {
                ctx.props().onexit.emit(());
                true
            }
            Msg::Back => {
                true
            }
            Msg::Into(s) => {
                if s.strip_suffix(".vcd").is_some() {
                    let mut choose = self.choose_list.clone();
                    choose.push(s);
                    let args = to_value(&GetFileListArgs { name: choose }).unwrap();
                    let link = ctx.props().ongetmodule.clone();//ctx.link().callback(Msg::GetModule);
                    spawn_local(async move {
                        let ret: Module = from_value(invoke("choose_vcd", args).await).unwrap();
                        //console::log_1(&format!("{:?}", ret).into());
                        link.emit(ret);
                    });
                }else if s.strip_suffix(".v").is_some() {
                    let mut choose = self.choose_list.clone();
                    choose.push(s.clone());
                    let args = to_value(&GetFileListArgs { name: choose }).unwrap();
                    let link = ctx.props().ongetmodule.clone();
                    let link_verilog = ctx.props().ongetverilog.clone();
                    spawn_local(async move {
                        let ret: (String, Module) = from_value(invoke("choose_verilog", args).await).unwrap();
                        //console::log_1(&format!("{:?}", ret).into());
                        link.emit(ret.1);
                        link_verilog.emit((s, ret.0));
                    });
                }else {
                    self.choose_list.push(s);
                    let args = to_value(&GetFileListArgs { name: self.choose_list.clone() }).unwrap();
                    let link = ctx.link().callback(Msg::SetList);
                    spawn_local(async move {
                        let ret: Vec<String> = from_value(invoke("get_file_list", args).await).unwrap();
                        //console::log_1(&format!("{:?}", ret).into());
                        link.emit(ret);
                    });
                }
                false
            }
            Msg::GetModule(s) => {
                true
            }
            Msg::SetList(s) => {
                //console::log_1(&format!("{:?}", s).into());
                let mut list = vec!["..".to_string()];
                list.extend(s);
                self.list = list;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let into = |l: String| link.callback(move |_| Msg::Into(l.clone()));
        html! {
            <div>
                <div style="background:rgba(0, 0, 0, 0.6);position:fixed;top:0;left:0;right:0;bottom:0;z-index:101;"
                    onclick={link.callback(Msg::Exit)} >
                </div>
                <div style="position:absolute;top:50%;left:50%;
                            background:#fff;height:70%;width:30%;
                            transform:translate(-50%, -50%);
                            border-radius: 15px;border: 2px solid #2e84f8;
                            z-index:102;">
                    {
                        for self.list.iter().map(|l| {
                            html!{ <h5 onclick={into(l.to_string())}>{l}</h5> }
                        })
                    }

                </div>
            </div>
        }
    }
}


#[derive(Serialize, Deserialize)]
struct GetFileListArgs {
    name: Vec<String>,
}
