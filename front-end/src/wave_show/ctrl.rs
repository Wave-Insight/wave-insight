use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::wave_show::ShowType;

use super::settings::Settings;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CtrlProps {
    pub name: String,
    pub setting: Settings,
    pub load: Vec<String>,
    pub drive: Vec<String>,
    #[prop_or_default]
    pub onset: Callback<(bool,Settings)>,//<close ctrl menu,new Settings>
    pub delete: Callback<()>,
}

pub enum Msg {
    ExitMenu(MouseEvent),
    ChooseShowType(ShowType),
    ChooseColor((u8,u8,u8)),
    ActiveFixed,
    SetFixed(u32),
    ActiveAnalog,
    SetAnalog(u32),
    FlipLoadShow,
    FlipDriveShow,
    AddAllLoad,
    AddAllDrive,
    DeleteSig,
}

pub struct Ctrl {
    show_load: bool,
    show_drive: bool,
}

impl Component for Ctrl {
    type Message = Msg;
    type Properties = CtrlProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            show_load: false,
            show_drive: false,
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::ExitMenu(_e) => {
                props.onset.emit((true,props.setting.clone()));
                true
            }
            Msg::ChooseShowType(show_type) => {
                let mut set = props.setting.clone();
                set.show_type = show_type;
                props.onset.emit((true,set));
                true
            }
            Msg::ChooseColor((r,g,b)) => {
                let mut set = props.setting.clone();
                set.color = (r,g,b);
                props.onset.emit((true,set));
                true
            }
            Msg::ActiveFixed => {
                let mut set = props.setting.clone();
                set.fixed_active = !set.fixed_active;
                props.onset.emit((true,set));
                true
            }
            Msg::SetFixed(exp) => {
                let mut set = props.setting.clone();
                set.fixed_point = exp;
                props.onset.emit((false,set));
                true
            }
            Msg::ActiveAnalog => {
                let mut set = props.setting.clone();
                set.analog_active = !set.analog_active;
                props.onset.emit((true,set));
                true
            }
            Msg::SetAnalog(interpolate) => {
                let mut set = props.setting.clone();
                set.analog_initerpolate = interpolate;
                props.onset.emit((false,set));
                true
            }
            Msg::FlipLoadShow => {
                self.show_load = !self.show_load;
                true
            }
            Msg::FlipDriveShow => {
                self.show_drive = !self.show_drive;
                true
            }
            Msg::AddAllDrive => {
                true
            }
            Msg::AddAllLoad => {
                true
            }
            Msg::DeleteSig => {
                props.delete.emit(());
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let props = ctx.props();

        let button_style = "
            background-color:#ffffff;
            border-radius:6px;
            border:1px solid #dcdcdc;
            display:inline-block;
            cursor:pointer;
            color:#666666;font-size:15px;font-weight:bold;
            margin:2% 6.5%;height:5%;width:20%;";

        let button_style_onselect = "
            background-color:#ffffff;
            border-radius:6px;
            border:2px solid #55ff55;
            display:inline-block;
            cursor:pointer;
            color:#666666;font-size:15px;font-weight:bold;
            margin:2% 6.5%;height:5%;width:20%;";

        let color_style = |(r,g,b):(u8,u8,u8)| format!("
            background-color:#{:02X?}{:02X?}{:02X?};
            border-radius:6px;
            border:2px solid #dcdcdc;
            display:inline-block;
            cursor:pointer;
            margin:2% 1.5%;height:5%;width:10%;",r,g,b);

        let color_style_onselect = |(r,g,b):(u8,u8,u8)| format!("
            background-color:#{:02X?}{:02X?}{:02X?};
            border-radius:6px;
            border:3px solid #008800;
            display:inline-block;
            cursor:pointer;
            margin:2% 1.5%;height:5%;width:10%;",r,g,b);

        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div>
                <div style="background:rgba(0, 0, 0, 0.6);position:fixed;top:0;left:0;right:0;bottom:0;z-index:101;"
                    onclick={link.callback(Msg::ExitMenu)} >
                </div>
                <div style="position:absolute;top:50%;left:50%;
                            background:#fff;height:70%;width:30%;
                            transform:translate(-50%, -50%);
                            border-radius: 15px;border: 2px solid #2e84f8;
                            z-index:102;">
                    <h4>{props.name.clone()}</h4>
                    <h5>{"Format"}</h5>
                    <button type="button" style={if props.setting.show_type==ShowType::Hex   {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Hex))}>{"Hex"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::UInt  {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::UInt))}>{"Dec"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::SInt  {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::SInt))}>{"Signed"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::Oct   {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Oct))}>{"Oct"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::Bin   {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Bin))}>{"Bin"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::Ascii {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Ascii))}>{"ASCII"}</button>
                    <h5>{"Fixed"}</h5>
                    <button type="button" style={if props.setting.fixed_active {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ActiveFixed)}>{"active"}</button>
                    <input type="number" onchange={link.callback(|e: Event| Msg::SetFixed(e.target_unchecked_into::<HtmlInputElement>().value_as_number() as u32))} />
                    <h5>{"Analog"}</h5>
                    <button type="button" style={if props.setting.analog_active {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ActiveAnalog)}>{"active"}</button>
                    <input type="number" onchange={link.callback(|e: Event| Msg::SetAnalog(e.target_unchecked_into::<HtmlInputElement>().value_as_number() as u32))} />
                    <h5>{"L&D"}</h5>
                    <button type="button" style={button_style} 
                        onclick={link.callback(|_| Msg::AddAllLoad)}
                        onmouseover={link.callback(|_| Msg::FlipLoadShow)}
                        onmouseout={link.callback(|_| Msg::FlipLoadShow)}>{"load"}</button>
                    { if self.show_load {html!{
                        <span style="position:absolute;background-color:#ffffff">
                            {for (&props.load).iter().map(|l| {
                                html!{<p>{l}</p>}
                            })}
                        </span>
                    }}else {
                        html!{}
                    }}
                    <button type="button" style={button_style} 
                        onclick={link.callback(|_| Msg::AddAllDrive)}
                        onmouseover={link.callback(|_| Msg::FlipDriveShow)}
                        onmouseout={link.callback(|_| Msg::FlipDriveShow)}>{"drive"}</button>
                    { if self.show_drive {html!{
                        <span style="position:absolute;background-color:#ffffff">
                            {for (&props.drive).iter().map(|l| {
                                html!{<p>{l}</p>}
                            })}
                        </span>
                    }}else {
                        html!{}
                    }}
                    <button type="button" style={button_style}
                        onclick={link.callback(|_| Msg::DeleteSig)}>
                        {"delete"}</button>

                    <button type="button" style={if props.setting.color==(255,0,0  ) {color_style_onselect((255,0,0))  } else {color_style((255,0,0))  }} onclick={link.callback(|_| Msg::ChooseColor((255,0,0)))}></button>
                    <button type="button" style={if props.setting.color==(255,128,0) {color_style_onselect((255,128,0))} else {color_style((255,128,0))}} onclick={link.callback(|_| Msg::ChooseColor((255,128,0)))}></button>
                    <button type="button" style={if props.setting.color==(255,255,0) {color_style_onselect((255,255,0))} else {color_style((255,255,0))}} onclick={link.callback(|_| Msg::ChooseColor((255,255,0)))}></button>
                    <button type="button" style={if props.setting.color==(0,255,0  ) {color_style_onselect((0,255,0))  } else {color_style((0,255,0))  }} onclick={link.callback(|_| Msg::ChooseColor((0,255,0)))}></button>
                    <button type="button" style={if props.setting.color==(0,255,255) {color_style_onselect((0,255,255))} else {color_style((0,255,255))}} onclick={link.callback(|_| Msg::ChooseColor((0,255,255)))}></button>
                    <button type="button" style={if props.setting.color==(0,0,255  ) {color_style_onselect((0,0,255))  } else {color_style((0,0,255))  }} onclick={link.callback(|_| Msg::ChooseColor((0,0,255)))}></button>
                    <button type="button" style={if props.setting.color==(255,0,255) {color_style_onselect((255,0,255))} else {color_style((255,0,255))}} onclick={link.callback(|_| Msg::ChooseColor((255,0,255)))}></button>
                </div>
            </div>
        }
    }
}
