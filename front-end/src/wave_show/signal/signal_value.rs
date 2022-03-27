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
    pub x_axis: f64,
    pub size: f64,
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
        let props = ctx.props();
        let x_axis = props.x_axis;
        let size = props.size;
        let zero_position = 5;
        let view_box = format!("0 0 {} {}", 2500, 50);
        if self.bool_signal {
            let mut points = String::new();
            let mut last: u32 = 0;
            let mut head: u32 = 0;
            let mut head_used = false;
            for d in &self.value {
                let x = ((d.0 as f64) - x_axis)*size;
                if (0.0..3000.0).contains(&x) {
                    if !head_used {
                        points.push_str(&format!("{:.2},{:.2} ", 0, zero_position+(1-head)*40));
                        head_used = true;
                    }
                    if d.1 == BigUint::new(vec![1]){
                        points.push_str(&format!("{:.2},{:.2} ", x, zero_position+40));
                        points.push_str(&format!("{:.2},{:.2} ", x, zero_position));
                        last = zero_position;
                    }else {
                        points.push_str(&format!("{:.2},{:.2} ", x, zero_position));
                        points.push_str(&format!("{:.2},{:.2} ", x, zero_position+40));
                        last = zero_position+40;
                    }
                }else if d.1 == BigUint::new(vec![1]) {
                    head = 1;
                }else {
                    head = 0;
                }
            };
            if !head_used {
                points.push_str(&format!("{:.2},{:.2} ", 0, zero_position+(1-head)*40));
                last = zero_position+(1-head)*40;
            }
            points.push_str(&format!("{:.2},{:.2} ", 3000, last));

            html! {
                <svg viewBox={view_box}>
                    <polyline points={points} fill="none" stroke={"rgb(0,255,0)"} />
                </svg>}
        }else {
            let mut points1 = String::new();
            let mut points2 = String::new();
            let mut value: Vec<Html> = vec![];
            let mut head: BigUint = BigUint::new(vec![0]);
            let mut head_used = true;
            for d in &self.value {
                let x = ((d.0 as f64) - x_axis)*size;
                if (0.0..3000.0).contains(&x) {
                    if !head_used {
                        head_used = true;
                        points1.push_str(&format!("{:.2},{:.2} ", 0, zero_position+40));
                        points2.push_str(&format!("{:.2},{:.2} ", 0, zero_position));
                        value.push(
                            html!{
                                <text x={format!("{}",0)} y={format!("{}",zero_position+37)} fill="rgb(255,255,255)">
                                    {format!("{}",head)}
                                </text>
                            }
                        );
                    }
                    points1.push_str(&format!("{:.2},{:.2} ", x-2.0, zero_position+40));
                    points1.push_str(&format!("{:.2},{:.2} ", x, zero_position+20));
                    points1.push_str(&format!("{:.2},{:.2} ", x+2.0, zero_position+40));
                    points2.push_str(&format!("{:.2},{:.2} ", x-2.0, zero_position));
                    points2.push_str(&format!("{:.2},{:.2} ", x, zero_position+20));
                    points2.push_str(&format!("{:.2},{:.2} ", x+2.0, zero_position));

                    value.push(
                        html!{
                            <text x={format!("{}",x+2.0)} y={format!("{}",zero_position+37)} fill="rgb(255,255,255)">
                                {format!("{}",d.1)}
                            </text>
                        }
                    );
                }else if x < 0.0 {
                    head = d.1.clone();
                    head_used = false;
                }
            };
            if !head_used {
                points1.push_str(&format!("{:.2},{:.2} ", 0, zero_position+40));
                points2.push_str(&format!("{:.2},{:.2} ", 0, zero_position));
                value.push(
                    html!{
                        <text x={format!("{}",0)} y={format!("{}",zero_position+37)} fill="rgb(255,255,255)">
                            {format!("{}",head)}
                        </text>
                    }
                );
            }
            points1.push_str(&format!("{:.2},{:.2} ", 3000, zero_position+40));
            points2.push_str(&format!("{:.2},{:.2} ", 3000, zero_position));

            html! {
                <svg viewBox={view_box}>
                    <polyline points={points1} fill="none" stroke={"rgb(0,255,0)"} />
                    <polyline points={points2} fill="none" stroke={"rgb(0,255,0)"} />
                    {for value}
                </svg> }
        }
    }
}
