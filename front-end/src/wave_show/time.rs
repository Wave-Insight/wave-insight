use serde::{Deserialize, Serialize};
use yew::prelude::*;

///A computed time ruler.
///
///The wave area draws its vertical grid on `labels`, so the grid and the numbers
///above it always line up.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Axis {
    ///the tick comb drawn by the time bar
    pub points: String,
    ///x position and time value of the ticks that get a number
    pub labels: Vec<(f64, i64)>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Time {
    points: String,
    labels: Vec<(f64, i64)>,
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
        let axis = build_axis(ctx.props().x_axis, ctx.props().size, ctx.props().width);
        Self {
            points: axis.points,
            labels: axis.labels,
            cursor1: None,
            cursor2: None,
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let axis = build_axis(ctx.props().x_axis, ctx.props().size, ctx.props().width);
        self.points = axis.points;
        self.labels = axis.labels;
        self.cursor1 = ctx.props().cursor1;
        self.cursor2 = ctx.props().cursor2;
        true
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        html! {
            <div class="timebar">
                <div class="timebar-readout">
                    <code>{
                        if let Some(cur) = self.cursor1.zip(self.cursor2).map(|(x,y)| y - x){
                            format!("Δ {cur}")
                        } else {"Δ -".to_string()}
                    }</code>
                </div>
                <svg class="timebar-svg">
                    {
                        for self.labels.iter().map(|&(x, time)| {
                            html!{
                                <text class="axis-text" x={format!("{x:.2}")} y={14}>
                                    {format!("{time}")}
                                </text>
                            }
                        })
                    }
                    {
                        if let Some(c1) = self.cursor1 {html!{
                            <text class="axis-text cursor-a"
                                x={format!("{}",((c1 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                y={format!("{}",13)}>
                                {
                                    format!("{c1}")
                                }
                            </text>
                        }}else {
                            html!{}
                        }
                    }
                    {
                        if let Some(c2) = self.cursor2 {html!{
                            <text class="axis-text cursor-b"
                                x={format!("{}",((c2 as f64) - ctx.props().x_axis)*ctx.props().size)}
                                y={format!("{}",13)}>
                                {
                                    format!("{c2}")
                                }
                            </text>
                        }}else {
                            html!{}
                        }
                    }
                    <polyline class="axis-line" points={self.points.clone()} />
                </svg>
            </div>
        }
    }
}

///Build the ruler for one zoom/pan state. Used by the time bar itself and by the
///wave area, which draws its grid on the same tick positions.
pub fn build_axis(x_axis: f64, size: f64, width: f64) -> Axis {
    let mut points = "".to_owned();
    let mut scale = (width / size / 10.0 / 40.0).floor();//TODO:10 is freq of clk
    scale = if scale < 1.0 {1.0} else {scale};

    //one tick every `scale*10` time units, label every n-th tick so the numbers
    //stay readable when zoomed in
    let tick_px = scale * 10.0 * size;
    let label_step = ((44.0 / tick_px).ceil() as usize).max(1);
    let head = (x_axis / scale / 10.0).floor();
    let mut labels = Vec::new();

    for d in 0..100 {
        let time = ((d as f64) - 1.0 + head) * scale * 10.0;
        let x = (time - x_axis) * size;
        points.push_str(&format!("{:.2},{} {:.2},{} {:.2},{} ", x, 30, x, 22, x, 30));//TODO:height should base on html
        if d % label_step == 0 {
            labels.push((x, time as i64));
        }
    }
    Axis { points, labels }
}
