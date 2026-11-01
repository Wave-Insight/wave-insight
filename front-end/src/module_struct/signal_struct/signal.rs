use web_sys::HtmlInputElement;
use yew::prelude::*;

use super::search::{compile_regex, matches_plain, MatchOptions};
use super::SignalComponent;
use crate::nav::SignalRow;

pub enum Msg {
    GetClick(String),
    SetFilter(String),
    /// `Aa`: match case exactly.
    ToggleCase,
    /// `ab`: only whole words.
    ToggleWholeWord,
    /// `.*`: treat the query as a regular expression.
    ToggleRegex,
    /// The `x` button: empty the query, keeping the toggles.
    Clear,
}

#[derive(Debug, Properties, PartialEq, Clone)]
pub struct SignalStructProps {
    /// Signals declared in the selected scope.
    pub signals: Vec<SignalRow>,
    /// A signal was clicked: its hierarchical path.
    #[prop_or_default]
    pub signaladd: Callback<String>,
}

pub struct SignalStruct {
    filter: String,
    options: MatchOptions,
}

impl Component for SignalStruct {
    type Message = Msg;
    type Properties = SignalStructProps;

    fn create(_ctx: &Context<Self>) -> Self {
        Self {
            filter: "".to_string(),
            // the pre-toggle behaviour: case-insensitive substring
            options: MatchOptions::default(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        let props = ctx.props();
        match msg {
            Msg::GetClick(path) => {
                props.signaladd.emit(path);
                true
            }
            Msg::SetFilter(s) => {
                self.filter = s;
                true
            }
            Msg::ToggleCase => {
                self.options.case_sensitive = !self.options.case_sensitive;
                true
            }
            Msg::ToggleWholeWord => {
                self.options.whole_word = !self.options.whole_word;
                true
            }
            Msg::ToggleRegex => {
                self.options.regex = !self.options.regex;
                true
            }
            Msg::Clear => {
                self.filter.clear();
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let query = self.filter.trim();
        // The regex mode compiles the query with the browser's engine. An
        // invalid pattern is a user typing, not a crash: it matches nothing and
        // the bar says so.
        let (regex, invalid) = if self.options.regex && !query.is_empty() {
            match compile_regex(query, self.options) {
                Some(regex) => (Some(regex), false),
                None => (None, true),
            }
        } else {
            (None, false)
        };
        let visible: Vec<&SignalRow> = ctx
            .props()
            .signals
            .iter()
            .filter(|signal| {
                if query.is_empty() {
                    return true;
                }
                if self.options.regex {
                    regex.as_ref().is_some_and(|regex| regex.test(&signal.name))
                } else {
                    matches_plain(&signal.name, query, self.options)
                }
            })
            .collect();
        let count = visible.len();
        let link = ctx.link();
        let flag = |on: bool| if on { "btn active" } else { "btn" };
        html! {
            <div class="sig-panel">
                <div class="sig-scroll">
                {
                    for visible.into_iter().map(|signal| { html!{
                        <SignalComponent
                            name={signal.display_name()}
                            width={signal.width}
                            path={signal.path.clone()}
                            onclick={link.callback(Msg::GetClick)}/>
                    }})
                }
                if count == 0 && !query.is_empty() {
                    <div class="panel-note">{if invalid { "invalid regular expression" } else { "no signal matches" }}</div>
                }
                </div>
                <div class="filter-wrap">
                    <input type="text" placeholder="filter signals…" value={self.filter.clone()}
                        oninput={link.callback(|e: InputEvent| Msg::SetFilter(e.target_unchecked_into::<HtmlInputElement>().value()))} />
                    // inline layout: `styles.css` is outside this change's
                    // scope, so the row cannot rely on a new rule yet
                    <div class="search-flags" style="display:flex;gap:6px;align-items:center;margin-top:6px">
                        <button type="button" class={flag(self.options.case_sensitive)}
                            title="Match case"
                            onclick={link.callback(|_| Msg::ToggleCase)}>{"Aa"}</button>
                        <button type="button" class={flag(self.options.whole_word)}
                            title="Whole word only"
                            onclick={link.callback(|_| Msg::ToggleWholeWord)}>{"ab"}</button>
                        <button type="button" class={flag(self.options.regex)}
                            title="Regular expression"
                            onclick={link.callback(|_| Msg::ToggleRegex)}>{".*"}</button>
                        <button type="button" class="btn" title="Clear the search"
                            onclick={link.callback(|_| Msg::Clear)}>{"×"}</button>
                    </div>
                </div>
            </div>
        }
    }
}
