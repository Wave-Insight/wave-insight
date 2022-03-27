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
    SignalAdd((Vec<String>,String)),
}

pub struct App {
    drawer_state: bool,
    module: Module,
    verilog_source: Vec<(String,String)>,
    signal_add: (String,Signal),
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            drawer_state: true,
            module: Module::new(),
            verilog_source: vec![],
            signal_add: ("".to_string(),Signal::new()),
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
                    FileType::IsVcd => {self.module = vcd_parser(&text,self.module.clone())},
                    FileType::IsVerilog => {
                        self.verilog_source.push((file_name,text.clone()));
                        self.module = verilog_parser(&text,self.module.clone());
                    },
                }
                console::log_1(&format!("finish parser {}",(match file_type {FileType::IsVcd=>{"vcd"},FileType::IsVerilog=>{"verilog"},})).into());
                true
            }
            Msg::SignalAdd(input) => {
                self.signal_add = (input.1.clone(),self.module.get_signal(&input).unwrap().clone());
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div style="height:100%">
                <TopBar onnavigationiconclick={link.callback(|_| Msg::NavIconClick)}/>
                {if self.drawer_state {
                    html!{
                    <div style="width:20%;float:left">
                        <FileLoad ongetfile={link.callback(|i:(FileType,String,String)| Msg::ParserFile(i.0,i.1,i.2))}/>
                        <ModuleStruct module={self.module.clone()} signaladd={link.callback(Msg::SignalAdd)}/>
                    </div>
                    }
                }else {
                    html!{}
                }}
                <div style={"width:".to_owned()+(if self.drawer_state {"80%"} else {"100%"})+";float:left;display:block;height:100%;overflow-y:auto"} >
                    <div style="display:block;height:100%;overflow-y:auto">
                        <CodeReader file={self.verilog_source.clone()} />
                        <WaveShow signaladd={self.signal_add.clone()} />
                    </div>
                </div>
            </div>
        }
    }
}
