use num::BigUint;
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub struct SignalValue {
    value: Vec<(i32,BigUint)>,
    height: String,
    bool_signal: bool,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalValueProps {
    pub value: Vec<(i32,BigUint)>,
    pub bool_signal: bool,
    pub zero_position: u32,
}

impl Component for SignalValue {
    type Message = ();
    type Properties = SignalValueProps;

    fn create(ctx: &Context<Self>) -> Self {
        let props = ctx.props();
        Self {
            value: props.value.clone(),
            height: "20px".to_string(),
            bool_signal: props.bool_signal,
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    //fn changed(&mut self, ctx: &Context<Self>) -> bool {
    //    true
    //}

    fn view(&self, ctx: &Context<Self>) -> Html {
        let zero_position = ctx.props().zero_position;
        let view_box = format!("0 0 {} {}", 2500, 50);
        if self.bool_signal {
            let mut points = String::new();
            let mut last: u32 = 0;
            for d in &self.value {
                if d.0>=0 && d.0 < 3000 {
                    if d.1 == BigUint::new(vec![1]){
                        points.push_str(&format!("{:.2},{:.2} ", d.0, zero_position+40));
                        points.push_str(&format!("{:.2},{:.2} ", d.0, zero_position));
                        last = zero_position;
                    }else {
                        points.push_str(&format!("{:.2},{:.2} ", d.0, zero_position));
                        points.push_str(&format!("{:.2},{:.2} ", d.0, zero_position+40));
                        last = zero_position+40;
                    }
                }
            };
            points.push_str(&format!("{:.2},{:.2} ", 3000, last));

            html! {
                <svg viewBox={view_box}>
                    <polyline points={points} fill="none" stroke={"rgb(255,0,0)"} />
                </svg>}
        }else {
            let mut points1 = String::new();
            let mut points2 = String::new();
            for d in &self.value {
                if d.0>=0 && d.0 < 3000 {
                    points1.push_str(&format!("{:.2},{:.2} ", d.0-2, zero_position+40));
                    points1.push_str(&format!("{:.2},{:.2} ", d.0, zero_position+20));
                    points1.push_str(&format!("{:.2},{:.2} ", d.0+2, zero_position+40));
                    points2.push_str(&format!("{:.2},{:.2} ", d.0-2, zero_position));
                    points2.push_str(&format!("{:.2},{:.2} ", d.0, zero_position+20));
                    points2.push_str(&format!("{:.2},{:.2} ", d.0+2, zero_position));
                }
            };
            points1.push_str(&format!("{:.2},{:.2} ", 3000, zero_position+40));
            points2.push_str(&format!("{:.2},{:.2} ", 3000, zero_position));

            html! {
                <svg viewBox={view_box}>
                    <polyline points={points1} fill="none" stroke={"rgb(255,0,0)"} />
                    <polyline points={points2} fill="none" stroke={"rgb(255,0,0)"} />
                </svg> }
        }
    }
}
