use wave_insight_lib::hello_string;
use yew::prelude::*;
use crate::code_reader::CodeReader;
use crate::wave_show::WaveShow;
use crate::file_load::FileLoad;
use crate::module_struct::ModuleStruct;

use crate::top_bar::TopBar;
use material_yew::{MatDrawer,
    drawer::{MatDrawerAppContent, MatDrawerTitle},}; 

pub enum Msg {
    NavIconClick,
    Opened,
    Closed,
}

pub struct App {
    drawer_state: bool,
}

impl Component for App {
    type Message = Msg;
    type Properties = ();

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            drawer_state: false,
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
                        <FileLoad/>
                        <ModuleStruct/>
                    </div>
                    <MatDrawerAppContent>
                        <div class="app-content" > <TopBar onnavigationiconclick={link.callback(|_| Msg::NavIconClick)}/> </div>
                        <p>{ hello_string() }</p>
                        <div>
                            <CodeReader/>
                            <WaveShow/>
                        </div>
                    </MatDrawerAppContent>
                </MatDrawer>
            </div>
        }
    }
}
