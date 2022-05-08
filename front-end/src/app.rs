use std::rc::Rc;

use wave_insight_lib::{data_struct::Module,
    data_struct::Signal};

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

#[cfg(feature = "wasm")]
use wave_insight_lib::{
    parser::vcd_parser::vcd_parser,
    parser::verilog_parser::verilog_parser};
#[cfg(feature = "wasm")]
use crate::file_load::FileLoad;
#[cfg(feature = "wasm")]
use crate::file_load::FileType;

pub enum Msg {
    NavIconClick,
    #[cfg(feature = "wasm")]
    ParserFile(FileType,String,String),
    SignalAdd((String,Rc<Signal>)),
    #[cfg(feature = "backend")]
    WsSend(String),
}

pub struct App {
    drawer_state: bool,
    module: Rc<Module>,
    verilog_source: Vec<(String,String)>,
    signal_add: (String,Rc<Signal>),
    #[cfg(feature = "backend")]
    websocket: Option<WebSocket>,
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            drawer_state: true,
            module: Rc::new(Module::new()),
            verilog_source: vec![],
            signal_add: ("".to_string(),Rc::new(Signal::new())),
            #[cfg(feature = "backend")]
            websocket: None,
        }
    }

    #[cfg(feature = "backend")]
    fn rendered(&mut self, _ctx: &Context<Self>, _first_render: bool) {
        let ws = WebSocket::new("ws://127.0.0.1:2992").unwrap();//TODO:when not connect

        let onmessage_callback = Closure::wrap(Box::new(move |e: MessageEvent| {
            if let Ok(txt) = e.data().dyn_into::<js_sys::JsString>() {
                console::log_1(&format!("message event, received Text: {:?}", txt).into());
            } else {
                console::log_1(&format!("message event, received Unknown: {:?}", e.data()).into());
            }
        }) as Box<dyn FnMut(MessageEvent)>);
        ws.set_onmessage(Some(onmessage_callback.as_ref().unchecked_ref()));
        onmessage_callback.forget();

        self.websocket = Some(ws);
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::NavIconClick => {
                self.drawer_state = !self.drawer_state;
                true
            }
            #[cfg(feature = "wasm")]
            Msg::ParserFile(file_type,file_name,text) => {
                match file_type {
                    FileType::IsVcd => {self.module = Rc::new(vcd_parser(&text,&mut Module::new()))},//TODO:module::new()
                    FileType::IsVerilog => {
                        self.module = Rc::new(verilog_parser(&text,Rc::clone(&self.module)));
                        self.verilog_source.push((file_name,text));
                    },
                }
                console::log_1(&format!("finish parser {}",(match file_type {FileType::IsVcd=>{"vcd"},FileType::IsVerilog=>{"verilog"},})).into());
                true
            }
            Msg::SignalAdd(input) => {
                self.signal_add = (input.0,input.1);
                true
            }
            #[cfg(feature = "backend")]
            Msg::WsSend(e) => {
                match &mut self.websocket {
                    Some(ws) => {
                       if ws.send_with_str(&e).is_ok() {};
                    },
                    None => {},
                }
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();

        let window = web_sys::window().expect("should have a window in this context");
        let win_height = window.inner_height().unwrap().as_f64().unwrap()-64.0;
        //console::log_1(&format!("height2 {:?}",win_height).into());

        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div style={"height:".to_owned()+&win_height.to_string()+"px"}>
                <TopBar onnavigationiconclick={link.callback(|_| Msg::NavIconClick)}/>
                {if self.drawer_state {
                    html!{
                    <div style="width:20%;float:left;height:100%;overflow-y:auto">
                        {self.file_button(ctx)}
                        <ModuleStruct module={Rc::clone(&self.module)} signaladd={link.callback(Msg::SignalAdd)}/>
                    </div>
                    }
                }else {
                    html!{}
                }}
                <div style={"width:".to_owned()+(if self.drawer_state {"80%"} else {"100%"})+";float:left;display:block;height:100%;overflow-y:auto"} >
                    <div style="display:block;height:100%;overflow-y:auto">
                        <CodeReader file={self.verilog_source.clone()} />
                        <WaveShow signaladd={(self.signal_add.0.clone(),Rc::clone(&self.signal_add.1))} module={Rc::clone(&self.module)} end_clock={self.module.end_clock} />
                    </div>
                </div>
            </div>
        }
    }
}

impl App {
    #[cfg(feature = "wasm")]
    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <FileLoad ongetfile={ctx.link().callback(|i:(FileType,String,String)| Msg::ParserFile(i.0,i.1,i.2))}/>
        }
    }
    #[cfg(feature = "backend")]
    fn file_button(&self, ctx: &Context<Self>) -> Html {
        html!{
            <button onclick={ctx.link().callback(|_| Msg::WsSend("file".to_string()))}>{"ws"}</button>
        }
    }
}
