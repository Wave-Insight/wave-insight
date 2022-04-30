use std::rc::Rc;

use num::{BigUint,BigInt, bigint::{ToBigInt, Sign}};
use wave_insight_lib::data_struct::{Signal, Module};
use yew::prelude::*;

use crate::wave_show::{Settings, ShowType};

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
    pub module: Rc<Module>,
    pub signal: Rc<Signal>,
    pub bool_signal: bool,
    pub x_axis: f64,
    pub size: f64,
    pub width: f64,
    pub setting: Settings,
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
                </svg>}
        }else {
            html! {
                <svg style="height:26px;width:100%">
                    <polyline points={self.points1.clone()} fill="none" stroke={color.clone()} />
                    <polyline points={self.points2.clone()} fill="none" stroke={color} />
                    {for self.value.clone()}
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
    let height = 20;
    let width = props.width;
    let mut points1 = String::new();
    let mut points2 = String::new();
    let mut value: Vec<Html> = vec![];
    if props.bool_signal {
        let mut last: u32 = 0;
        let mut head: u32 = 0;
        let mut head_used = false;
        for d in props.module.value.get(&props.signal.value_key).unwrap_or(&vec![]) {
            let x = ((d.0 as f64) - x_axis)*size;
            if (0.0..width).contains(&x) {
                if !head_used {
                    points1.push_str(&format!("{:.2},{} ", 0, zero_position+(1-head)*height));
                    head_used = true;
                }
                if d.1 == BigUint::new(vec![1]){
                    points1.push_str(&format!("{:.2},{} ", x, zero_position+height));
                    points1.push_str(&format!("{:.2},{} ", x, zero_position));
                    last = zero_position;
                }else {
                    points1.push_str(&format!("{:.2},{} ", x, zero_position));
                    points1.push_str(&format!("{:.2},{} ", x, zero_position+height));
                    last = zero_position+height;
                }
            }else if x < 0.0 {
                if d.1 == BigUint::new(vec![1]) {
                    head = 1;
                }else {
                    head = 0;
                }
            }
        };
        if !head_used {
            points1.push_str(&format!("{:.2},{} ", 0, zero_position+(1-head)*height));
            last = zero_position+(1-head)*height;
        }
        points1.push_str(&format!("{:.2},{} ", width, last));

    }else {
            
        let mut head: BigUint = BigUint::new(vec![0]);
        let mut head_used = true;
        let mut last_x = 0.0;
        for d in props.module.value.get(&props.signal.value_key).unwrap_or(&vec![]) {
            let x = ((d.0 as f64) - x_axis)*size;
            if (0.0..width).contains(&x) {
                if !head_used {
                    head_used = true;
                    points1.push_str(&format!("{:.2},{} ", 0, zero_position+height));
                    points2.push_str(&format!("{:.2},{} ", 0, zero_position));
                    value.push(value_text(0.0, &head, show_type, bitcount));
                }
                if x - last_x <= 12.0 {
                    value.pop();
                }
                points1.push_str(&format!("{:.2},{} ", x-2.0, zero_position+height));
                points1.push_str(&format!("{:.2},{} ", x, zero_position+height/2));
                points1.push_str(&format!("{:.2},{} ", x+2.0, zero_position+height));
                points2.push_str(&format!("{:.2},{} ", x-2.0, zero_position));
                points2.push_str(&format!("{:.2},{} ", x, zero_position+height/2));
                points2.push_str(&format!("{:.2},{} ", x+2.0, zero_position));

                value.push(value_text(x+2.0, &d.1, show_type, bitcount));
            }else if x < 0.0 {
                head = d.1.clone();
                head_used = false;
            }
            last_x = x;
        };
        if !head_used {
            points1.push_str(&format!("{:.2},{} ", 0, zero_position+height));
            points2.push_str(&format!("{:.2},{} ", 0, zero_position));
            value.push(value_text(0.0, &head, show_type, bitcount));
        }
        points1.push_str(&format!("{:.2},{} ", width, zero_position+height));
        points2.push_str(&format!("{:.2},{} ", width, zero_position));
    }
    (points1, points2, value)
}

fn value_text(begin: f64, value: &BigUint, show_type: &ShowType, bitcount: u32) -> Html {
    let zero_position = 3;
    html!{
        <text x={format!("{}",begin)} y={format!("{}",zero_position+17)} fill="rgb(255,255,255)">
            {
                if *show_type==ShowType::Hex {
                    value.to_str_radix(16).to_string()
                }else if *show_type==ShowType::Oct {
                    value.to_str_radix(8).to_string()
                }else if *show_type==ShowType::Bin {
                    value.to_str_radix(2).to_string()
                }else if *show_type==ShowType::UInt {
                    format!("{}",value)
                }else if *show_type==ShowType::SInt {
                    let bound = BigUint::new(vec![2]).pow(bitcount-1);
                    let value_to_sint = if *value >= bound {
                        value.to_bigint().unwrap() - BigInt::new(Sign::Plus,vec![2]).pow(bitcount)
                    }else {
                        value.to_bigint().unwrap()
                    };
                    format!("{}",value_to_sint)
                }else {
                    let value_to_bytes = value.to_bytes_be();
                    let s = match std::str::from_utf8(&value_to_bytes) {
                        Ok(v) => v,
                        Err(_e) => "invalid",//TODO:do not panic
                    };
                    s.to_string()
                }
            }
        </text>
    }
}
