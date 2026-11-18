//! Loopback HTTP backend for the Wave Insight Web front end.
//!
//! The browser build cannot read local files, and the pure-wasm build decodes
//! dumps inside the wasm heap. This backend keeps the native, memory-mapped
//! reader on the host side: the front end only receives bounded, rendered text
//! through a single `POST /invoke` route, and the same server hosts the static
//! UI so the iframe has one origin to talk to.
//!
//! Trust model: the process is started by the DSH plugin, which prints a fresh
//! random token on stdout in the `LISTENING` line and embeds it in the iframe
//! URL. `/invoke` requires that token, a loopback `Host` header, and either no
//! `Origin` or this server's own origin. That combination blocks DNS rebinding
//! and cross-site requests from reaching `open_dump`, which reads arbitrary
//! local files.
//!
//! Command names and payloads are the Tauri IPC ones the existing front-end
//! remote layer already knows (`open_dump`, `dump_hierarchy`, `dump_window`,
//! `dump_value_at`, `dump_values_at`, `dump_find`, `close_dump`,
//! `close_all_dumps`).

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
    http::{header, HeaderMap, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    env,
    net::SocketAddr,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use wave_insight_lib::waveform::ShowType;

use dumps::DumpRegistry;

#[derive(Clone)]
struct AppState {
    registry: Arc<DumpRegistry>,
    ui_dir: PathBuf,
    token: Arc<String>,
    port: u16,
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

/// `dump_find` takes the Tauri `ShowType` unit-variant name ("Hex"), unlike the
/// window/value commands that take a plain lowercase string.
#[derive(Debug, Deserialize)]
struct FindArgs {
    id: u64,
    signal: usize,
    query: String,
    #[serde(default)]
    from: u64,
    #[serde(default)]
    backwards: bool,
    #[serde(default)]
    show: ShowType,
}

#[derive(Debug, Deserialize)]
struct IdArgs {
    id: u64,
}

fn ok(value: Value) -> Value {
    json!({ "ok": true, "result": value })
}

fn fail(error: impl Into<String>) -> Value {
    json!({ "ok": false, "error": error.into() })
}

fn parse<T: serde::de::DeserializeOwned>(args: &Value, cmd: &str) -> Result<T, Value> {
    serde_json::from_value(args.clone()).map_err(|error| fail(format!("{cmd}: {error}")))
}

/// Run one registry call on the blocking pool: every registry method takes a
/// synchronous mutex and may touch the filesystem or mmap for seconds, which
/// must not stall the HTTP task answering the other panels.
async fn blocking<T, F>(state: &AppState, job: F) -> Value
where
    T: Serialize + Send + 'static,
    F: FnOnce(&DumpRegistry) -> Result<T, String> + Send + 'static,
{
    let registry = state.registry.clone();
    match tokio::task::spawn_blocking(move || job(&registry)).await {
        Ok(Ok(value)) => ok(serde_json::to_value(value).unwrap_or(Value::Null)),
        Ok(Err(error)) => fail(error),
        Err(error) => fail(format!("backend worker failed: {error}")),
    }
}

async fn dispatch(state: &AppState, request: InvokeRequest) -> Value {
    match request.cmd.as_str() {
        "ping" => ok(json!({ "ok": true, "service": "wave-insight-web-backend", "version": 2 })),
        "open_dump" => match parse::<OpenArgs>(&request.args, "open_dump") {
            Ok(args) => blocking(state, move |registry| registry.open(&args.path)).await,
            Err(error) => error,
        },
        "dump_hierarchy" => match parse::<HierarchyArgs>(&request.args, "dump_hierarchy") {
            Ok(args) => {
                blocking(state, move |registry| {
                    registry.hierarchy(args.id, args.offset, args.limit)
                })
                .await
            }
            Err(error) => error,
        },
        "dump_window" => match parse::<WindowArgs>(&request.args, "dump_window") {
            Ok(args) => {
                blocking(state, move |registry| {
                    registry.window(
                        args.id,
                        &args.signals,
                        args.start,
                        args.end,
                        args.max_changes_per_signal,
                        args.show.as_deref(),
                    )
                })
                .await
            }
            Err(error) => error,
        },
        "dump_value_at" => match parse::<ValueAtArgs>(&request.args, "dump_value_at") {
            Ok(args) => {
                blocking(state, move |registry| {
                    registry.value_at(args.id, args.signal, args.time, args.show.as_deref())
                })
                .await
            }
            Err(error) => error,
        },
        "dump_values_at" => match parse::<ValuesAtArgs>(&request.args, "dump_values_at") {
            Ok(args) => {
                blocking(state, move |registry| {
                    registry.values_at(args.id, &args.signals, args.time, args.show.as_deref())
                })
                .await
            }
            Err(error) => error,
        },
        "dump_find" => match parse::<FindArgs>(&request.args, "dump_find") {
            Ok(args) => {
                blocking(state, move |registry| {
                    registry.find(
                        args.id,
                        args.signal,
                        &args.query,
                        args.from,
                        args.backwards,
                        &args.show,
                    )
                })
                .await
            }
            Err(error) => error,
        },
        "close_dump" => match parse::<IdArgs>(&request.args, "close_dump") {
            Ok(args) => blocking(state, move |registry| registry.close(args.id)).await,
            Err(error) => error,
        },
        "close_all_dumps" => ok(json!(state.registry.close_all())),
        other => fail(format!("unknown command: {other}")),
    }
}

/// `/invoke` is reachable only with this process's token, from this origin.
fn authorized(state: &AppState, headers: &HeaderMap) -> bool {
    let token = headers
        .get("x-wave-token")
        .and_then(|value| value.to_str().ok());
    match token {
        Some(value) => value == state.token.as_str(),
        None => false,
    }
}

/// Reject a request whose `Host` is not this loopback listener (a rebound
/// hostname would carry the attacker's name here) or whose `Origin` is not
/// this server.
fn trusted_request(state: &AppState, headers: &HeaderMap) -> bool {
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let loopback = format!("127.0.0.1:{}", state.port);
    let localhost = format!("localhost:{}", state.port);
    if host != loopback && host != localhost {
        return false;
    }
    match headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        None => true,
        Some(origin) => {
            origin == format!("http://{loopback}") || origin == format!("http://{localhost}")
        }
    }
}

async fn invoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<InvokeRequest>,
) -> Response {
    if !authorized(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(fail("unauthorized: missing or invalid viewer token")),
        )
            .into_response();
    }
    if !trusted_request(&state, &headers) {
        return (
            StatusCode::FORBIDDEN,
            Json(fail("unauthorized: request was not addressed to this viewer")),
        )
            .into_response();
    }
    let mut response = Json(dispatch(&state, request).await).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

