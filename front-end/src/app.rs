use wave_insight_lib::{data_struct::Module,
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
use material_yew::{MatDrawer,
    drawer::{MatDrawerAppContent, MatDrawerTitle},};

pub enum Msg {
    NavIconClick,
    Opened,
    Closed,
    ParserFile(FileType,String,String),
}

pub struct App {
    drawer_state: bool,
    module: Module,
    verilog_source: Vec<(String,String)>,
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            drawer_state: true,
            module: Module::new(),
            verilog_source: vec![],
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::NavIconClick => {
                self.drawer_state = !self.drawer_state;
                true
            }
            Msg::Closed => {
                self.drawer_state = false;
                false
            }
            Msg::Opened => {
                self.drawer_state = true;
                false
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
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                <MatDrawer open={self.drawer_state} drawer_type="dismissible"
                           onopened={link.callback(|_| Msg::Opened)}
                           onclosed={link.callback(|_| Msg::Closed)}>

                    <MatDrawerTitle>
                        <span class="drawer-title">{"Components"}</span>
                    </MatDrawerTitle>

                    <div class="drawer-content">
                        <FileLoad ongetfile={link.callback(|i:(FileType,String,String)| Msg::ParserFile(i.0,i.1,i.2))}/>
                        <ModuleStruct module={self.module.clone()} />
                    </div>
                    <MatDrawerAppContent>
                        <div class="app-content" >
                            <TopBar onnavigationiconclick={link.callback(|_| Msg::NavIconClick)}/>
                            <div>
                                <CodeReader file={self.verilog_source.clone()}/>
                                <WaveShow/>
                            </div>
                        </div>
                    </MatDrawerAppContent>
                </MatDrawer>
            </div>
        }
    }
}
