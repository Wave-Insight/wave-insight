//! One-way command bridge for the DSH web-remote build.
//!
//! The host cannot call Rust directly in a browser page, so it posts a
//! `message` event to `window`; `web/dsh-invoke.js` forwards those with
//! `data.type === "wave-dsh"` into a DOM `CustomEvent` named
//! `wave-web-command`, whose `detail` is one of:
//!
//! ```text
//! { "kind": "open_path", "path": "/absolute/path/to/dump.fst" }
//! { "kind": "add_signals", "signals": [...] }   // any wave_insight_lib::agent::UiCommand
//! ```
//!
//! `open_path` opens through the same native/HTTP path as the desktop drag &
//! drop; the session is handed to the app's open listener, which delivers
//! `Msg::RemoteReady`. `open_source` reads one `.v`/`.sv` file through the
//! backend's `read_source` and hands the text to the same `Msg::GetVerilog`
//! the file picker uses, so the DSH host can show the source pane the sources
//! its tools are reading. Every other command reuses `Msg::AgentCommand` and
//! the existing reply path in `agent::reply`.

use std::cell::Cell;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;
use wave_insight_lib::agent::UiCommand;

use super::AgentCommand;
use crate::app::Msg;

/// The DOM event the JS shim dispatches for host commands.
pub const EVENT: &str = "wave-web-command";

thread_local! {
    /// Command ids only need to be unique and increasing for one page; the
    /// backend ignores a reply for an id it is no longer waiting on.
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
}

fn next_id() -> u64 {
    NEXT_ID.with(|slot| {
        let id = slot.get();
        slot.set(id.saturating_add(1));
        id
    })
}

/// Register the web command listener; the returned closure must be kept alive
/// for the page (the App stores it in `agent_listener`).
pub fn attach_web_command_listener(
    link: &yew::html::Scope<crate::app::App>,
) -> Option<Closure<dyn FnMut(JsValue)>> {
    let window = web_sys::window()?;
    let link = link.clone();

    let handler = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
        let Some(command) = command_json(&event) else {
            return;
        };

        if command.get("kind").and_then(|kind| kind.as_str()) == Some("open_path") {
            let Some(path) = command
                .get("path")
                .and_then(|path| path.as_str())
                .map(str::to_string)
            else {
                return;
            };
            let link = link.clone();
            spawn_local(async move {
                match crate::remote::try_open(&path).await {
                    Ok(Some(session)) => {
                        // the app's open listener turns this into Msg::RemoteReady;
                        // if the app is gone, do not leak the backend mapping
                        if !crate::remote::deliver_open(session.clone()) {
                            let id = session.id();
                            let _ = crate::remote::close(id).await;
                        }
                    }
                    // web-remote never falls back to a local wasm read, so
                    // `Ok(None)` cannot happen here
                    Ok(None) => {}
                    Err(error) => link.send_message(Msg::Error(
                        crate::remote::native_error_status(&path, &error),
                    )),
                }
            });
            return;
        }

        if command.get("kind").and_then(|kind| kind.as_str()) == Some("open_source") {
            let Some(source) = command
                .get("path")
                .and_then(|path| path.as_str())
                .map(str::to_string)
            else {
                return;
            };
            let link = link.clone();
            spawn_local(async move {
                match crate::remote::read_source(&source).await {
                    Ok(text) => {
                        link.send_message(Msg::GetVerilog((source, text)));
                    }
                    Err(error) => link.send_message(Msg::Error(format!("{source}: {error}"))),
                }
            });
            return;
        }

        match serde_json::from_value::<UiCommand>(command) {
            Ok(command) => link.send_message(Msg::AgentCommand(AgentCommand {
                id: next_id(),
                command,
            })),
            Err(error) => {
                // Unknown/newer command: answer it so the host does not wait
                // for a timeout. The shim's __TAURI__.core.invoke is a no-op.
                super::reply::send_reply_error(
                    next_id(),
                    format!("wave-web-command could not be parsed: {error}"),
                );
            }
        }
    });

    let target: web_sys::EventTarget = window.into();
    if target
        .add_event_listener_with_callback(EVENT, handler.as_ref().unchecked_ref())
        .is_ok()
    {
        Some(handler)
    } else {
        None
    }
}

/// The shim's forwarded `CustomEvent.detail`, round-tripped through JSON so the
/// shared serde definitions stay the only place the command shapes are written.
fn command_json(event: &JsValue) -> Option<serde_json::Value> {
    let detail = js_sys::Reflect::get(event, &JsValue::from_str("detail")).ok()?;
    let text = js_sys::JSON::stringify(&detail).ok()?;
    let text = text.as_string()?;
    serde_json::from_str(&text).ok()
}

/// Tell the DSH host that the page (or a freshly adopted remote dump) is ready.
///
/// Posting is the whole effect: no state is changed and repeating a post is
/// harmless, so the shim's page-ready announcement and this post after every
/// `Msg::RemoteReady` can both fire. The client's view replay is idempotent.
pub fn post_ready() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let Ok(parent) = js_sys::Reflect::get(&window, &JsValue::from_str("parent")) else {
        return;
    };
    let Ok(post) = js_sys::Reflect::get(&parent, &JsValue::from_str("postMessage")) else {
        return;
    };
    let Ok(post) = post.dyn_into::<js_sys::Function>() else {
        return;
    };
    // A plain object, not a serde_wasm_bindgen value: `to_value` turns a JSON
    // map into a JS `Map`, and the parent reads `event.data.type` — which is
    // undefined on a Map, so every handshake was silently dropped.
    let message = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &message,
        &JsValue::from_str("type"),
        &JsValue::from_str("wave-dsh-ready"),
    );
    let _ = post.call2(&parent, &message.into(), &JsValue::from_str("*"));
}
