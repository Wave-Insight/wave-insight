use serde::{Deserialize, Serialize};
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Time {
    points: String,
}

pub enum Msg {
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct TimeProps {
    pub x_axis: f64,
    pub size: f64,
    pub width: f64,
}

impl Component for Time {
    type Message = Msg;
    type Properties = TimeProps;

    fn create(ctx: &Context<Self>) -> Self {
        let points = time_svg(ctx.props());
        Self {
            points,
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        self.points = time_svg(ctx.props());
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        html! {
            <div style={"height:9%;background-color:#202020"}>
                <div style="float:left;width:10%">
                    <code></code>
                </div>
                <svg style="float:right;width:90%;height:100%">
                    <polyline points={self.points.clone()} fill="none" stroke="rgb(255,255,255)" />
                </svg>
            </div>
        }
    }
}

fn time_svg(props: &TimeProps) -> String {
    let width = props.width;
    let x_axis = props.x_axis;
    let size = props.size;
    let mut points = "".to_owned();
    let mut scale = (width / size / 10.0 / 40.0).floor();//TODO:10 is freq of clk
    scale = if scale < 1.0 {1.0} else {scale};
    for d in 0..100 {
        let x = (((d as f64) - 1.0 + (x_axis/scale/10.0).floor())*scale*10.0 - x_axis)*size;
        points.push_str(&format!("{:.2},{} {:.2},{} {:.2},{} ", x, 20, x, 10, x,20));//TODO:height should base on html
    }
    points
}
