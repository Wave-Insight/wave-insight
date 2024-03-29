use std::rc::Rc;
use std::cell::RefCell;

use wave_insight_lib::data_struct::{Signal, ModuleValue, ShowType, BitsData, BoolData};
use yew::prelude::*;

use crate::wave_show::Settings;

#[derive(Clone, Debug, PartialEq)]
pub struct SignalValue {
    points1: String,
    points2: String,
    value: Vec<Html>,
    height: String,
    bool_signal: bool,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalValueProps {
    pub signal_value: Rc<RefCell<ModuleValue>>,
    pub signal: Rc<Signal>,
    pub bool_signal: bool,
    pub x_axis: f64,
    pub size: f64,
    pub width: f64,
    pub setting: Settings,

    pub cursor1: Option<i32>,
    pub cursor2: Option<i32>,
}

impl Component for SignalValue {
    type Message = ();
    type Properties = SignalValueProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        let (points1, points2, value) = wave_svg(props);
        Self {
            points1,
            points2,
            value,
            height: "20px".to_string(),
            bool_signal: props.bool_signal,
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let props = ctx.props();
        let (points1, points2, value) = wave_svg(props);
        self.points1 = points1;
        self.points2 = points2;
        self.value = value;
        self.bool_signal = props.bool_signal;
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let props = ctx.props();
        let color = format!("rgb({},{},{})",props.setting.color.0,props.setting.color.1,props.setting.color.2);
        if self.bool_signal {
            html! {
                <svg style="height:26px;width:100%">
                    <polyline points={self.points1.clone()} fill="none" stroke={color} />
                    {
                        if let Some(c1) = ctx.props().cursor1 {html!{
                            <line x1={format!("{}",((c1 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                x2={format!("{}",((c1 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                y1={0}
                                y2={26}
                                style="stroke:rgb(255,255,0);stroke-width:3" />
                        }}else {
                            html!{}
                        }
                    }
                    {
                        if let Some(c2) = ctx.props().cursor2 {html!{
                            <line x1={format!("{}",((c2 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                x2={format!("{}",((c2 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                y1={0}
                                y2={26}
                                style="stroke:rgb(255,255,255);stroke-width:3" />
                        }}else {
                            html!{}
                        }
                    }
                </svg>}
        }else {
            html! {
                <svg style="height:26px;width:100%">
                    <polyline points={self.points1.clone()} fill="none" stroke={color.clone()} />
                    <polyline points={self.points2.clone()} fill="none" stroke={color} />
                    {for self.value.clone()}
                    {
                        if let Some(c1) = ctx.props().cursor1 {html!{
                            <line x1={format!("{}",((c1 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                x2={format!("{}",((c1 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                y1={0}
                                y2={26}
                                style="stroke:rgb(255,255,0);stroke-width:3" />
                        }}else {
                            html!{}
                        }
                    }
                    {
                        if let Some(c2) = ctx.props().cursor2 {html!{
                            <line x1={format!("{}",((c2 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                x2={format!("{}",((c2 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                y1={0}
                                y2={26}
                                style="stroke:rgb(255,255,255);stroke-width:3" />
                        }}else {
                            html!{}
                        }
                    }
                </svg> }
        }
    }
}

fn wave_svg(props: &SignalValueProps) -> (String,String,Vec<Html>) {
    let x_axis = props.x_axis;
    let size = props.size;
    let show_type = &props.setting.show_type;
    let bitcount = props.signal.size as u32;
    let zero_position = 3;
    let base_height = 10;
    let height = 2*base_height;
    let width = props.width;
    let mut points1 = String::new();
    let mut points2 = String::new();
    let mut value: Vec<Html> = vec![];
    if props.bool_signal {
        let mut last: u32 = zero_position;
        let mut head: u32 = 0;
        let mut head_used = false;
        for d in props.signal_value.borrow().get_bool(&props.signal.value_key) {
            let x = ((d.0 as f64) - x_axis)*size;
            if (0.0..width).contains(&x) {
                if !head_used {
                    points1.push_str(&format!("{:.2},{} ", 0, zero_position+(2-head)*base_height));
                    last = zero_position+(2-head)*base_height;
                    head_used = true;
                }
                match d.1 {
                    BoolData::Zero => {
                        points1.push_str(&format!("{x:.2},{last} "));
                        last = zero_position+height;
                        points1.push_str(&format!("{x:.2},{last} "));
                    },
                    BoolData::One => {
                        points1.push_str(&format!("{x:.2},{last} "));
                        last = zero_position;
                        points1.push_str(&format!("{x:.2},{last} "));
                    },
                    BoolData::X => {//TODO:X and Z should have different color
                        points1.push_str(&format!("{x:.2},{last} "));
                        last = zero_position+base_height;
                        points1.push_str(&format!("{x:.2},{last} "));
                    },
                    BoolData::Z => {
                        points1.push_str(&format!("{x:.2},{last} "));
                        last = zero_position+base_height;
                        points1.push_str(&format!("{x:.2},{last} "));
                    },
                }
            }else if x < 0.0 {
                head = match d.1 {
                    BoolData::Zero => 0,
                    BoolData::One => 2,
                    BoolData::X => 1,
                    BoolData::Z => 1,
                }
            }
        };
        if !head_used {
            points1.push_str(&format!("{:.2},{} ", 0, zero_position+(2-head)*base_height));
            last = zero_position+(2-head)*base_height;
        }
        points1.push_str(&format!("{width:.2},{last} "));

    }else {
            
        let mut head = BitsData::new(vec![(0,0)]);
        let mut head_used = true;
        let mut last_x = 0.0;
        let mut all_signal_value = props.signal_value.borrow().get_bits(&props.signal.value_key);
        all_signal_value.push((1<<30, BitsData::new(vec![(0, 0)])));//TODO:end clk
        for (d, d_next) in all_signal_value.iter().zip(all_signal_value.iter().skip(1)) {
            let x = ((d.0 as f64) - x_axis)*size;
            let x_next = ((d_next.0 as f64) - x_axis)*size;
            if (0.0..width).contains(&x) {
                if !head_used {
                    head_used = true;
                    points1.push_str(&format!("{:.2},{} ", 0, zero_position+height));
                    points2.push_str(&format!("{:.2},{} ", 0, zero_position));
                    value.push(value_text(0.0, &head, show_type, bitcount, x));
                }
                if x - last_x <= 12.0 {
                    value.pop();//TODO:pop it in fn value_text? or no need to pop?
                }
                points1.push_str(&format!("{:.2},{} ", x-2.0, zero_position+height));
                points1.push_str(&format!("{:.2},{} ", x, zero_position+height/2));
                points1.push_str(&format!("{:.2},{} ", x+2.0, zero_position+height));
                points2.push_str(&format!("{:.2},{} ", x-2.0, zero_position));
                points2.push_str(&format!("{:.2},{} ", x, zero_position+height/2));
                points2.push_str(&format!("{:.2},{} ", x+2.0, zero_position));

                value.push(value_text(x+2.0, &d.1, show_type, bitcount, x_next-x));
            }else if x < 0.0 {
                head = d.1.clone();
                head_used = false;
            }
            last_x = x;
        };
        if !head_used {
            points1.push_str(&format!("{:.2},{} ", 0, zero_position+height));
            points2.push_str(&format!("{:.2},{} ", 0, zero_position));
            value.push(value_text(0.0, &head, show_type, bitcount, width));
        }
        points1.push_str(&format!("{:.2},{} ", width, zero_position+height));
        points2.push_str(&format!("{width:.2},{zero_position} "));
    }
    (points1, points2, value)
}

fn value_text(begin: f64, value: &BitsData, show_type: &ShowType, bitcount: u32, show_width: f64) -> Html {
    let zero_position = 3;
    let text_raw = value.to_string(bitcount as usize, show_type);
    let avaliable = (show_width/9.0) as usize;
    let text = if avaliable<= 1 {
        "".to_string()
    }else if text_raw.len() < avaliable {
        text_raw
    }else {
        text_raw.get(..avaliable-1).unwrap().to_string()+"."
    };
    html!{
        <text x={format!("{begin}")} y={format!("{}",zero_position+17)} fill="rgb(255,255,255)">
            {
                text
            }
        </text>
    }
}
