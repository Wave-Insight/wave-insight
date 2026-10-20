//! The time ruler above the wave area.
//!
//! R2 step 2 (B2): the pane *subscribes* to the store's committed viewport and
//! cursors instead of being handed numbers as props. A zoom/pan or a cursor
//! click is committed to the store once (on mouseup / one wheel notch); this
//! component re-renders from that subscription, while the wave pane only
//! repaints its canvas — the row list is never diffed for a gesture.

use yew::prelude::*;

use crate::state::{select, Store, Subscription, Viewport};

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

pub enum Msg {
    /// The committed viewport/cursors changed.
    Viewport(Viewport),
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct TimeProps {
    /// The app store: the ruler subscribes to `viewport` itself.
    pub store: Store,
    /// Width of the wave surface, measured by the pane.
    pub width: f64,
}

pub struct Time {
    points: String,
    labels: Vec<(f64, i64)>,
    viewport: Viewport,
    width: f64,
    _viewport_sub: Subscription,
}

impl Component for Time {
    type Message = Msg;
    type Properties = TimeProps;

    fn create(ctx: &Context<Self>) -> Self {
        let store = ctx.props().store.clone();
        let subscription =
            store.subscribe(select::viewport, ctx.link().callback(Msg::Viewport));
        let viewport = store.select(select::viewport);
        let axis = build_axis(viewport.view.x_axis, viewport.view.size, ctx.props().width);
        Self {
            points: axis.points,
            labels: axis.labels,
            viewport,
            width: ctx.props().width,
            _viewport_sub: subscription,
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Viewport(viewport) => {
                self.viewport = viewport;
                self.rebuild();
                true
            }
        }
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        if (self.width - ctx.props().width).abs() > 0.5 {
            self.width = ctx.props().width;
            self.rebuild();
            true
        } else {
            false
        }
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        let view = self.viewport.view;
        let cursors = self.viewport.cursors;
        html! {
            <div class="timebar">
                <div class="timebar-readout">
                    <code>{
                        if let Some(cur) = cursors.one.zip(cursors.two).map(|(x,y)| y - x){
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
                        if let Some(c1) = cursors.one {html!{
                            <text class="axis-text cursor-a"
                                x={format!("{}",((c1 as f64) - view.x_axis)*view.size)}
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
                        if let Some(c2) = cursors.two {html!{
                            <text class="axis-text cursor-b"
                                x={format!("{}",((c2 as f64) - view.x_axis)*view.size)}
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

impl Time {
    fn rebuild(&mut self) {
        let axis = build_axis(self.viewport.view.x_axis, self.viewport.view.size, self.width);
        self.points = axis.points;
        self.labels = axis.labels;
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
