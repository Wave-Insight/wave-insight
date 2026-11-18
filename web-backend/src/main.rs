//! Loopback HTTP backend for the Wave Insight Web front end.
//!
//! The browser build cannot read local files, and the pure-wasm build decodes
//! dumps inside the wasm heap. This backend keeps the native, memory-mapped
//! reader on the host side: the front end only receives bounded, rendered text
//! through a single `POST /invoke` route, and the same server hosts the static
//! UI so the iframe has one same-origin to talk to.
//!
//! Command names and payloads are the Tauri IPC ones the existing front-end
//! remote layer already knows (`open_dump`, `dump_hierarchy`, `dump_window`,
//! `dump_value_at`, `dump_values_at`, `close_dump`, `close_all_dumps`).

// The registry is plain Rust (no Tauri types); include it verbatim so both the
// desktop app and this daemon use the same dump semantics.
// Several registry helpers are desktop-only; this daemon exposes the subset
// the Web front end needs, so their dead_code warnings are expected here.
#[allow(dead_code)]
#[path = "../../src-tauri/src/dumps.rs"]
mod dumps;

use axum::{
    body::Body,
    extract::State,
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    env,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use dumps::DumpRegistry;

#[derive(Clone)]
struct AppState {
    registry: Arc<DumpRegistry>,
    ui_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct InvokeRequest {
    cmd: String,
    #[serde(default)]
    args: Value,
}

#[derive(Debug, Deserialize)]
struct OpenArgs {
    path: String,
}

#[derive(Debug, Deserialize)]
struct HierarchyArgs {
    id: u64,
    offset: usize,
    limit: usize,
}

#[derive(Debug, Deserialize)]
struct WindowArgs {
    id: u64,
    signals: Vec<usize>,
    start: u64,
    end: Option<u64>,
    // The front end sends camelCase (Tauri IPC convention); accept both.
    #[serde(default, alias = "maxChangesPerSignal")]
    max_changes_per_signal: Option<usize>,
    #[serde(default)]
    show: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ValueAtArgs {
    id: u64,
    signal: usize,
    time: u64,
    #[serde(default)]
    show: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ValuesAtArgs {
    id: u64,
    signals: Vec<usize>,
    time: u64,
    #[serde(default)]
    show: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdArgs {
    id: u64,
}

fn ok(value: Value) -> Json<Value> {
    Json(json!({ "ok": true, "result": value }))
}

fn fail(error: impl Into<String>) -> Json<Value> {
    Json(json!({ "ok": false, "error": error.into() }))
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value, cmd: &str) -> Result<T, Json<Value>> {
    serde_json::from_value(args.clone()).map_err(|error| fail(format!("{cmd}: {error}")))
}

async fn invoke(State(state): State<AppState>, Json(request): Json<InvokeRequest>) -> Json<Value> {
    let registry = &state.registry;
    match request.cmd.as_str() {
        "ping" => ok(json!({ "ok": true })),
        "open_dump" => {
            let args: OpenArgs = match parse(&request.args, "open_dump") {
                Ok(v) => v,
                Err(e) => return e,
            };
            match registry.open(&args.path) {
                Ok(summary) => ok(serde_json::to_value(summary).unwrap_or(Value::Null)),
                Err(error) => fail(error),
            }
        }
        "dump_hierarchy" => {
            let args: HierarchyArgs = match parse(&request.args, "dump_hierarchy") {
                Ok(v) => v,
                Err(e) => return e,
            };
            match registry.hierarchy(args.id, args.offset, args.limit) {
                Ok(page) => ok(serde_json::to_value(page).unwrap_or(Value::Null)),
                Err(error) => fail(error),
            }
        }
        "dump_window" => {
            let args: WindowArgs = match parse(&request.args, "dump_window") {
                Ok(v) => v,
                Err(e) => return e,
            };
            match registry.window(
                args.id,
                &args.signals,
                args.start,
                args.end,
                args.max_changes_per_signal,
                args.show.as_deref(),
            ) {
                Ok(windows) => ok(serde_json::to_value(windows).unwrap_or(Value::Null)),
                Err(error) => fail(error),
            }
        }
        "dump_value_at" => {
            let args: ValueAtArgs = match parse(&request.args, "dump_value_at") {
                Ok(v) => v,
                Err(e) => return e,
            };
            match registry.value_at(args.id, args.signal, args.time, args.show.as_deref()) {
                Ok(value) => ok(serde_json::to_value(value).unwrap_or(Value::Null)),
                Err(error) => fail(error),
            }
        }
        "dump_values_at" => {
            let args: ValuesAtArgs = match parse(&request.args, "dump_values_at") {
                Ok(v) => v,
                Err(e) => return e,
            };
            match registry.values_at(args.id, &args.signals, args.time, args.show.as_deref()) {
                Ok(values) => ok(serde_json::to_value(values).unwrap_or(Value::Null)),
                Err(error) => fail(error),
            }
        }
        "close_dump" => {
            let args: IdArgs = match parse(&request.args, "close_dump") {
                Ok(v) => v,
                Err(e) => return e,
            };
            match registry.close(args.id) {
                Ok(()) => ok(Value::Null),
                Err(error) => fail(error),
            }
        }
        "close_all_dumps" => ok(json!(registry.close_all())),
        other => fail(format!("unknown command: {other}")),
    }
}

fn content_type(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "map" => "application/json; charset=utf-8",
        _ => "application/octet-stream",
    }
}

async fn static_file(State(state): State<AppState>, uri: Uri) -> Response {
    let raw = uri.path().trim_start_matches('/');
    let raw = if raw.is_empty() { "index.html" } else { raw };
    let relative = Path::new(raw);
    if relative.components().any(|part| {
        matches!(
            part,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let full = state.ui_dir.join(relative);
    match tokio::fs::read(&full).await {
        Ok(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(raw))
            .body(Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn arg_value(flag: &str) -> Option<String> {
    let mut args = env::args();
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
    }
    None
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ui_dir = PathBuf::from(arg_value("--ui-dir").unwrap_or_else(|| "dist".to_owned()));
    let port: u16 = arg_value("--port")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let address = SocketAddr::from(([127, 0, 0, 1], port));

    let state = AppState {
        registry: Arc::new(DumpRegistry::new()),
        ui_dir,
    };
    let app = Router::new()
        .route("/invoke", post(invoke))
        .fallback(get(static_file))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(address).await?;
    // The host plugin reads this line to learn the OS-assigned port.
    println!("LISTENING {}", listener.local_addr()?.port());
    axum::serve(listener, app).await?;
    Ok(())
}
