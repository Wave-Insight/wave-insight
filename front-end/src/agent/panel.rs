//! The `AI / MCP` dock panel: what is running, and what to paste where.
//!
//! The panel is the whole user-facing surface of the feature: it reports the
//! endpoint, shows a masked token with a copy button, prints ready-to-paste
//! client config and says what the token grants. Nothing here talks to a model;
//! the conversation happens in the MCP client.

use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::spawn_local;
use wave_insight_lib::agent::McpInfo;
use yew::prelude::*;

use crate::file_load::invoke;

pub enum Msg {
    /// `mcp_info` answered.
    Info(McpInfo),
    /// `mcp_info` failed.
    Error(String),
    /// The refresh button.
    Refresh,
    /// The copied confirmation (or the reason it could not copy).
    Copied(Result<(), String>),
    /// Show the raw config instead of the copied confirmation.
    ShowText,
}

pub struct AgentPanel {
    info: Option<McpInfo>,
    error: Option<String>,
    copy: Option<Result<(), String>>,
    show_text: bool,
}

impl AgentPanel {
    fn refresh(&self, ctx: &Context<Self>) {
        let link = ctx.link().clone();
        spawn_local(async move {
            match invoke("mcp_info", wasm_bindgen::JsValue::UNDEFINED).await {
                value if value.is_undefined() || value.is_null() => {
                    link.send_message(Msg::Error(
                        "the MCP server is not available in this build".to_string(),
                    ));
                }
                value => match serde_wasm_bindgen::from_value::<McpInfo>(value) {
                    Ok(info) => link.send_message(Msg::Info(info)),
                    Err(error) => link.send_message(Msg::Error(format!(
                        "the MCP status could not be read: {error}"
                    ))),
                },
            }
        });
    }

    /// The client command for the reported endpoint.
    fn claude_command(info: &McpInfo) -> String {
        let url = info
            .url
            .clone()
            .unwrap_or_else(|| "http://127.0.0.1:7437/mcp".to_string());
        format!(
            "claude mcp add --transport http wave-insight {url} \\\n  --header \"Authorization: Bearer {}\"",
            info.token
        )
    }

    /// The generic `mcpServers` entry other clients paste.
    fn json_config(info: &McpInfo) -> String {
        let url = info
            .url
            .clone()
            .unwrap_or_else(|| "http://127.0.0.1:7437/mcp".to_string());
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                "wave-insight": {
                    "type": "http",
                    "url": url,
                    "headers": { "Authorization": format!("Bearer {}", info.token) }
                }
            }
        }))
        .unwrap_or_default()
    }

    fn mask(token: &str) -> String {
        if token.len() <= 12 {
            return token.to_string();
        }
        format!("{}…{}", &token[..8], &token[token.len() - 4..])
    }

    fn copy(link: yew::html::Scope<Self>, text: String) {
        let Some(window) = web_sys::window() else {
            link.send_message(Msg::Copied(Err("no window".to_string())));
            return;
        };
        // `navigator.clipboard` needs a secure context and the feature is only
        // compiled in where it is asked for; fall back to a hidden textarea +
        // `document.execCommand("copy")`, which the webview allows everywhere.
        let document = window.document();
        let navigator = window.navigator();
        let clipboard_available = js_sys::Reflect::get(&navigator, &"clipboard".into())
            .map(|value| !value.is_undefined() && !value.is_null())
            .unwrap_or(false);
        if clipboard_available {
            let clipboard = navigator.clipboard();
            let promise = clipboard.write_text(&text);
            spawn_local(async move {
                match wasm_bindgen_futures::JsFuture::from(promise).await {
                    Ok(_) => link.send_message(Msg::Copied(Ok(()))),
                    Err(_) => link.send_message(Msg::Copied(fallback_copy(document, &text))),
                }
            });
            return;
        }
        link.send_message(Msg::Copied(fallback_copy(document, &text)));
    }
}

/// The clipboard fallback: a temporary textarea and `execCommand("copy")`.
///
/// A webview can deny `navigator.clipboard.writeText` (no user gesture from its
/// point of view, or a non-secure origin); the old path still works and the
/// panel reports whether it did.
fn fallback_copy(document: Option<web_sys::Document>, text: &str) -> Result<(), String> {
    let Some(document) = document else {
        return Err("the clipboard is blocked here; select the text instead".to_string());
    };
    let body = document
        .body()
        .ok_or_else(|| "the clipboard is blocked here; select the text instead".to_string())?;
    let textarea = document
        .create_element("textarea")
        .map_err(|_| "the clipboard is blocked here; select the text instead".to_string())?;
    let textarea: web_sys::HtmlElement = textarea
        .dyn_into()
        .map_err(|_| "the clipboard is blocked here; select the text instead".to_string())?;
    textarea.set_text_content(Some(text));
    let _ = body.append_child(&textarea);
    textarea.focus().ok();
    // `exec_command` is not in the web-sys feature list this crate compiles
    // with, so the call goes through `js_sys` (the DOM method is still there).
    let copy = js_sys::Reflect::get(&document, &"execCommand".into())
        .ok()
        .and_then(|function| function.dyn_into::<js_sys::Function>().ok())
        .and_then(|function| function.call1(&document, &"copy".into()).ok())
        .and_then(|result| result.as_bool())
        .unwrap_or(false);
    let _ = body.remove_child(&textarea);
    if copy {
        Ok(())
    } else {
        Err("the clipboard is blocked here; select the text instead".to_string())
    }
}

