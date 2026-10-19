use std::rc::Rc;
use std::cell::RefCell;

use wave_insight_lib::data_struct::{
    Module,
    Signal,
    ModuleValue};

use yew::prelude::*;
use web_sys::console;//TODO:for debug

use crate::code_reader::CodeReader;
use crate::wave_show::WaveShow;
use crate::module_struct::ModuleStruct;

use crate::top_bar::TopBar;

#[cfg(feature = "backend")]
use web_sys::{MessageEvent, WebSocket};
#[cfg(feature = "backend")]
use wasm_bindgen::{prelude::Closure, JsCast};

use crate::file_load::FileLoad;

#[cfg(feature = "tauri")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "tauri")]
use serde_wasm_bindgen::{to_value, from_value};
#[cfg(feature = "tauri")]
use wasm_bindgen::prelude::*;
#[cfg(feature = "tauri")]
use wasm_bindgen_futures::spawn_local;
#[cfg(feature = "tauri")]
use crate::file_load::{invoke, listen};

pub enum Msg {
    #[cfg(feature = "wasm")]
    FileDrop(DragEvent),
    #[cfg(feature = "tauri")]
    FileDrop(Vec<String>),
    #[cfg(feature = "wasm")]
    DragOver(DragEvent),

    NavIconClick,
    SignalAdd((String,Rc<Signal>)),
    GetModule(Module),
    #[cfg(feature = "wasm")]
    GetValue(ModuleValue),
    GetVerilog((String, String)),
    #[cfg(feature = "tauri")]
    GetValueFromTauri(String),
    #[cfg(feature = "tauri")]
    AddValueFromTauri(Option<(String, (Vec<i32>, Vec<(u8, u8)>))>),
    #[cfg(feature = "server")]
    WsSend(String),
    #[cfg(feature = "server")]
    GetWebsocket(String),

    ///the sidebar splitter was grabbed
    SplitterDown(MouseEvent),
    ///the mouse moved while dragging the splitter
    SplitterMove(MouseEvent),
    SplitterUp,
}

///width of the sidebar when nothing else was chosen
const SIDEBAR_DEFAULT: f64 = 280.0;
const SIDEBAR_MIN: f64 = 180.0;

fn sidebar_clamp(width: f64) -> f64 {
    let max = web_sys::window()
        .and_then(|w| w.inner_width().ok())
        .and_then(|v| v.as_f64())
        .map(|w| (w * 0.45).max(SIDEBAR_MIN + 40.0))
        .unwrap_or(640.0);
    width.max(SIDEBAR_MIN).min(max)
}

