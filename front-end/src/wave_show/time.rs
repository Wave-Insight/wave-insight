use serde::{Deserialize, Serialize};
use yew::prelude::*;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Time {
    points: String,
    cursor1: Option<i32>,
    cursor2: Option<i32>,
}

pub enum Msg {
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct TimeProps {
    pub x_axis: f64,
    pub size: f64,
    pub width: f64,
    pub cursor1: Option<i32>,
    pub cursor2: Option<i32>,
}

impl Component for Time {
    type Message = Msg;
    type Properties = TimeProps;

    fn create(ctx: &Context<Self>) -> Self {
        let points = time_svg(ctx.props());
        Self {
            points,
            cursor1: None,
            cursor2: None,
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        self.points = time_svg(ctx.props());
        self.cursor1 = ctx.props().cursor1;
        self.cursor2 = ctx.props().cursor2;
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        html! {
            <div style={"height:30px;background-color:#202020"}>
                <div style="float:left;width:10%">
                    <code>{
                        if let Some(cur) = self.cursor1.zip(self.cursor2).map(|(x,y)| y - x){
                            format!("{cur}")
                        } else {"".to_string()}
                    }</code>
                </div>
                <svg style="float:right;width:90%;height:100%">
                    {
                        if let Some(c1) = self.cursor1 {html!{
                            <text x={format!("{}",((c1 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                y={format!("{}",18)}
                                fill="rgb(255,255,0)" >
                                {
                                    format!("{c1}")
                                }
                            </text>
                            //<line x1={format!("{}",((c1 as f64) - ctx.props().x_axis)*ctx.props().size)}
                            //        x2={format!("{}",((c1 as f64) - ctx.props().x_axis)*ctx.props().size)}
                            //        y1={2}
                            //        y2={30}
                            //        style="stroke:rgb(255,255,0)" />
                        }}else {
                            html!{}
                        }
                    }
                    {
                        if let Some(c2) = self.cursor2 {html!{
                            <text x={format!("{}",((c2 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                y={format!("{}",18)}
                                fill="rgb(255,255,255)">
                                {
                                    format!("{c2}")
                                }
                            </text>
                            //<line x1={format!("{}",((c2 as f64) - ctx.props().x_axis)*ctx.props().size)}
                            //    x2={format!("{}",((c2 as f64) - ctx.props().x_axis)*ctx.props().size)}
                            //    y1={2}
                            //    y2={30}
                            //    style="stroke:rgb(255,255,255)" />
                        }}else {
                            html!{}
                        }
                    }
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
        points.push_str(&format!("{:.2},{} {:.2},{} {:.2},{} ", x,29, x,20, x,29));//TODO:height should base on html
    }
    points
}
