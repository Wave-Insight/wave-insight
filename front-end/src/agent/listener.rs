//! Listening for the backend's command events.
//!
//! The webview has no typed Tauri event API in this crate (the only bindings are
//! `core.invoke` and `webview.onDragDropEvent`), so the listener goes through
//! `window.__TAURI__.event.listen` with `js_sys::Reflect` - the same approach
//! the drag-and-drop handler uses. The event name and payload shape are the
//! contract with `src-tauri/src/agent/server.rs`:
//!
//! ```text
//! "wave-agent-command" { id: number, command: UiCommand }
//! ```

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wave_insight_lib::agent::UiCommand;

use super::AgentCommand;
use crate::app::Msg;

/// The event name the backend emits.
pub const EVENT: &str = "wave-agent-command";

/// Register the command listener; returns the closure so the caller can keep it
/// alive for the page's lifetime.
///
/// A missing `__TAURI__.event` means the page is the browser build (or a very
/// old desktop shell): the listener simply does not attach, and the AI panel
/// reports that the window cannot be reached.
pub fn attach_agent_listener(
    link: &yew::html::Scope<crate::app::App>,
) -> Option<Closure<dyn FnMut(JsValue)>> {
    let window = web_sys::window()?;
    let tauri = js_sys::Reflect::get(&window, &JsValue::from_str("__TAURI__")).ok()?;
    if !tauri.is_object() {
        return None;
    }
    let event = js_sys::Reflect::get(&tauri, &JsValue::from_str("event")).ok()?;
    let listen = js_sys::Reflect::get(&event, &JsValue::from_str("listen")).ok()?;
    let listen: js_sys::Function = listen.dyn_into().ok()?;

    let link = link.clone();
    let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let payload = match js_sys::Reflect::get(&event, &JsValue::from_str("payload")) {
            Ok(payload) => payload,
            Err(_) => return,
        };
        let id = js_sys::Reflect::get(&payload, &JsValue::from_str("id"))
            .ok()
            .and_then(|id| id.as_f64())
            .map(|id| id as u64);
        let Some(id) = id else {
            return;
        };
        // The command is plain data: round-trip it through JSON so the shared
        // serde definition is the only place the shape is written down.
        let command = js_sys::Reflect::get(&payload, &JsValue::from_str("command"))
            .ok()
            .and_then(|command| js_sys::JSON::stringify(&command).ok())
            .and_then(|text| text.as_string())
            .and_then(|text| serde_json::from_str::<UiCommand>(&text).ok());
        match command {
            Some(command) => link.send_message(Msg::AgentCommand(AgentCommand { id, command })),
            None => {
                // A command this build does not understand (a newer backend):
                // answer it so the tool call fails with a message instead of
                // waiting for the timeout.
                super::reply::send_reply_error(
                    id,
                    "this Wave Insight build does not understand that command; update the app",
                );
            }
        }
    });
    let handler_ref = handler.as_ref().unchecked_ref::<js_sys::Function>();
    let _ = listen.call2(&event, &JsValue::from_str(EVENT), handler_ref);
    // the handler lives for the page, like the drag-drop one does
    Some(handler)
}