pub struct App {
    drawer_state: bool,
    module: Rc<Module>,
    signal_value: Rc<RefCell<ModuleValue>>,//do not re-render when change, so RefCell is ok
    #[cfg(feature = "backend")]
    signal_value_raw: ModuleValue,//TODO:not a good implement
    verilog_source: Vec<(String,String)>,
    signal_add: (String,Rc<Signal>),
    #[cfg(feature = "server")]
    websocket: Option<WebSocket>,
    sidebar_width: f64,
    ///(mouse x, sidebar width) while the splitter is dragged
    splitter_start: Option<(f64, f64)>,
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {

        #[cfg(feature = "tauri")]
        let file_drop_callback = ctx.link().callback(Msg::FileDrop);

        #[cfg(feature = "tauri")]
        spawn_local(async move {
            #[derive(Debug, Serialize, Deserialize)]
            struct TauriEvent {
                event: String,
                windowLabel: String,
                payload: Vec<String>,
                id: f64,
            }
            let closure = Closure::<dyn FnMut(JsValue)>::new(move |x: JsValue| {
                let tauri_event: TauriEvent = serde_wasm_bindgen::from_value(x).unwrap();//TODO:do not unwrap
                //console::log_1(&format!("callback: {:?}", tauri_event).into());
                file_drop_callback.emit(tauri_event.payload);
            });
            let ret = listen("tauri://file-drop", &closure).await;
            closure.forget();
        });

        #[cfg(feature = "server")]
        let websocket = create_websocket(ctx);
        Self {
            drawer_state: true,
            module: Rc::new(Module::new()),
            signal_value: Rc::new(RefCell::new(ModuleValue::new())),
            #[cfg(feature = "backend")]
            signal_value_raw: ModuleValue::new(),
            verilog_source: vec![],
            signal_add: ("".to_string(),Rc::new(Signal::new())),
            #[cfg(feature = "server")]
            websocket: Some(websocket),
            sidebar_width: SIDEBAR_DEFAULT,
            splitter_start: None,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            #[cfg(feature = "wasm")]
            Msg::FileDrop(e) => {
                e.prevent_default();
                console::log_1(&"file droped".into());
                console::log_1(&format!("{:?}", e).into());
                e.data_transfer()
                    .and_then(|x| x.files())
                    .and_then(|x| x.get(0))
                    .map(|x| {
                        console::log_1(&format!("{:?}", x.name()).into());
                        console::log_1(&format!("{:?}", x.text()).into());
                    });
                true
            }
            #[cfg(feature = "wasm")]
            Msg::DragOver(e) => {
                e.prevent_default();
                console::log_1(&format!("{:?}", e).into());
                true
            }

            #[cfg(feature = "tauri")]
            Msg::FileDrop(path) => {
                #[derive(Serialize, Deserialize)]
                struct GetFileListArgs {
                    path: String,
                }
                path.into_iter().for_each(|p| {
                    if p.strip_suffix(".vcd").is_some() {
                        let args = to_value(&GetFileListArgs { path: p }).unwrap();
                        let link = ctx.link().callback(Msg::GetModule);
                        spawn_local(async move {
                            let ret: Module = from_value(invoke("choose_vcd_absolute", args).await).unwrap();
                            //console::log_1(&format!("{:?}", ret).into());
                            link.emit(ret);
                        });
                    }else if p.strip_suffix(".v").is_some() {
                        let args = to_value(&GetFileListArgs { path: p.clone() }).unwrap();
                        let link = ctx.link().callback(Msg::GetModule);
                        let link_verilog = ctx.link().callback(Msg::GetVerilog);
                        spawn_local(async move {
                            let ret: (String, Module) = from_value(invoke("choose_verilog_absolute", args).await).unwrap();
                            //console::log_1(&format!("{:?}", ret).into());
                            link.emit(ret.1);
                            link_verilog.emit((p.clone(), ret.0));
                        });
                    };
                });
                true
            }

            Msg::NavIconClick => {
                self.drawer_state = !self.drawer_state;
                true
            }
            Msg::SplitterDown(e) => {
                e.prevent_default();
                if e.detail() == 2 {
                    //double click restores the default width
                    self.sidebar_width = SIDEBAR_DEFAULT;
                    self.splitter_start = None;
                }else {
                    self.splitter_start = Some((e.x() as f64, self.sidebar_width));
                }
                true
            }
            Msg::SplitterMove(e) => {
                if let Some((start_x, start_width)) = self.splitter_start {
                    self.sidebar_width = sidebar_clamp(start_width + (e.x() as f64 - start_x));
                }
                true
            }
            Msg::SplitterUp => {
                self.splitter_start = None;
                true
            }
            Msg::SignalAdd(input) => {
                #[cfg(feature = "server")]
                ctx.link().callback(Msg::WsSend).emit(format!("s:{}",input.1.value_key));
                #[cfg(feature = "tauri")]
                ctx.link().callback(Msg::GetValueFromTauri).emit(input.1.value_key.clone());
                self.signal_add = (input.0, input.1);
                true
            }
            Msg::GetModule(m) => {
                self.module = Rc::new(m);
                true
            }
            #[cfg(feature = "wasm")]
            Msg::GetValue(v) => {
                self.signal_value = Rc::new(v.into());
                true
            }
            Msg::GetVerilog(v) => {
                self.verilog_source.push(v);
                true
            }
            #[cfg(feature = "tauri")]
            Msg::GetValueFromTauri(key) => {
                let args = to_value(&TauriArgs { key }).unwrap();
                let link = ctx.link().callback(Msg::AddValueFromTauri);
                spawn_local(async move {
                    let ret = from_value(invoke("get_value", args).await).unwrap();
                    //console::log_1(&format!("{:?}", ret).into());
                    link.emit(ret);
                });
                false
            }
            #[cfg(feature = "tauri")]
            Msg::AddValueFromTauri(x) => {
                match x {
                    Some((key, value)) => {
                        self.signal_value.borrow_mut().value.insert(key, value);
                    },
                    None => {},
                };
                false
            }
            #[cfg(feature = "server")]
            Msg::WsSend(e) => {
                match &mut self.websocket {
                    Some(ws) => {
                       if ws.send_with_str(&e).is_ok() {};
                    },
                    None => {},
                }
                true
            }
            #[cfg(feature = "server")]
            Msg::GetWebsocket(m) => {
                if let Some(module_string) = m.strip_prefix("module:") {
                    let module: Module = serde_json::from_str(module_string).unwrap();//TODO:do not unwrap
                    self.module = Rc::new(module);
                }else if let Some(signal_string) = m.strip_prefix("sig:") {
                    if let Some((key, value)) = signal_string.split_once(29u8 as char) {
                        let value_parse: (Vec<i32>, Vec<(u8, u8)>) = serde_json::from_str(value).unwrap();//TODO:do not unwrap
                        self.signal_value_raw.value.insert(key.to_string(), value_parse);
                        self.signal_value = Rc::new(self.signal_value_raw.clone());
                        //TODO:value update here will cause an adition signal add on wave show
                    }
                }
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();

        html! {
            <div class="app" style={format!("--sidebar-w:{:.0}px", self.sidebar_width)}>
                <TopBar onnavigationiconclick={link.callback(|_| Msg::NavIconClick)}/>
                <div class="app-body">
                    {if self.drawer_state {
                        html!{
                        <aside class="sidebar">
                            {self.file_button(ctx)}
                            <ModuleStruct module={Rc::clone(&self.module)} signaladd={link.callback(Msg::SignalAdd)}/>
                        </aside>
                        }
                    }else {
                        html!{}
                    }}
                    {if self.drawer_state {
                        html!{
                            <div class={if self.splitter_start.is_some() {"splitter dragging"} else {"splitter"}}
                                title="Drag to resize, double click to reset"
                                onmousedown={link.callback(Msg::SplitterDown)} />
                        }
                    }else {
                        html!{}
                    }}
                    <main class="main-pane">
                        <CodeReader
                            file={self.verilog_source.clone()}
                            line={self.signal_add.1.location_define.line} />
                        <WaveShow
                            signaladd={(self.signal_add.0.clone(),Rc::clone(&self.signal_add.1))}
                            module={Rc::clone(&self.module)}
                            signal_value={Rc::clone(&self.signal_value)}
                            end_clock={self.module.end_clock} />
                    </main>
                </div>
                {if self.splitter_start.is_some() {
                    html!{
                        <div class="splitter-shield"
                            onmousemove={link.callback(Msg::SplitterMove)}
                            onmouseup={link.callback(|_| Msg::SplitterUp)}
                            //releasing the button outside the window never fires mouseup
                            onmouseout={link.callback(|_| Msg::SplitterUp)} />
                    }
                }else {
                    html!{}
                }}
            </div>
        }
    }
}

impl App {
    #[cfg(feature = "wasm")]
    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <FileLoad
                module={Rc::clone(&self.module)}
                ongetmodule={ctx.link().callback(Msg::GetModule)}
                ongetvalue={ctx.link().callback(Msg::GetValue)}
                ongetverilog={ctx.link().callback(Msg::GetVerilog)} />
        }
    }
    #[cfg(feature = "tauri")]
    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <FileLoad
                ongetmodule={ctx.link().callback(Msg::GetModule)}
                ongetverilog={ctx.link().callback(Msg::GetVerilog)} />
        }
    }
    #[cfg(feature = "server")]
    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <button onclick={ctx.link().callback(|_| Msg::WsSend("file".to_string()))}>{"ws"}</button>
        }
    }
    
}

#[cfg(feature = "server")]
fn create_websocket(ctx: &Context<App>) -> WebSocket {
    let callback: Callback<String> = ctx.link().callback(Msg::GetWebsocket);

    let window = web_sys::window().expect("should have a window in this context");
    let addr = window.location().hostname().unwrap();

    let ws = WebSocket::new(&format!("ws://{}:2993",addr)).unwrap();//TODO:when not connect

    let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
        if let Ok(txt) = e.data().dyn_into::<js_sys::JsString>() {
            console::log_1(&format!("message event, received Text: {:?}", txt).into());
            callback.emit(txt.as_string().unwrap())
        } else {
            console::log_1(&format!("message event, received Unknown: {:?}", e.data()).into());
        }
    }) as Box<dyn FnMut(MessageEvent)>);
    ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
    onmessage_callback.forget();
    ws
}

#[cfg(feature = "tauri")]
#[derive(Serialize, Deserialize)]
struct TauriArgs {
    key: String,
}