impl Component for AgentPanel {
    type Message = Msg;
    type Properties = ();

    fn create(ctx: &Context<Self>) -> Self {
        let this = Self {
            info: None,
            error: None,
            copy: None,
            show_text: false,
        };
        this.refresh(ctx);
        this
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::Info(info) => {
                self.info = Some(info);
                self.error = None;
                self.copy = None;
                true
            }
            Msg::Error(error) => {
                self.error = Some(error);
                true
            }
            Msg::Refresh => {
                self.copy = None;
                self.error = None;
                self.refresh(ctx);
                true
            }
            Msg::Copied(result) => {
                self.copy = Some(result);
                true
            }
            Msg::ShowText => {
                self.show_text = !self.show_text;
                true
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        let link = ctx.link();
        let body = match (&self.info, &self.error) {
            (_, Some(error)) => html! {
                <>
                    <div class="agent-line agent-error">{error.clone()}</div>
                    <button type="button" class="split-btn" onclick={link.callback(|_| Msg::Refresh)}>
                        {"Retry"}
                    </button>
                </>
            },
            (None, None) => html! { <div class="agent-line">{"Starting the MCP server…"}</div> },
            (Some(info), None) => {
                let command = Self::claude_command(info);
                let config = Self::json_config(info);
                html! {
                    <>
                        <div class="agent-line">
                            <span class={if info.running {"agent-dot on"} else {"agent-dot"}}></span>
                            {if info.running { "MCP server running" } else { "MCP server not running" }}
                            {if let Some(port) = info.port {
                                html! { <span class="agent-muted">{format!(" · port {port}")}</span> }
                            } else {
                                html! {}
                            }}
                            <span class="agent-muted">{format!(" · v{}", info.version)}</span>
                        </div>
                        {if let Some(url) = &info.url {
                            html! { <div class="agent-line"><code class="agent-code">{url.clone()}</code></div> }
                        } else {
                            html! {}
                        }}
                        {if let Some(error) = &info.error {
                            html! { <div class="agent-line agent-error">{error.clone()}</div> }
                        } else {
                            html! {}
                        }}
                        <div class="agent-line agent-muted">
                            {"Token "}
                            <code class="agent-token">{Self::mask(&info.token)}</code>
                            {" — the full token is in the config below."}
                        </div>
                        <div class="agent-line">
                            <button type="button" class="split-btn"
                                onclick={{
                                    let command = command.clone();
                                    let copy_link = link.clone();
                                    let link = link.clone();
                                    link.callback(move |_| {
                                        Self::copy(copy_link.clone(), command.clone());
                                        Msg::ShowText
                                    })
                                }}>
                                {"Copy Claude Code command"}
                            </button>
                            <button type="button" class="split-btn"
                                onclick={{
                                    let config = config.clone();
                                    let copy_link = link.clone();
                                    let link = link.clone();
                                    link.callback(move |_| {
                                        Self::copy(copy_link.clone(), config.clone());
                                        Msg::ShowText
                                    })
                                }}>
                                {"Copy JSON config"}
                            </button>
                            <button type="button" class="split-btn"
                                onclick={link.callback(|_| Msg::Refresh)}>
                                {"Refresh"}
                            </button>
                        </div>
                        {match &self.copy {
                            Some(Ok(())) => html! { <div class="agent-line">{"Copied."}</div> },
                            Some(Err(error)) => html! { <div class="agent-line agent-error">{error.clone()}</div> },
                            None => html! {},
                        }}
                        <div class="agent-line agent-muted">{"Claude Code"}</div>
                        <pre class="agent-code-block">{command}</pre>
                        <div class="agent-line agent-muted">{"Any MCP client (mcpServers)"}</div>
                        <pre class="agent-code-block">{config}</pre>
                        <div class="agent-line agent-muted">{"Tools"}</div>
                        <div class="agent-line">
                            {for info.tools.iter().map(|tool| html! {
                                <code class="agent-tool">{tool.clone()}</code>
                            })}
                        </div>
                        <div class="agent-line">
                            {"Try: “list the open waveforms, add top.pc and top.state, and show t=1000 to 2000.”"}
                        </div>
                        <div class="agent-line agent-warn">
                            {"The token lets the client read waveform files on this machine and change \
                              what this window shows. Keep it private; it is regenerated if you delete \
                              mcp-token from the app config directory."}
                        </div>
                        <div class="agent-line agent-muted">
                            {"The server listens on 127.0.0.1 only. Data tools can analyse any local \
                              dump by path, whether or not it is open here."}
                        </div>
                    </>
                }
            }
        };
        html! {
            <div class="agent-panel" role="region" aria-label="AI / MCP">
                {body}
            </div>
        }
    }
}
