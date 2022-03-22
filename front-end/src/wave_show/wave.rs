use yew::prelude::*;

pub struct WaveShow {
}

impl Component for WaveShow {
    type Message = ();
    type Properties = ();

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
        }
    }

    fn update(&mut self, _ctx: &Context<Self>, _msg: Self::Message) -> bool {
        true
    }

    fn view(&self, _ctx: &Context<Self>) -> Html {
        let view_box = format!("0 0 {} {}", 2500, 1500);
        
        html! {
            <div style="display:block;height:400px;overflow-y:auto">
                <div style="float:left;width:100px">
                </div>
                <div style="float:right;width:1400px;background-color:#202020">
                    <svg viewBox={view_box}>
                    </svg>
                </div>
            </div>
        }
    }
}
