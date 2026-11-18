//! Sending the webview's answer back to the waiting MCP tool call.

use serde_json::json;
use wasm_bindgen_futures::spawn_local;
use wave_insight_lib::agent::UiResult;

use crate::file_load::invoke;

/// Answer one command id with a result (or an error message).
///
/// Fire-and-forget on purpose: the store dispatch that produced the answer has
/// already happened, and the backend ignores a reply for an id it stopped
/// waiting for (a tool call that timed out).
pub fn send_reply(id: u64, result: UiResult) {
    let args = json!({
        "id": id,
        "result": result,
        "error": serde_json::Value::Null,
    });
    deliver(args);
}

/// Answer one command id with an error the model can read.
pub fn send_reply_error(id: u64, error: impl Into<String>) {
    let args = json!({
        "id": id,
        "result": serde_json::Value::Null,
        "error": error.into(),
    });
    deliver(args);
}

fn deliver(args: serde_json::Value) {
    let args = match serde_wasm_bindgen::to_value(&args) {
        Ok(args) => args,
        Err(_) => return,
    };
    spawn_local(async move {
        let _ = invoke("agent_reply", args).await;
    });
}
