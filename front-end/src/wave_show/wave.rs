use yew::prelude::*;

use wave_insight_lib::data_struct::Signal;

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct WaveShowProps {
    pub signaladd: (String,Signal),
}

pub struct WaveShow {
    signal_name: Vec<String>,
    //signal_path: (Vec<String>,String),
    signal_show: Vec<Vec<(i32,i32)>>,
}

impl Component for WaveShow {
    type Message = ();
    type Properties = WaveShowProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            signal_name: vec![],
            signal_show: vec![],
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn changed(&mut self, ctx: &Context<Self>) -> bool {
        let (signal_name,signal) = &ctx.props().signaladd;
        self.signal_name.push(signal_name.clone());
        self.signal_show.push(signal.value_change.clone());
        true
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        let view_box = format!("0 0 {} {}", 2500, 1500);
        
        html! {
            <div style="display:block;height:400px;overflow-y:auto">
                <div style="float:left;width:100px">
                    {
                        for (&self.signal_name).iter().map(|s| {
                            html!{<p>{s}</p>}
                        })
                    }
                </div>
                <div style="float:right;width:1400px;background-color:#202020">
                    <svg viewBox={view_box}>
                    </svg>
                </div>
            </div>
        }
    }
}
