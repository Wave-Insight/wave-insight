use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::wave_show::ShowType;

use super::settings::Settings;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct CtrlProps {
    pub name: String,
    pub setting: Settings,
    #[prop_or_default]
    pub onset: Callback<(bool,Settings)>,
}

pub enum Msg {
    ExitMenu(MouseEvent),
    ChooseShowType(ShowType),
    ActiveFixed,
    SetFixed(u32),
    ActiveAnalog,
    SetAnalog(u32),
}

pub struct Ctrl {
}

impl Component for Ctrl {
    type Message = Msg;
    type Properties = CtrlProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
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
            Msg::ActiveFixed => {
                let mut set = props.setting.clone();
                set.fixed_active = !set.fixed_active;
                props.onset.emit((true,set));
                true
            }
            Msg::SetFixed(exp) => {
                let mut set = props.setting.clone();
                set.fixed_point = exp;
                props.onset.emit((true,set));
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
                props.onset.emit((true,set));
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

        // This gives us a component's "`Scope`" which allows us to send messages, etc to the component.
        html! {
            <div style="background:rgba(0, 0, 0, 0.6);position:fixed;top:0;left:0;right:0;bottom:0;z-index:101;"
                onclick={link.callback(Msg::ExitMenu)} >
                <div style="position:absolute;top:50%;left:50%;background:#fff;height:70%;width:30%;transform:translate(-50%, -50%);border-radius: 15px;
  border: 2px solid #2e84f8;">
                    <h1>{props.name.clone()}</h1>
                    <h2>{"Format"}</h2>
                    <button type="button" style={if props.setting.show_type==ShowType::Hex   {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Hex))}>{"Hex"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::UInt  {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::UInt))}>{"Dec"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::SInt  {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::SInt))}>{"Signed"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::Oct   {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Oct))}>{"Oct"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::Bin   {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Bin))}>{"Bin"}</button>
                    <button type="button" style={if props.setting.show_type==ShowType::Ascii {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ChooseShowType(ShowType::Ascii))}>{"ASCII"}</button>
                    <h2>{"Fixed"}</h2>
                    <button type="button" style={if props.setting.fixed_active {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ActiveFixed)}>{"active"}</button>
                    <input type="range" onchange={link.callback(|e: Event| Msg::SetFixed(e.target_unchecked_into::<HtmlInputElement>().value_as_number() as u32))} />
                    <h2>{"Analog"}</h2>
                    <button type="button" style={if props.setting.analog_active {button_style_onselect} else {button_style}} onclick={link.callback(|_| Msg::ActiveAnalog)}>{"active"}</button>
                    <input type="range" onchange={link.callback(|e: Event| Msg::SetAnalog(e.target_unchecked_into::<HtmlInputElement>().value_as_number() as u32))} />
                </div>
            </div>
        }
    }
}