/// Liveness probe for the DSH panel: no token, no dump access, no caching.
async fn health() -> Response {
    let mut response = Json(json!({ "ok": true, "service": "wave-insight-web-backend" })).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
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

/// Content-hashed bundles may be cached forever; the entry documents may not.
fn cache_control(path: &str) -> &'static str {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == "index.html" || name == "dsh-invoke.js" || name.ends_with(".json") {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
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
            .header(header::CACHE_CONTROL, cache_control(raw))
            .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
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

/// A fresh capability token per process. The DSH plugin prints it next to the
/// port and embeds it in the iframe URL; the front-end shim echoes it back on
/// every `/invoke`.
fn new_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_err() {
        // Extremely unlikely; keep the server usable rather than refusing to
        // start, with a value that still varies per process and run.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        bytes = (nanos ^ (u128::from(std::process::id()) << 64)).to_le_bytes();
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ui_dir = PathBuf::from(arg_value("--ui-dir").unwrap_or_else(|| "dist".to_owned()));
    let port: u16 = arg_value("--port")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let address = SocketAddr::from(([127, 0, 0, 1], port));

    let listener = tokio::net::TcpListener::bind(address).await?;
    let bound = listener.local_addr()?.port();
    let token = new_token();

    let state = AppState {
        registry: Arc::new(DumpRegistry::new()),
        ui_dir,
        token: Arc::new(token.clone()),
        port: bound,
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/invoke", post(invoke))
        .fallback(get(static_file))
        .with_state(state);

    // The Host plugin reads this line to learn the port and the token.
    println!("LISTENING {bound} {token}");
    axum::serve(listener, app).await?;
    Ok(())
}
