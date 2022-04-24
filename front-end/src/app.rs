use std::rc::Rc;

use wave_insight_lib::{data_struct::Module,
    data_struct::Signal,
    parser::vcd_parser::vcd_parser,
    parser::verilog_parser::verilog_parser};

use yew::prelude::*;
use web_sys::console;//TODO:for debug

use crate::code_reader::CodeReader;
use crate::wave_show::WaveShow;
use crate::file_load::FileLoad;
use crate::file_load::FileType;
use crate::module_struct::ModuleStruct;

use crate::top_bar::TopBar;

pub enum Msg {
    NavIconClick,
    ParserFile(FileType,String,String),
    SignalAdd((String,Rc<Signal>)),
}

pub struct App {
    drawer_state: bool,
    module: Rc<Module>,
    verilog_source: Vec<(String,String)>,
    signal_add: (String,Rc<Signal>),
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
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::NavIconClick => {
                self.drawer_state = !self.drawer_state;
                true
            }
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
                        <FileLoad ongetfile={link.callback(|i:(FileType,String,String)| Msg::ParserFile(i.0,i.1,i.2))}/>
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
